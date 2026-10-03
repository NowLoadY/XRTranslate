use super::{TranslationEvent, TranslationSessionOwner};
use crate::client_settings::CaptureSource;

/// Read-only observer for the generic recognition/translation event stream.
///
/// Implementations must return quickly. Any storage or blocking work belongs
/// on a plugin-owned worker queue.
pub trait SessionEventSubscriber: Send + Sync {
    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent);

    /// Domain consumers opt into events from their own task identity.
    fn accepts_owner(&self, _owner: &super::TranslationSessionOwner) -> bool {
        true
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOutputEvent<'a> {
    /// A validated, completed segment accepted into host history. Stable identity
    /// lets irreversible outputs consume it once even when captions are revised.
    CommittedTranslation {
        stream_id: u64,
        turn_id: &'a str,
        segment_index: u32,
        translated: &'a str,
    },
    Caption {
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &'a str,
        translated: &'a str,
        speaker: &'a str,
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
