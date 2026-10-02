use crate::ui::components::card;
use eframe::egui;

pub fn render_canvas(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) {
    card(ui, |ui| {
        ui.set_min_height(140.0);

        let preview = plugin.manager().chatbox_preview();
        let is_empty = preview.text.trim().is_empty();
        let char_count = preview.text.chars().count();
        let limit = plugin.draft().max_text_length;

        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            let text_color = if char_count > limit {
                egui::Color32::from_rgb(220, 38, 38)
            } else {
                crate::ui::theme::text_weak()
            };
            let lifecycle = if preview.typing {
                Some(crate::i18n::tr(language, "Live").to_owned())
            } else {
                preview
                    .next_message_expires_in
                    .map(|remaining| format!("{:.1}s", remaining.as_secs_f64()))
            };
            let status = lifecycle.map_or_else(
                || format!("{char_count}/{limit}"),
                |lifecycle| format!("{char_count}/{limit} · {lifecycle}"),
            );

            ui.label(
                egui::RichText::new(status)
                    .font(egui::FontId::monospace(11.5))
                    .color(text_color)
                    .strong(),
            );
        });

        ui.add_space(8.0);

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 90.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.add_space(6.0);

                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(15, 23, 42))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 65, 85)))
                    .corner_radius(egui::CornerRadius::same(16))
                    .inner_margin(egui::Margin::symmetric(20, 14))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(380.0));
                        if is_empty {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "Empty"))
                                    .family(egui::FontFamily::Monospace)
                                    .color(egui::Color32::from_rgb(100, 116, 139))
                                    .size(13.0)
                                    .italics(),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&preview.text)
                                        .family(egui::FontFamily::Monospace)
                                        .color(egui::Color32::from_rgb(241, 245, 249))
                                        .size(13.5),
                                )
                                .wrap(),
                            );
                        }
                    });
            },
        );

        ui.add_space(6.0);
    });
}

pub fn render_bottom_input_bar(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    capabilities: xrtranslate_engine::language::LanguageCapabilities,
    preparing_text: bool,
    actions: &mut Vec<super::OscUiAction>,
) {
    use crate::ui::components::text_composer::{ComposerContext, TextAction};
    let before = (
        plugin.draft.typing_source_lang.clone(),
        plugin.draft.typing_target_lang.clone(),
    );
    let action = plugin.composer.render(
        ui,
        "osc_text_composer",
        (
            &mut plugin.draft.typing_source_lang,
            &mut plugin.draft.typing_target_lang,
        ),
        ComposerContext {
            language,
            capabilities,
            preparing: preparing_text,
            allow_direct: true,
            direct_enabled: plugin.draft.enabled,
            show_languages: true,
        },
    );
    if before
        != (
            plugin.draft.typing_source_lang.clone(),
            plugin.draft.typing_target_lang.clone(),
        )
    {
        actions.push(super::OscUiAction::SaveSettings);
    }
    match action {
        Some(TextAction::Translate {
            text,
            source_lang,
            target_lang,
        }) => {
            actions.push(super::OscUiAction::TranslateInput {
                text,
                source_lang,
                target_lang,
            });
        }
        Some(TextAction::Direct(text)) => actions.push(super::OscUiAction::DirectInput(text)),
        None => {}
    }
}
