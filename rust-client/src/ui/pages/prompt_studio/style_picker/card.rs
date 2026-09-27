use super::StyleCard;
use crate::{
    i18n::{UiLanguage, tr, tr_dynamic},
    ui::components::{
        faded_scroll_text,
        selection_card::{self, SelectionCard},
    },
};
use eframe::egui::{self, Color32, FontId};

pub(super) fn render(
    ui: &mut egui::Ui,
    style: &StyleCard,
    width: f32,
    selected: bool,
    enabled: bool,
    language: UiLanguage,
) -> bool {
    selection_card::show(
        ui,
        SelectionCard {
            id: egui::Id::new((&style.selection, "style_card")),
            title: &tr_dynamic(language, &style.name),
            avatar: &style.name,
            width,
            height: 336.0,
            selected,
            enabled,
            action: tr(language, if selected { "In use" } else { "Choose style" }),
        },
        |ui| {
            faded_scroll_text::show(
                ui,
                &style.text,
                FontId::proportional(13.0),
                Color32::from_gray(97),
            );
        },
    )
}
