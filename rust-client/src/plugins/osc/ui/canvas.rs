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
    let is_enabled = plugin.draft().enabled && !(plugin.translate_input() && preparing_text);
    let mut submit = false;
    let has_text = !plugin.draft_input().trim().is_empty();
    let translate_mode = plugin.translate_input();

    let container_id = ui.make_persistent_id("osc_bottom_chatbox_container");

    let translation_languages = capabilities.targets().options();

    let input_border_color = crate::ui::theme::border();
    let compact = ui.available_width() < 600.0;
    let radius = if compact { 22 } else { 10 };
    crate::ui::organic_border::show(
        ui,
        container_id.with("organic_border"),
        egui::Frame::new()
            .fill(if compact {
                crate::ui::theme::surface_control()
            } else {
                egui::Color32::TRANSPARENT
            })
            .corner_radius(egui::CornerRadius::same(radius))
            .inner_margin(egui::Margin::symmetric(12, 6)),
        radius as f32,
        input_border_color,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mode_text = if translate_mode {
                    egui::RichText::new(crate::i18n::tr(language, "Translate"))
                        .color(egui::Color32::from_rgb(37, 99, 235))
                        .size(12.0)
                        .strong()
                } else {
                    egui::RichText::new(crate::i18n::tr(language, "Direct"))
                        .color(crate::ui::theme::text_weak())
                        .size(12.0)
                };
                let mode_btn = ui
                    .add_enabled(is_enabled, egui::Button::new(mode_text).frame(false))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if mode_btn.clicked() {
                    plugin.set_translate_input(!translate_mode);
                }

                if translate_mode {
                    ui.allocate_ui_with_layout(
                        egui::vec2(232.0, ui.spacing().interact_size.y),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let source_label =
                                crate::language_label(language, &plugin.draft().typing_source_lang);
                            let source_options: Vec<(String, String)> = translation_languages
                                .iter()
                                .map(|(code, label)| {
                                    (
                                        (*code).to_string(),
                                        crate::i18n::tr(language, label).to_string(),
                                    )
                                })
                                .collect();
                            crate::ui::components::searchable_combobox_frameless(
                                ui,
                                container_id.with("typing_source_lang"),
                                source_label,
                                &mut plugin.draft_mut().typing_source_lang,
                                &source_options,
                                Some(96.0),
                            );

                            let swap_btn = ui
                                .add_enabled(
                                    is_enabled,
                                    egui::Button::new(
                                        egui::RichText::new("↔")
                                            .size(12.0)
                                            .color(crate::ui::theme::text_weak())
                                            .strong(),
                                    )
                                    .frame(false)
                                    .min_size(egui::vec2(32.0, ui.spacing().interact_size.y)),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if swap_btn.clicked() {
                                let draft = plugin.draft_mut();
                                std::mem::swap(
                                    &mut draft.typing_source_lang,
                                    &mut draft.typing_target_lang,
                                );
                            }

                            let target_label =
                                crate::language_label(language, &plugin.draft().typing_target_lang);
                            let target_options: Vec<(String, String)> = source_options
                                .into_iter()
                                .filter(|(code, _)| {
                                    !crate::languages_conflict(
                                        code,
                                        &plugin.draft().typing_source_lang,
                                    )
                                })
                                .collect();
                            crate::ui::components::searchable_combobox_frameless(
                                ui,
                                container_id.with("typing_target_lang"),
                                target_label,
                                &mut plugin.draft_mut().typing_target_lang,
                                &target_options,
                                Some(96.0),
                            );
                        },
                    );
                }
            });

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = is_enabled && has_text;
                    let send_btn = ui
                        .add_enabled(
                            can_send,
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::tr(language, "Send"))
                                    .color(if can_send {
                                        egui::Color32::WHITE
                                    } else {
                                        crate::ui::theme::text_weak()
                                    })
                                    .size(12.0)
                                    .strong(),
                            )
                            .fill(if can_send {
                                egui::Color32::from_rgb(37, 99, 235)
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .frame(can_send)
                            .corner_radius(egui::CornerRadius::same(6))
                            .min_size(egui::vec2(52.0, ui.spacing().interact_size.y)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    submit |= send_btn.clicked();
                    if translate_mode && preparing_text {
                        ui.spinner();
                    }

                    let hint_text = if !is_enabled {
                        crate::i18n::tr(language, "OSC is disabled")
                    } else if translate_mode {
                        crate::i18n::tr(
                            language,
                            "Type a message to Translate & Send (Press Enter)...",
                        )
                    } else {
                        crate::i18n::tr(
                            language,
                            "Type a message to Chatbox (Press Enter to send)...",
                        )
                    };

                    let text_frame = egui::Frame::new()
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::symmetric(0, 2));

                    let edit_resp = ui.add_enabled(
                        is_enabled,
                        egui::TextEdit::singleline(plugin.draft_input_mut())
                            .id_salt("osc_bottom_chatbox_input")
                            .hint_text(
                                egui::RichText::new(hint_text)
                                    .size(13.0)
                                    .color(crate::ui::theme::text_weak()),
                            )
                            .text_color(crate::ui::theme::text_strong())
                            .font(egui::FontId::proportional(13.5))
                            .frame(text_frame)
                            .margin(egui::Margin::symmetric(0, 2))
                            .min_size(egui::vec2(0.0, ui.spacing().interact_size.y))
                            .desired_width(ui.available_width()),
                    );

                    if edit_resp.changed() && translate_mode {
                        let text = plugin.draft_input();
                        let current_src = &plugin.draft().typing_source_lang;
                        let current_tgt = &plugin.draft().typing_target_lang;
                        if let Some((new_src, new_tgt)) =
                            xrtranslate_engine::auto_route_language_pair(
                                text,
                                current_src,
                                current_tgt,
                            )
                        {
                            if (new_src != current_src || new_tgt != current_tgt)
                                && capabilities.select(new_src, new_tgt).is_ok()
                            {
                                plugin.draft_mut().typing_source_lang = new_src.to_string();
                                plugin.draft_mut().typing_target_lang = new_tgt.to_string();
                            }
                        }
                    }

                    if edit_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if has_text {
                            submit = true;
                        }
                        edit_resp.request_focus();
                    }
                });
            });
        },
    );

    if submit && is_enabled && has_text {
        ui.memory_mut(|memory| memory.interrupt_ime());
        let text = plugin.draft_input().trim().to_string();
        if plugin.translate_input() {
            let current_src = &plugin.draft().typing_source_lang;
            let current_tgt = &plugin.draft().typing_target_lang;
            let (final_src, final_tgt) = if let Some((new_src, new_tgt)) =
                xrtranslate_engine::auto_route_language_pair(&text, current_src, current_tgt)
            {
                (new_src.to_string(), new_tgt.to_string())
            } else {
                (current_src.clone(), current_tgt.clone())
            };
            plugin.draft_mut().typing_source_lang = final_src.clone();
            plugin.draft_mut().typing_target_lang = final_tgt.clone();

            actions.push(super::OscUiAction::TranslateInput {
                text,
                source_lang: final_src,
                target_lang: final_tgt,
            });
        } else {
            plugin.send_manual_message(&text);
            plugin.draft_input_mut().clear();
        }
        ui.ctx().request_repaint();
    }
}
