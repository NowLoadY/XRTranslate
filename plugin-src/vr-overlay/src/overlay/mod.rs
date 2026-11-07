//! SteamVR overlay runtime and page.
//!
//! Provides private, low-latency, HMD-locked bilingual subtitles rendered
//! directly inside VR using the official SteamVR (OpenVR) Compositor Overlay API.

mod avatar;
mod graphics;
mod input;
mod interaction;
mod openvr;
#[cfg(debug_assertions)]
mod preview;
#[cfg(debug_assertions)]
pub(crate) use preview::render as render_avatar_preview;
mod renderer;
pub mod runtime;
mod space;
mod spatial;
pub mod ui;
mod voice_props;

pub use runtime::{VrOverlayHandle, VrOverlayManager, VrOverlaySettings};
pub use ui::{VrOverlayPageContext, VrOverlayUiAction};

use crate::session_coordinator::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};
use std::path::PathBuf;

impl HostOutputSubscriber for VrOverlayHandle {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::Previews {
                stream_id,
                is_typing,
                entries,
            } => self.set_previews(stream_id, is_typing, &entries),
            HostOutputEvent::CommittedTranslation { .. } => {}
            HostOutputEvent::StreamCancelled(stream_id) => self.cancel_stream(stream_id),
            // Modern hosts already publish ASR partials through the preview set.
            HostOutputEvent::Caption {
                turn_id,
                translated,
                segment_index: 0,
                update: CaptionUpdate::Replace,
                ..
            } if !turn_id.is_empty() && translated.is_empty() => {}
            HostOutputEvent::Caption {
                stream_id,
                turn_id,
                segment_index,
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
                runtime::caption_id(stream_id, &turn_id, segment_index),
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { &source },
                &translated,
                &additional_translations,
                &speaker,
            ),
            HostOutputEvent::Caption {
                stream_id,
                turn_id,
                segment_index,
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
                runtime::caption_id(stream_id, &turn_id, segment_index),
                stream_id,
                audio_source,
                is_typing,
                if asr_only { "" } else { &source },
                &translated,
                &additional_translations,
                &speaker,
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
}

impl VrOverlayPlugin {
    pub fn new(mut draft: VrOverlaySettings, input_directory: PathBuf) -> Self {
        draft.normalize();
        let manager = VrOverlayManager::new(draft.clone(), input_directory);
        Self { manager, draft }
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

    pub fn sync_settings(&mut self) {
        self.draft.normalize();
        self.manager.update_settings(self.draft.clone());
    }

    pub fn connect(&self) {
        self.manager.connect();
    }

    pub fn disconnect(&self) {
        self.manager.disconnect();
    }
}
