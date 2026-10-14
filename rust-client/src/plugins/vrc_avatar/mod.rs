//! Avatar intent adapter. Transport and translation resources retain their host owners.

mod control;

pub(crate) use control::{HostAction, VrcAvatarPlugin};
pub(crate) use xrt_avatar_control::{HostResult, OperationId};

use crate::session_coordinator::{PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy};

pub(crate) fn session_binding(operation: OperationId) -> PluginSessionBinding {
    PluginSessionBinding {
        owner: PluginSessionOwner::new(
            super::PluginId::VRC_AVATAR.as_str(),
            format!(
                "{}:{}",
                operation.generation().value(),
                operation.sequence()
            ),
            "Avatar translation",
            "Open Avatar controls",
            "Translation controlled by your Avatar",
        ),
        output_policy: SessionOutputPolicy::Host,
        host_tts: true,
        external_audio_gate: true,
        finish_when_audio_ends: false,
    }
}

impl VrcAvatarPlugin {
    pub(crate) fn render_settings(
        &self,
        ui: &mut eframe::egui::Ui,
        language: crate::i18n::UiLanguage,
    ) {
        use crate::i18n::{tr, tr_dynamic};
        use xrt_avatar_control::Status;

        let snapshot = self.snapshot();
        let status = match snapshot.status {
            Status::Unavailable => "Avatar control unavailable",
            Status::Off if !snapshot.reset_confirmed => "Synchronizing Avatar controls",
            Status::Off => "Avatar translation is off",
            Status::Starting => "Starting Avatar translation",
            Status::Active => "Avatar translation is running",
            Status::Stopping => "Stopping Avatar translation",
            Status::Error => "Avatar translation could not continue",
        };
        ui.label(tr(language, status));
        if let Some(message) = self.diagnostic() {
            ui.label(tr_dynamic(language, message));
        }
        ui.label(tr(language, "Uses your selected languages, audio input and output settings. Enable OSC in VRChat, then use XRT > Translation on your Avatar."));
        ui.hyperlink_to(
            tr(language, "Avatar package and setup"),
            "https://github.com/NowLoadY/XRT-Avatar-Plugin",
        );
    }
}
