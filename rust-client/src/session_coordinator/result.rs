use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{client_settings::CaptureSource, network::SessionEvent};
use xrtranslate_protocol::{SegmentBoundary, SegmentTiming};

/// Translation facts shared by input plugins and their downstream consumers.
#[derive(Clone, Debug)]
pub struct TranslationSegment {
    pub stream_id: u64,
    pub audio_source: CaptureSource,
    pub turn_id: String,
    pub segment_index: u32,
    pub segment_count: u32,
    pub source: String,
    pub translated: Option<String>,
    pub additional_translations: Vec<xrtranslate_protocol::AdditionalTranslation>,
    pub asr_only: bool,
    pub speaker_id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    pub timing: SegmentTiming,
    pub boundary: SegmentBoundary,
    pub revisable: bool,
    pub live: bool,
}

#[derive(Clone, Debug)]
pub enum TranslationOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum TranslationEvent {
    Segment(TranslationSegment),
    /// Complete replacement for this turn's current source or translation batch.
    ReplaceSegments(Vec<TranslationSegment>),
    StreamEnded {
        stream_id: u64,
    },
    Finished {
        stream_id: u64,
        outcome: TranslationOutcome,
    },
}

struct Snapshot {
    revision: u64,
    count: u32,
    segments: BTreeMap<u32, TranslationSegment>,
    published_revisable: Option<bool>,
}

/// Normalizes results before presentation policy is applied. Each source and
/// translation snapshot is delivered in order after every part arrives. A live
/// snapshot may be promoted to final without changing its recognition revision.
#[derive(Default)]
pub(crate) struct TranslationEventAdapter {
    snapshots: HashMap<(u64, String, bool), Snapshot>,
    published_revision: HashMap<(u64, String), u64>,
    finished: HashSet<u64>,
}

impl TranslationEventAdapter {
    pub fn push(&mut self, event: &SessionEvent, stream_id: u64) -> Vec<TranslationEvent> {
        if self.finished.contains(&stream_id) {
            return Vec::new();
        }
        let outcome = match event {
            SessionEvent::Disconnected(reason) => Some(match reason.as_str() {
                "Finished" => TranslationOutcome::Completed,
                "Cancelled" => TranslationOutcome::Cancelled,
                _ => TranslationOutcome::Failed("Translation connection was interrupted".into()),
            }),
            SessionEvent::Error(message) | SessionEvent::BackendError { message, .. } => {
                Some(TranslationOutcome::Failed(message.clone()))
            }
            _ => None,
        };
        if let Some(outcome) = outcome {
            self.snapshots
                .retain(|(stream, _, _), _| *stream != stream_id);
            self.published_revision
                .retain(|(stream, _), _| *stream != stream_id);
            self.finished.insert(stream_id);
            return vec![TranslationEvent::Finished { stream_id, outcome }];
        }
        if let SessionEvent::StreamEnded { stream_id, .. } = event {
            self.snapshots
                .retain(|(stream, _, _), _| stream != stream_id);
            self.published_revision
                .retain(|(stream, _), _| stream != stream_id);
            return vec![TranslationEvent::StreamEnded {
                stream_id: *stream_id,
            }];
        }
        let translated = match event {
            SessionEvent::Translation { translated, .. } => Some(translated.clone()),
            _ => None,
        };
        let (additional_translations, asr_only) = match event {
            SessionEvent::Translation {
                additional_translations,
                asr_only,
                ..
            } => (additional_translations.clone(), *asr_only),
            _ => (Vec::new(), false),
        };
        let (segment, authoritative, revision) = match event {
            SessionEvent::SourceSegment {
                stream_id,
                audio_source,
                continuous,
                text: source,
                turn_id,
                segment_index,
                segment_count,
                speaker_id,
                source_start_ms,
                source_end_ms,
                timing,
                boundary,
                revisable,
                authoritative_snapshot,
                revision,
                ..
            }
            | SessionEvent::Translation {
                stream_id,
                audio_source,
                continuous,
                source,
                turn_id,
                segment_index,
                segment_count,
                speaker_id,
                source_start_ms,
                source_end_ms,
                timing,
                boundary,
                revisable,
                authoritative_snapshot,
                revision,
                ..
            } => (
                TranslationSegment {
                    stream_id: *stream_id,
                    audio_source: *audio_source,
                    turn_id: turn_id.clone(),
                    segment_index: *segment_index,
                    segment_count: *segment_count,
                    source: source.clone(),
                    translated,
                    additional_translations,
                    asr_only,
                    speaker_id: speaker_id.clone(),
                    source_start_ms: *source_start_ms,
                    source_end_ms: *source_end_ms,
                    timing: *timing,
                    boundary: *boundary,
                    revisable: *revisable,
                    live: *continuous
                        && *revisable
                        && (!authoritative_snapshot || segment_index == segment_count),
                },
                *authoritative_snapshot,
                *revision,
            ),
            _ => return Vec::new(),
        };
        let TranslationSegment {
            stream_id,
            segment_index,
            segment_count,
            ..
        } = segment;
        if self.finished.contains(&stream_id) {
            return Vec::new();
        }
        if !authoritative {
            return vec![TranslationEvent::Segment(segment)];
        }
        if segment_count == 0 || segment_index == 0 || segment_index > segment_count {
            return Vec::new();
        }
        let turn_key = (stream_id, segment.turn_id.clone());
        if self
            .published_revision
            .get(&turn_key)
            .is_some_and(|published| revision < *published)
        {
            return Vec::new();
        }
        let key = (
            stream_id,
            segment.turn_id.clone(),
            segment.translated.is_some(),
        );
        let snapshot = self.snapshots.entry(key).or_insert_with(|| Snapshot {
            revision,
            count: segment_count,
            segments: BTreeMap::new(),
            published_revisable: None,
        });
        if revision < snapshot.revision
            || (revision == snapshot.revision && snapshot.published_revisable == Some(false))
        {
            return Vec::new();
        }
        if revision > snapshot.revision {
            *snapshot = Snapshot {
                revision,
                count: segment_count,
                segments: BTreeMap::new(),
                published_revisable: None,
            };
        }
        if snapshot.count != segment_count {
            if snapshot.segments.is_empty() && snapshot.published_revisable == Some(true) {
                snapshot.count = segment_count;
            } else {
                return Vec::new();
            }
        }
        snapshot.segments.insert(segment_index, segment);
        if snapshot.segments.len() != segment_count as usize {
            return Vec::new();
        }
        let revisable = snapshot.segments.values().any(|segment| segment.revisable);
        if snapshot.published_revisable == Some(true) && revisable {
            snapshot.segments.clear();
            return Vec::new();
        }
        snapshot.published_revisable = Some(revisable);
        self.published_revision.insert(turn_key, revision);
        vec![TranslationEvent::ReplaceSegments(
            std::mem::take(&mut snapshot.segments)
                .into_values()
                .collect(),
        )]
    }
}
