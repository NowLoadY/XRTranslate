//! Settings for the Game Plugins group and its active game integration.
use super::avatar::connectors::Connector;
use crate::i18n::{UiLanguage, tr};
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub enabled: bool,
}

impl Settings {
    pub(crate) fn active(self) -> bool {
        self.enabled && super::PluginId::AVATAR.is_supported()
    }
}

pub(crate) fn render_selection(
    ui: &mut egui::Ui,
    connector: &mut Connector,
    language: UiLanguage,
) -> bool {
    let before = *connector;
    crate::ui::components::combobox_ui(ui, "game_plugin_selection", connector.label(), |ui| {
        for game in Connector::ALL {
            crate::ui::components::selectable_value(ui, connector, game, game.label());
        }
    })
    .response
    .on_hover_text(tr(language, "Game"));
    before != *connector
}
