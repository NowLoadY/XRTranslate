use super::{TranslationEvent, TranslationSessionOwner};
use crate::client_settings::CaptureSource;
use serde::{Deserialize, Serialize};

/// Per-output audio selection. Text results are independent of audio capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSourceFilter {
    pub microphone: bool,
    pub system_audio: bool,
}

impl Default for AudioSourceFilter {
    fn default() -> Self {
        Self {
            microphone: true,
            system_audio: true,
        }
    }
}

impl AudioSourceFilter {
    pub fn allows(self, source: CaptureSource, is_text: bool) -> bool {
        is_text
            || match source {
                CaptureSource::Microphone => self.microphone,
                CaptureSource::SystemAudio => self.system_audio,
                // Live capture expands Both into separate routes. A legacy mixed
                // caption cannot exclude either source, so requires both enabled.
                CaptureSource::Both => self.microphone && self.system_audio,
            }
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_source_filter_handles_each_selection_without_filtering_text() {
        for microphone in [false, true] {
            for system_audio in [false, true] {
                let filter = AudioSourceFilter {
                    microphone,
                    system_audio,
                };
                assert_eq!(filter.allows(CaptureSource::Microphone, false), microphone);
                assert_eq!(
                    filter.allows(CaptureSource::SystemAudio, false),
                    system_audio
                );
                assert_eq!(
                    filter.allows(CaptureSource::Both, false),
                    microphone && system_audio
                );
                for source in [
                    CaptureSource::Microphone,
                    CaptureSource::SystemAudio,
                    CaptureSource::Both,
                ] {
                    assert!(filter.allows(source, true));
                }
            }
        }
        let partial: AudioSourceFilter = serde_json::from_str(r#"{"microphone": false}"#).unwrap();
        assert!(!partial.microphone);
        assert!(partial.system_audio);
    }
}
