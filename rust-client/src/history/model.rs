use crate::{client_settings::CaptureSource, streaming::RevisableText};
use xrtranslate_protocol::{CorpusTermMatch, SegmentBoundary, SegmentTiming};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecognitionHistoryEntry {
    pub(crate) stream_id: Option<u64>,
    pub(crate) live: bool,
    pub(crate) text: String,
    pub(crate) turn_id: String,
    pub(crate) speaker_id: String,
    pub(crate) source_start_ms: f64,
    pub(crate) source_end_ms: f64,
    pub(crate) timing: SegmentTiming,
    pub(crate) boundary: SegmentBoundary,
    pub(crate) activation_matches: Vec<CorpusTermMatch>,
    pub(crate) context_matches: Vec<CorpusTermMatch>,
    pub(crate) revisable: bool,
    pub(crate) overlap_ratio: f32,
    pub(crate) authoritative_snapshot: bool,
    pub(crate) revision_id: u64,
    pub(crate) revision: Option<RevisableText>,
}

pub(crate) struct PendingRecognitionWindow {
    pub(crate) stream_id: u64,
    pub(super) continuous: bool,
    pub(super) turn_id: String,
    pub(super) segment_count: u32,
    pub(super) segments: Vec<(u32, RecognitionHistoryEntry)>,
}

pub(crate) struct PendingAuthoritativeRecognition {
    pub(crate) stream_id: u64,
    pub(crate) turn_id: String,
    pub(crate) revision_id: u64,
    pub(crate) segment_count: u32,
    pub(crate) segments: Vec<(u32, RecognitionHistoryEntry)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingFinalAsr {
    pub(crate) stream_id: u64,
    pub(crate) text: String,
    pub(crate) turn_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TranslationHistoryEntry {
    pub(crate) turn_id: String,
    pub(crate) segment_index: u32,
    pub(crate) stream_id: Option<u64>,
    pub(crate) audio_source: CaptureSource,
    pub(crate) live: bool,
    pub(crate) source: String,
    pub(crate) translated: String,
    pub(crate) additional_translations: Vec<xrtranslate_protocol::AdditionalTranslation>,
    pub(crate) asr_only: bool,
    pub(crate) speaker_id: String,
    pub(crate) source_start_ms: f64,
    pub(crate) source_end_ms: f64,
    pub(crate) timing: SegmentTiming,
    pub(crate) boundary: SegmentBoundary,
    pub(crate) term_matches: Vec<CorpusTermMatch>,
    pub(crate) revisable: bool,
    pub(crate) overlap_ratio: f32,
    pub(crate) authoritative_snapshot: bool,
    pub(crate) revision_id: u64,
    pub(crate) source_revision: Option<RevisableText>,
    pub(crate) translated_revision: Option<RevisableText>,
    pub(crate) additional_revisions: std::collections::BTreeMap<String, RevisableText>,
}

impl TranslationHistoryEntry {
    /// Plain-text projection for subtitle surfaces without annotated spans.
    pub(crate) fn output_text(&self) -> String {
        let mut text = self.translated.clone();
        for extra in &self.additional_translations {
            if extra.translated_text.trim().is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&extra.target_lang);
            text.push_str(" · ");
            text.push_str(&extra.translated_text);
        }
        text
    }

    pub(crate) fn preview(
        stream_id: u64,
        audio_source: CaptureSource,
        preview: xrtranslate_protocol::TranslationPreview,
    ) -> Self {
        Self {
            turn_id: preview.turn_id,
            segment_index: preview.segment_index,
            stream_id: Some(stream_id),
            audio_source,
            live: true,
            source: preview.source_text,
            translated: preview.translated_text,
            additional_translations: Vec::new(),
            asr_only: false,
            speaker_id: preview.speaker_id,
            source_start_ms: 0.0,
            source_end_ms: 0.0,
            timing: SegmentTiming::default(),
            boundary: SegmentBoundary::default(),
            term_matches: Vec::new(),
            revisable: true,
            overlap_ratio: 0.0,
            authoritative_snapshot: false,
            revision_id: preview.revision,
            source_revision: None,
            translated_revision: None,
            additional_revisions: Default::default(),
        }
    }
}

pub(crate) struct StreamMerge {
    pub(crate) entry: TranslationHistoryEntry,
    pub(crate) rolled_over: bool,
    pub(crate) changed: bool,
}

pub(crate) struct PendingAuthoritativeTranslation {
    pub(crate) stream_id: u64,
    pub(crate) revision_id: u64,
    pub(crate) segment_count: u32,
    pub(crate) segments: Vec<(u32, TranslationHistoryEntry)>,
}

pub(crate) struct AuthoritativeTranslationMerge {
    pub(crate) accepted: bool,
    pub(crate) stabilized: Vec<TranslationHistoryEntry>,
    pub(crate) live: Option<TranslationHistoryEntry>,
    pub(crate) changed: bool,
}
