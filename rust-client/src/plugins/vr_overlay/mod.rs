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
