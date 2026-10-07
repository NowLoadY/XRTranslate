use crate::i18n::{UiLanguage, tr};
use crate::session_coordinator::AudioSourceFilter;
use crate::ui::theme;
use eframe::egui;

/// Shared output controls; callers own applying and saving their plugin settings.
pub fn audio_source_filter(
    ui: &mut egui::Ui,
    filter: &mut AudioSourceFilter,
    language: UiLanguage,
) -> bool {
    let before = *filter;
    ui.push_id("audio_source_filter", |ui| {
        ui.label(
            egui::RichText::new(tr(language, "Show audio translations"))
                .color(theme::text_strong())
                .strong(),
        )
        .on_hover_text(tr(
            language,
            "Only affects this plugin. Text translations are always shown.",
        ));
        ui.horizontal_wrapped(|ui| {
            for (label, enabled) in [
                ("Microphone", &mut filter.microphone),
                ("System Audio", &mut filter.system_audio),
            ] {
                let label = tr(language, label);
                let width = ui
                    .painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        egui::FontId::proportional(13.0),
                        theme::text_strong(),
                    )
                    .size()
                    .x
                    + 40.0
                    + ui.spacing().item_spacing.x * 2.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| super::toggle_with_label(ui, enabled, label),
                );
            }
        });
        if !filter.microphone && !filter.system_audio {
            ui.label(
                egui::RichText::new(tr(language, "Audio translations hidden"))
                    .small()
                    .color(theme::text_weak()),
            );
        }
    });
    *filter != before
}
