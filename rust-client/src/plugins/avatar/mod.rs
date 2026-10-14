//! Avatar intent adapter. Transport and translation resources retain their host owners.

pub(crate) mod connectors;
mod control;
mod output;
mod websocket;

pub(crate) use control::{AvatarPlugin, HostAction};
pub(crate) use xrt_connector_core::{HostResult, OperationId};

use crate::session_coordinator::{PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy};

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
            "Open Avatar controls",
            "Translation controlled by your Avatar",
        ),
        output_policy: SessionOutputPolicy::Host,
        host_tts: true,
        external_audio_gate: true,
        finish_when_audio_ends: false,
    }
}

impl AvatarPlugin {
    pub(crate) fn render_settings(
        &mut self,
        ui: &mut eframe::egui::Ui,
        language: crate::i18n::UiLanguage,
    ) -> bool {
        use crate::i18n::{tr, tr_dynamic};
        use xrt_connector_core::Status;
        let before = self.settings.clone();
        eframe::egui::ComboBox::from_label(tr(language, "Game connector"))
            .selected_text(self.settings.connector.label())
            .show_ui(ui, |ui| {
                for connector in connectors::Connector::ALL {
                    ui.selectable_value(&mut self.settings.connector, connector, connector.label());
                }
            });
        let snapshot = self.snapshot();
        let status = match snapshot.status {
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
        ui.label(tr(
            language,
            "Uses your selected languages, audio input and output settings.",
        ));
        if self.settings.connector.osc().is_none() {
            ui.checkbox(
                &mut self.settings.captions,
                tr(language, "Send captions to the game component"),
            );
            if let Some(runtime) = &self.websocket {
                ui.horizontal(|ui| {
                    ui.label(&runtime.url);
                    if ui.button(tr(language, "Copy address")).clicked() {
                        ui.ctx().copy_text(runtime.url.clone());
                    }
                    if ui.button(tr(language, "Copy pairing token")).clicked() {
                        ui.ctx().copy_text(runtime.token.clone());
                    }
                });
            }
        } else {
            ui.label(tr(
                language,
                "Enable OSC in the game, then use the XRT control on your Avatar.",
            ));
        }
        ui.hyperlink_to(
            tr(language, "Avatar package and setup"),
            format!(
                "https://github.com/NowLoadY/XRT-Avatar-Plugin/tree/main/docs/adapters/{}",
                self.settings.connector.id()
            ),
        );
        before != self.settings
    }
    fn enabled_for_reset(&self) -> bool {
        self.snapshot().pending_reset.is_some() && self.diagnostic().is_none()
    }
}
