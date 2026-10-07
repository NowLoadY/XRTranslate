//! Built-in SteamVR In-Game Overlay plugin for XRTranslate.
//!
//! Provides private, low-latency, HMD-locked bilingual subtitles rendered
//! directly inside VR using the official SteamVR (OpenVR) Compositor Overlay API.

mod avatar;
mod graphics;
mod openvr;
#[cfg(debug_assertions)]
mod preview;
#[cfg(debug_assertions)]
pub(crate) use preview::render as render_avatar_preview;
mod renderer;
pub mod runtime;
mod space;
pub mod ui;

pub use runtime::{VrOverlayHandle, VrOverlayManager, VrOverlaySettings};
pub use ui::{VrOverlayPageContext, VrOverlayUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};

impl HostOutputSubscriber for VrOverlayHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::CommittedTranslation { .. } | HostOutputEvent::StreamCancelled(_) => {}
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                additional_translations,
                asr_only,
                speaker,
                update: CaptionUpdate::RollOver,
                ..
            } => self.roll_stream(
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { source },
                translated,
                additional_translations,
                speaker,
            ),
            HostOutputEvent::Caption {
                stream_id,
                audio_source,
                is_typing,
                source,
                translated,
                additional_translations,
                asr_only,
                speaker,
                update,
                ..
            } => self.add_caption(
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { source },
                translated,
                additional_translations,
                speaker,
                matches!(update, CaptionUpdate::Replace),
            ),
            HostOutputEvent::StreamEnded(stream_id) => self.end_stream(stream_id),
            HostOutputEvent::Clear => self.clear(),
        }
    }
}

/// Owns SteamVR overlay state, manager handle, and settings draft.
pub struct VrOverlayPlugin {
    manager: VrOverlayManager,
    draft: VrOverlaySettings,
    host_enabled: bool,
}

impl VrOverlayPlugin {
    pub fn new(mut draft: VrOverlaySettings, host_enabled: bool) -> Self {
        draft.normalize();
        let mut effective = draft.clone();
        if !host_enabled {
            effective.enabled = false;
        }
        let manager = VrOverlayManager::new(effective);
        Self {
            manager,
            draft,
            host_enabled,
        }
    }

    pub fn manager(&self) -> &VrOverlayManager {
        &self.manager
    }

    pub fn handle(&self) -> VrOverlayHandle {
        self.manager.handle()
    }

    pub fn draft(&self) -> &VrOverlaySettings {
        &self.draft
    }

    pub fn draft_mut(&mut self) -> &mut VrOverlaySettings {
        &mut self.draft
    }

    pub fn set_host_enabled(&mut self, enabled: bool) {
        if self.host_enabled != enabled {
            self.host_enabled = enabled;
            self.sync_settings();
        }
    }

    pub fn sync_settings(&mut self) {
        self.draft.normalize();
        let mut effective = self.draft.clone();
        if !self.host_enabled {
            effective.enabled = false;
        }
        self.manager.update_settings(effective);
    }

    pub fn connect(&self) {
        self.manager.connect();
    }

    pub fn disconnect(&self) {
        self.manager.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_settings::CaptureSource;

    #[test]
    fn tagged_asr_and_additional_targets_share_the_normal_caption_lifecycle() {
        use std::time::{Duration, Instant};
        let plugin = VrOverlayPlugin::new(VrOverlaySettings::default(), true);
        let handle = plugin.handle();
        handle.on_host_output(HostOutputEvent::Caption {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            is_typing: false,
            source: "uncorrected recognition",
            translated: "corrected terminology",
            additional_translations: &[],
            asr_only: true,
            speaker: "",
            update: CaptionUpdate::Append,
        });
        for text in ["こんにちは", "こんばんは"] {
            handle.on_host_output(HostOutputEvent::Caption {
                stream_id: 2,
                audio_source: CaptureSource::Microphone,
                is_typing: false,
                source: "hello",
                translated: "你好",
                additional_translations: &[xrtranslate_protocol::AdditionalTranslation {
                    target_lang: "ja".into(),
                    translated_text: text.into(),
                    term_matches: Vec::new(),
                }],
                asr_only: false,
                speaker: "",
                update: CaptionUpdate::Replace,
            });
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            let status = plugin.manager().status();
            if status
                .latest_caption_preview
                .as_ref()
                .is_some_and(|text| text.contains("こんばんは"))
            {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "caption worker did not consume multilingual output"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.cards.len(), 2);
        assert_eq!(status.cards[0].source, "");
        assert_eq!(status.cards[0].preview_text(true), "corrected terminology");
        assert_eq!(status.cards[1].additional_translations, ["こんばんは"]);
        assert_eq!(status.cards[1].source, "hello");
    }

    #[test]
    fn vr_overlay_plugin_lifecycle_and_settings_sync() {
        let mut plugin = VrOverlayPlugin::new(VrOverlaySettings::default(), true);
        assert!(plugin.draft().enabled);
        assert_eq!(plugin.draft().max_items, 3);
        assert!(plugin.draft().bilingual);

        plugin.draft_mut().max_items = 4;
        plugin.draft_mut().distance_meters = 1.5;
        plugin.sync_settings();

        assert_eq!(plugin.draft().max_items, 4);
        assert_eq!(plugin.draft().distance_meters, 1.5);

        plugin.set_host_enabled(false);
        assert!(!plugin.host_enabled);
    }

    #[test]
    fn host_audio_revisions_and_typed_results_reach_preview_without_dialogue_cooldown() {
        use std::time::{Duration, Instant};
        let plugin = VrOverlayPlugin::new(VrOverlaySettings::default(), true);
        let handle = plugin.handle();
        let send = |id, source, update, is_typing| {
            handle.on_host_output(HostOutputEvent::Caption {
                stream_id: id,
                audio_source: CaptureSource::Microphone,
                is_typing,
                source,
                translated: source,
                additional_translations: &[],
                asr_only: false,
                speaker: "",
                update,
            })
        };
        for _ in 0..100 {
            send(1, "draft", CaptionUpdate::Replace, false);
        }
        send(1, "final correction", CaptionUpdate::RollOver, false);
        send(1, "next live", CaptionUpdate::Replace, false);
        send(2, "typed", CaptionUpdate::Append, true);
        let deadline = Instant::now() + Duration::from_secs(2);
        let snapshot = loop {
            let snapshot = plugin.manager().status();
            if snapshot.cards.last().is_some_and(|c| c.source == "typed") {
                break snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "caption worker did not consume queued output"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(snapshot.cards.len(), 3);
        assert_eq!(snapshot.cards[0].source, "final correction");
        assert!(!snapshot.cards[0].live);
        assert!(snapshot.cards[1].live);
        assert!(!snapshot.cards[2].live);
    }

    #[test]
    fn host_output_subscriber_routes_caption_events() {
        let plugin = VrOverlayPlugin::new(VrOverlaySettings::default(), true);
        let handle = plugin.handle();

        // 1. Initial caption
        handle.on_host_output(HostOutputEvent::Caption {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            is_typing: false,
            source: "Hello world".into(),
            translated: "你好，世界".into(),
            additional_translations: &[],
            asr_only: false,
            speaker: "Speaker 1".into(),
            update: CaptionUpdate::Replace,
        });

        // 2. Rollover caption
        handle.on_host_output(HostOutputEvent::Caption {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            is_typing: false,
            source: "Next line".into(),
            translated: "下一句".into(),
            additional_translations: &[],
            asr_only: false,
            speaker: "Speaker 1".into(),
            update: CaptionUpdate::RollOver,
        });

        // 3. Clear
        handle.on_host_output(HostOutputEvent::Clear);
    }
}
