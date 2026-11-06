use crate::client_settings::CaptureSource;

/// How a translated caption changes a consumer's current stream entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionUpdate {
    Replace,
    Append,
    /// Finalize the current caption with these contents. The next live caption
    /// arrives separately as Replace, including for the legacy window protocol.
    RollOver,
}

/// Presentation event emitted after host history merging. External output
/// plugins consume this instead of being named inside the event pump.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostOutputEvent<'a> {
    /// Complete replaceable preview set for one stream, separate from history.
    Previews {
        stream_id: u64,
        is_typing: bool,
        entries: &'a [super::TranslationPreview],
    },
    /// A validated, completed segment accepted into host history. Stable identity
    /// lets irreversible outputs consume it once even when captions are revised.
    CommittedTranslation {
        stream_id: u64,
        turn_id: &'a str,
        segment_index: u32,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &'a str,
        translated: &'a str,
        additional_translations: &'a [xrtranslate_protocol::AdditionalTranslation],
        speaker: &'a str,
        asr_only: bool,
    },
    Caption {
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &'a str,
        translated: &'a str,
        speaker: &'a str,
        additional_translations: &'a [xrtranslate_protocol::AdditionalTranslation],
        asr_only: bool,
        update: CaptionUpdate,
    },
    StreamEnded(u64),
    /// Explicit cancellation invalidates completed output still awaiting delivery.
    StreamCancelled(u64),
    Clear,
}

pub trait HostOutputSubscriber: Send + Sync {
    fn on_host_output(&self, event: HostOutputEvent<'_>);
}
