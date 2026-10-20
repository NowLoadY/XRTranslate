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
        enabled: bool,
    ) {
        use crate::i18n::{tr, tr_dynamic};
        use xrt_connector_core::Status;
        let status_value = self.snapshot().status;
        let status = match status_value {
            _ if !enabled => "Disabled",
            Status::Unavailable if self.enabled_for_reset() => "Syncing controls…",
            Status::Unavailable => "Not connected",
            Status::Off => "Ready",
            Status::Starting => "Starting…",
            Status::Active => "Translating",
            Status::Stopping => "Stopping…",
            Status::Error => "Translation failed",
        };
        ui.label(tr(language, status));
        if enabled
            && let Some(message) = self.diagnostic()
        {
            let message = match message {
                "OSC receive configuration changed" => {
                    "Enable OSC, then use your avatar's XRT control."
                }
                message => message,
            };
            ui.label(tr_dynamic(language, message));
        }
        if enabled
            && self.settings.connector.osc().is_some()
            && self.settings.captions
            && matches!(
                status_value,
                Status::Off | Status::Starting | Status::Active
            )
            && !self.output.available()
        {
            ui.weak(tr(language, "No caption object found on this Avatar."));
        }
        ui.horizontal_wrapped(|ui| {
            if enabled
                && self.settings.connector.osc().is_none()
                && let Some(runtime) = &self.websocket
            {
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
            ui.hyperlink_to(
                tr(language, "Setup guide"),
                format!(
                    "https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/{}/installation.md",
                    self.settings.connector.id()
                ),
            );
        });
        if enabled && self.settings.connector.osc().is_some() && self.diagnostic().is_none() {
            let hint = match status_value {
                Status::Unavailable if !self.enabled_for_reset() => {
                    Some("Enable OSC, then use your avatar's XRT control.")
                }
                Status::Off => Some("Start translation from your game controls."),
                _ => None,
            };
            if let Some(hint) = hint {
                ui.weak(tr(language, hint));
            }
        }
    }
    fn enabled_for_reset(&self) -> bool {
        self.snapshot().pending_reset.is_some() && self.diagnostic().is_none()
    }
}
