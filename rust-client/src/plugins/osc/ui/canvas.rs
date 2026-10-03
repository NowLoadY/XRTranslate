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
        crate::ui::automation::record_output(
            ui,
            "OSC preview",
            crate::ui::automation::ElementValue::Text(preview.text.clone()),
            ui.max_rect(),
        );
        crate::ui::automation::record_output(
            ui,
            "OSC typing",
            crate::ui::automation::ElementValue::Bool(preview.typing),
            ui.max_rect(),
        );
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
    context: super::OscPageContext<'_>,
    actions: &mut Vec<super::OscUiAction>,
) {
    use crate::ui::components::text_composer::{ComposerContext, TextAction, TextMode};
    let mut source = context.source_lang.to_owned();
    let mut target = context.target_lang.to_owned();
    let mut asr_only = context.asr_only;
    if plugin.composer.mode == TextMode::Translate
        && crate::ui::components::translation_primary_language_selector(
            ui,
            "osc_primary_languages",
            &mut source,
            &mut target,
            context.additional_target_lang,
            &mut asr_only,
            context.languages,
            context.language,
        )
    {
        actions.push(super::OscUiAction::SetLanguageRoute {
            source_lang: source.clone(),
            target_lang: target.clone(),
            asr_only,
        });
    }
    let action = plugin.composer.render(
        ui,
        "osc_text_composer",
        (&mut source, &mut target),
        ComposerContext {
            language: context.language,
            capabilities: context.languages,
            preparing: context.preparing_text,
            allow_direct: true,
            direct_enabled: plugin.draft.enabled,
            show_languages: false,
        },
    );
    match action {
        Some(TextAction::Translate { text, .. }) => {
            // Route changes precede submission in the same frame. The host is
            // the source of truth for both this composer and Translation.
            actions.push(super::OscUiAction::TranslateInput { text });
        }
        Some(TextAction::Direct(text)) => actions.push(super::OscUiAction::DirectInput(text)),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::osc::{OscPageContext, OscPlugin, OscUiAction, runtime::OscSettings};

    #[test]
    fn shared_language_edits_precede_submission_without_changing_legacy_settings() {
        let mut plugin = OscPlugin::new(
            OscSettings {
                listen_port: 0,
                ..Default::default()
            },
            true,
        );
        *plugin.draft_input_mut() = "hello".into();
        let driver = crate::ui::automation::driver();
        {
            let mut state = driver.frame_state.lock().unwrap();
            *state = Default::default();
            state.pending_set = Some((
                "Japanese".into(),
                crate::ui::automation::ElementValue::Text("Input only (ASR)".into()),
            ));
            state.pending_click = Some("Send".into());
        }
        let mut actions = Vec::new();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_bottom_input_bar(
                    &mut plugin,
                    ui,
                    OscPageContext {
                        language: crate::i18n::UiLanguage::English,
                        preparing_text: false,
                        mute_gate_enabled: false,
                        languages: Default::default(),
                        source_lang: "ja",
                        target_lang: "ko",
                        additional_target_lang: Some("fr"),
                        asr_only: false,
                    },
                    &mut actions,
                );
            });
        });
        output.textures_delta.clear();
        assert_eq!(
            actions,
            vec![
                OscUiAction::SetLanguageRoute {
                    source_lang: "ja".into(),
                    target_lang: "ko".into(),
                    asr_only: true
                },
                OscUiAction::TranslateInput {
                    text: "hello".into()
                },
            ]
        );
        assert_eq!(plugin.draft().typing_source_lang, "zh");
        assert_eq!(plugin.draft().typing_target_lang, "en");
        assert!(
            !driver
                .frame_state
                .lock()
                .unwrap()
                .elements
                .iter()
                .any(|element| element.label == "Fixed extra language")
        );
        *driver.frame_state.lock().unwrap() = Default::default();
    }
}
