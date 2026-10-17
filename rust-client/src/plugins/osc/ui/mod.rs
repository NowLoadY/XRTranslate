pub mod canvas;
mod settings;
pub mod toolbar;

use eframe::egui;

#[derive(Clone, Copy)]
pub struct OscPageContext<'a> {
    pub language: crate::i18n::UiLanguage,
    pub preparing_text: bool,
    pub microphone_sync_enabled: bool,
    pub lyrics_available: bool,
    pub languages: xrtranslate_engine::language::LanguageCapabilities,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
    pub additional_target_lang: Option<&'a str>,
    pub asr_only: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OscUiAction {
    ClearHostHistory,
    SetMicrophoneSyncEnabled(bool),
    SetSpeakerNumberVisible(bool),
    ApplySettings,
    SetOutputEnabled(bool),
    DirectInput(String),
    SetLanguageRoute {
        source_lang: String,
        target_lang: String,
        asr_only: bool,
    },
    TranslateInput {
        text: String,
    },
}

pub fn render(
    plugin: &mut super::OscPlugin,
    ui: &mut egui::Ui,
    context: OscPageContext<'_>,
) -> Vec<OscUiAction> {
    ui.push_id(plugin.game(), |ui| render_page(plugin, ui, context))
        .inner
}

fn render_page(
    plugin: &mut super::OscPlugin,
    ui: &mut egui::Ui,
    context: OscPageContext<'_>,
) -> Vec<OscUiAction> {
    let mut actions = Vec::new();
    egui::Panel::bottom(ui.make_persistent_id("osc_bottom_input_bar"))
        .show_separator_line(false)
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            ui.add_space(10.0);
            canvas::render_bottom_input_bar(plugin, ui, context, &mut actions);
        });

    egui::ScrollArea::vertical()
        .id_salt("osc_page_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    context.language,
                    plugin.game().studio_title(),
                ))
                .size(22.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
            );
            ui.label(crate::i18n::tr(
                context.language,
                match plugin.game() {
                    super::OscGame::VRChat => {
                        "In VRChat's Action Menu, turn on OSC → Enabled."
                    }
                    super::OscGame::ChilloutVR => {
                        "Enable OSC in Settings → Implementation → OSC."
                    }
                },
            ));

            ui.add_space(12.0);

            let is_wide = (ui.available_width() > ui.available_height()
                && ui.available_width() >= 540.0)
                || ui.available_width() >= 800.0;

            if is_wide {
                let total_width = ui.available_width();
                let left_width = (total_width * 0.46).clamp(300.0, 440.0);
                let right_width = total_width - left_width - ui.spacing().item_spacing.x;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(left_width, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            toolbar::render_toolbar(
                                plugin,
                                ui,
                                context.language,
                                context.microphone_sync_enabled,
                                context.lyrics_available,
                                &mut actions,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(right_width, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            canvas::render_canvas(plugin, ui, context.language);
                        },
                    );
                });
            } else {
                toolbar::render_toolbar(
                    plugin,
                    ui,
                    context.language,
                    context.microphone_sync_enabled,
                    context.lyrics_available,
                    &mut actions,
                );

                ui.add_space(12.0);

                canvas::render_canvas(plugin, ui, context.language);
            }
            ui.add_space(8.0);
        });

    actions
}

pub fn render_settings(
    plugin: &mut super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> Vec<OscUiAction> {
    ui.push_id(plugin.game(), |ui| settings::render(plugin, ui, language))
        .inner
}
