//! Settings for the Game Plugins group and its active game integration.
use super::avatar::connectors::Connector;
use crate::i18n::{UiLanguage, tr};
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub enabled: bool,
    pub selection: Selection,
}

impl Settings {
    pub(crate) fn active(self, selection: Selection) -> bool {
        self.enabled && self.selection == selection && super::PluginId::AVATAR.is_supported()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Selection {
    #[default]
    Avatar,
    BigWalk,
}

impl Selection {
    pub(crate) fn render(
        &mut self,
        ui: &mut egui::Ui,
        connector: &mut Connector,
        language: UiLanguage,
    ) -> bool {
        let before = (*self, *connector);
        let label = match self {
            Self::Avatar => connector.label(),
            Self::BigWalk => "Big Walk",
        };
        egui::ComboBox::from_id_salt("game_plugin_selection")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for game in Connector::ALL {
                    if ui
                        .selectable_label(*self == Self::Avatar && *connector == game, game.label())
                        .clicked()
                    {
                        *self = Self::Avatar;
                        *connector = game;
                    }
                }
                if ui
                    .selectable_label(*self == Self::BigWalk, "Big Walk")
                    .clicked()
                {
                    *self = Self::BigWalk;
                }
            })
            .response
            .on_hover_text(tr(language, "Game"));
        before != (*self, *connector)
    }
}
