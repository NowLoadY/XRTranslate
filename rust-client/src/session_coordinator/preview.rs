use super::{TranslationEvent, TranslationSegment};
use crate::client_settings::CaptureSource;

#[derive(Clone, Debug, PartialEq)]
pub struct TranslationPreview {
    pub stream_id: u64,
    pub audio_source: CaptureSource,
    pub turn_id: String,
    /// Zero identifies an ASR partial before sentence segmentation.
    pub segment_index: u32,
    pub source: String,
    pub translated: Option<String>,
    pub revision: u64,
}

/// Shared in-memory presentation state. Revisions replace the same row;
/// final results, source corrections and lifecycle events retire previews.
#[derive(Clone, Default)]
pub struct LivePreviews {
    entries: Vec<TranslationPreview>,
    completed: Vec<(u64, String, u32, String)>,
}

impl LivePreviews {
    pub fn entries(&self) -> &[TranslationPreview] {
        &self.entries
    }

    pub fn observe(&mut self, event: &TranslationEvent) {
        match event {
            TranslationEvent::Preview(preview) => self.upsert(preview.clone()),
            TranslationEvent::Segment(segment) => self.segment(segment),
            TranslationEvent::ReplaceSegments(segments) => {
                if let Some(first) = segments.first() {
                    self.completed.retain(|(stream, turn, index, source)| {
                        *stream != first.stream_id
                            || *turn != first.turn_id
                            || segments.iter().any(|segment| {
                                *index == segment.segment_index && *source == segment.source
                            })
                    });
                    self.entries.retain(|entry| {
                        entry.stream_id != first.stream_id
                            || entry.turn_id != first.turn_id
                            || segments.iter().any(|segment| {
                                entry.segment_index == segment.segment_index
                                    && entry.source == segment.source
                            })
                    });
                }
                for segment in segments {
                    self.segment(segment);
                }
            }
            TranslationEvent::StreamEnded { stream_id }
            | TranslationEvent::Finished { stream_id, .. } => {
                self.entries.retain(|entry| entry.stream_id != *stream_id);
                self.completed.retain(|(stream, ..)| stream != stream_id);
            }
        }
    }

    fn segment(&mut self, segment: &TranslationSegment) {
        if segment.translated.is_none() {
            self.entries
                .retain(|entry| entry.stream_id != segment.stream_id || entry.segment_index != 0);
        }
        self.completed.retain(|(stream, turn, index, source)| {
            *stream != segment.stream_id
                || *turn != segment.turn_id
                || *index != segment.segment_index
                || (segment.translated.is_none() && *source == segment.source)
        });
        if segment.translated.is_some() {
            self.completed.push((
                segment.stream_id,
                segment.turn_id.clone(),
                segment.segment_index,
                segment.source.clone(),
            ));
        }
        self.entries.retain(|entry| {
            entry.stream_id != segment.stream_id
                || entry.turn_id != segment.turn_id
                || (entry.segment_index != 0
                    && (entry.segment_index != segment.segment_index
                        || (segment.translated.is_none() && entry.source == segment.source)))
        });
        if segment.translated.is_none()
            && !self.completed.iter().any(|(stream, turn, index, source)| {
                *stream == segment.stream_id
                    && *turn == segment.turn_id
                    && *index == segment.segment_index
                    && *source == segment.source
            })
            && !self.entries.iter().any(|entry| {
                entry.stream_id == segment.stream_id
                    && entry.turn_id == segment.turn_id
                    && entry.segment_index == segment.segment_index
                    && entry.translated.is_some()
            })
        {
            self.upsert(TranslationPreview {
                stream_id: segment.stream_id,
                audio_source: segment.audio_source,
                turn_id: segment.turn_id.clone(),
                segment_index: segment.segment_index,
                source: segment.source.clone(),
                translated: None,
                revision: 0,
            });
        }
    }

    fn upsert(&mut self, preview: TranslationPreview) {
        if preview.segment_index == 0 {
            self.entries
                .retain(|entry| entry.stream_id != preview.stream_id || entry.segment_index != 0);
        }
        if self.completed.iter().any(|(stream, turn, index, source)| {
            *stream == preview.stream_id
                && *turn == preview.turn_id
                && *index == preview.segment_index
                && *source == preview.source
        }) {
            return;
        }
        let index = self.entries.iter().position(|entry| {
            entry.stream_id == preview.stream_id
                && entry.turn_id == preview.turn_id
                && entry.segment_index == preview.segment_index
        });
        if index.is_some_and(|index| self.entries[index].revision > preview.revision) {
            return;
        }
        let empty = preview
            .translated
            .as_ref()
            .map_or(preview.source.is_empty(), |text| text.is_empty());
        match index {
            Some(index) if empty => {
                self.entries.remove(index);
            }
            Some(index) => self.entries[index] = preview,
            None if !empty => {
                if self.entries.len() >= 32 {
                    self.entries.remove(0);
                }
                self.entries.push(preview);
            }
            None => {}
        }
    }
}
