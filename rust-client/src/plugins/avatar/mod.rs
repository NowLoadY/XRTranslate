//! Avatar intent adapter. Transport and translation resources retain their host owners.

pub(crate) mod connectors;
mod control;
pub(crate) mod display;
mod osc_caption;
mod output;
mod ui;
mod websocket;

pub(crate) use control::{AvatarPlugin, HostAction};
pub(crate) use xrt_connector_core::{HostResult, OperationId};

use crate::session_coordinator::{PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy};

pub(crate) fn translation_context(operation: OperationId) -> xrtranslate_prompt::TaskContext {
    xrtranslate_prompt::TaskContext::conversation(format!(
        "avatar:{}:{}",
        operation.generation().value(),
        operation.sequence()
    ))
}

pub(crate) fn session_binding(operation: OperationId) -> PluginSessionBinding {
    PluginSessionBinding {
        owner: PluginSessionOwner::new(
            super::PluginId::AVATAR.as_str(),
            format!(
                "{}:{}",
                operation.generation().value(),
                operation.sequence()
            ),
            "Avatar translation",
            "Open Game Plugins",
            "Translation controlled by your Avatar",
        ),
        output_policy: SessionOutputPolicy::Host,
        host_tts: true,
        external_audio_gate: true,
        finish_when_audio_ends: false,
    }
}

impl AvatarPlugin {
    pub(crate) fn render_connection(
        &self,
        ui: &mut eframe::egui::Ui,
        language: crate::i18n::UiLanguage,
    ) {
        use crate::i18n::{tr, tr_dynamic};
        use xrt_connector_core::Status;
        let status_value = self.snapshot().status;
        let status = match status_value {
            Status::Unavailable if self.enabled_for_reset() => "Synchronizing Avatar controls",
            Status::Unavailable => "Avatar control unavailable",
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
        if self.settings.connector.osc().is_some()
            && self.settings.captions
            && matches!(
                status_value,
                Status::Off | Status::Starting | Status::Active
            )
            && !self.output.available()
        {
            ui.weak(tr(language, "No caption object found on this Avatar."));
        }
        if self.settings.connector.osc().is_none() {
            if let Some(runtime) = &self.websocket {
                if ui.button(tr(language, "Copy connection info")).clicked() {
                    ui.ctx().copy_text(
                        serde_json::json!({
                            "protocol": 1,
                            "connector": self.settings.connector.id(),
                            "address": runtime.url,
                            "token": runtime.token,
                        })
                        .to_string(),
                    );
                }
            }
        } else {
            ui.label(tr(
                language,
                "Enable OSC in the game, then use the XRT control on your Avatar.",
            ));
        }
        if self.settings.connector.id() == "vrchat" {
            ui.hyperlink_to(tr(language, "Add to VCC / ALCOM"),
                "vcc://vpm/addRepo?url=https%3A%2F%2Fraw.githubusercontent.com%2FNowLoadY%2FXRT-Avatar-Plugin%2Fmain%2Fvpm%2Findex.json");
        }
        ui.hyperlink_to(
            tr(language, "Avatar package and setup"),
            format!(
                "https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/{}/installation.md",
                self.settings.connector.id()
            ),
        );
    }
    fn enabled_for_reset(&self) -> bool {
        self.snapshot().pending_reset.is_some() && self.diagnostic().is_none()
    }
}
