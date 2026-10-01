use crate::{client_settings::CaptureSource, network::SessionEvent};

/// Read-only observer for the generic recognition/translation event stream.
///
/// Implementations must return quickly. Any storage or blocking work belongs
/// on a plugin-owned worker queue.
pub trait SessionEventSubscriber: Send + Sync {
    fn on_session_event(&self, event: &SessionEvent);

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
    Clear,
}

pub trait HostOutputSubscriber: Send + Sync {
    fn on_host_output(&self, event: HostOutputEvent<'_>);
}
