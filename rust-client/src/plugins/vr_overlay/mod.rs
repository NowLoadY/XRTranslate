//! Built-in SteamVR In-Game Overlay plugin for XRTranslate.
//!
//! Provides private, low-latency, HMD-locked bilingual subtitles rendered
//! directly inside VR using the official SteamVR (OpenVR) Compositor Overlay API.

mod openvr;
mod renderer;
pub mod runtime;
pub mod ui;

pub use runtime::{VrOverlayHandle, VrOverlayManager, VrOverlaySettings};
pub use ui::{VrOverlayPageContext, VrOverlayUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};

impl HostOutputSubscriber for VrOverlayHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::Caption {
                stream_id,
                source,
                translated,
                speaker,
                update: CaptionUpdate::RollOver,
                ..
            } => self.roll_stream(stream_id, source, translated, speaker),
            HostOutputEvent::Caption {
                stream_id,
                source,
                translated,
                speaker,
                is_typing,
                ..
            } => self.add_caption(stream_id, source, translated, speaker, is_typing),
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
    pub fn new(draft: VrOverlaySettings, host_enabled: bool) -> Self {
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
        let mut effective = self.draft.clone();
        if !self.host_enabled {
            effective.enabled = false;
        }
        self.manager.update_settings(effective);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_settings::CaptureSource;

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
            speaker: "Speaker 1".into(),
            update: CaptionUpdate::RollOver,
        });

        // 3. Clear
        handle.on_host_output(HostOutputEvent::Clear);
    }
}
