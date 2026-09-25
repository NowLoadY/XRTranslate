pub mod canvas;
mod settings;
pub mod toolbar;

use eframe::egui;

#[derive(Clone, Copy)]
pub struct OscPageContext<'a> {
    pub language: crate::i18n::UiLanguage,
    pub last_error: Option<&'a str>,
    pub mute_gate_enabled: bool,
    pub translation_languages: &'a [(&'static str, &'static str)],
}

#[derive(Debug, PartialEq, Eq)]
pub enum OscUiAction {
    ClearHostHistory,
    SetMuteGateEnabled(bool),
    SetSpeakerNumberVisible(bool),
    SaveSettings,
    SettingsApplied(Result<(), String>),
    TranslateInput {
        text: String,
        source_lang: String,
        target_lang: String,
    },
}

pub fn render(
    plugin: &mut super::OscPlugin,
    ui: &mut egui::Ui,
    context: OscPageContext<'_>,
) -> Vec<OscUiAction> {
    let mut actions = Vec::new();

    let bottom_bar_height = 70.0;
    let scroll_height = (ui.available_height() - bottom_bar_height - 10.0).max(60.0);

    egui::ScrollArea::vertical()
        .id_salt("osc_page_scroll")
        .auto_shrink([false, false])
        .max_height(scroll_height)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr(context.language, "VRChat OSC Studio"))
                    .size(22.0)
                    .color(crate::ui::theme::text_strong())
                    .strong(),
            );

            ui.add_space(12.0);

            if let Some(error) = context.last_error {
                crate::ui::components::error_notice(ui, context.language, error);
                ui.add_space(10.0);
            }

            toolbar::render_toolbar(
                plugin,
                ui,
                context.language,
                context.mute_gate_enabled,
                &mut actions,
            );

            ui.add_space(12.0);

            canvas::render_canvas(plugin, ui, context.language);
            ui.add_space(8.0);
        });

    ui.add_space(10.0);

    canvas::render_bottom_input_bar(plugin, ui, context.language, context.translation_languages, &mut actions);

    actions
}

pub fn render_settings(
    plugin: &mut super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> Vec<OscUiAction> {
    settings::render(plugin, ui, language)
}
