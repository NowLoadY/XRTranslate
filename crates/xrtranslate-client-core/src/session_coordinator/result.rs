use crate::recognition::CaptureSource;
use xrtranslate_protocol::{SegmentBoundary, SegmentTiming};
/// Translation facts shared by input plugins and their downstream consumers.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TranslationSegment {
    pub stream_id: u64,
    pub audio_source: CaptureSource,
    pub turn_id: String,
    pub segment_index: u32,
    pub segment_count: u32,
    pub source: String,
    pub translated: Option<String>,
    pub source_lang: String,
    pub target_lang: String,
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum TranslationOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum TranslationEvent {
    SeparatedAudio(xrtranslate_protocol::SeparatedAudio),
    /// Replaceable presentation only; never a durable result or TTS input.
    Preview(super::preview::TranslationPreview),
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
