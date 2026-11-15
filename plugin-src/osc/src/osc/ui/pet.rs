//! The pet's mood lives inside the banner definition dropdown.
use super::super::pet::{PetMood, PetSettings};
use crate::{
    i18n::{UiLanguage, tr},
    ui::components,
};
use eframe::egui;

pub(super) fn render(ui: &mut egui::Ui, settings: &mut PetSettings, language: UiLanguage) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(tr(language, "Mood:"));
        for mood in [
            PetMood::Natural,
            PetMood::Happy,
            PetMood::Shy,
            PetMood::Sleepy,
        ] {
            changed |= components::selectable_value(
                ui,
                &mut settings.mood,
                mood,
                tr(language, mood.label()),
            )
            .changed();
        }
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr(
                language,
                "Plays out little stories in the empty message area and makes room when you speak.",
            ))
            .small(),
        )
        .wrap(),
    );
    changed
}
