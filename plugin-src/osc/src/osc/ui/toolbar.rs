use super::super::runtime::{
    MAX_PREFIX_LENGTH, OscFormatMode, OscMessageSeparator, default_lyric_offset_seconds,
};
use crate::ui::components::{self, card};
use eframe::egui;

pub fn render_toolbar(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    microphone_sync_enabled: bool,
    lyrics_available: bool,
    actions: &mut Vec<super::OscUiAction>,
) {
    let mut changed = false;
    let compact = ui.available_width() < 600.0;

    card(ui, |ui| {
        components::feature_ui(
            ui,
            crate::feature_access::Feature::OscChatbox,
            language,
            |ui| {
                ui.vertical(|ui| {
                    let mut settings = ui
                        .horizontal(|ui| {
                            let mut enabled = plugin.applied().enabled;
                            if components::feature_checkbox(
                                ui,
                                crate::feature_access::Feature::OscChatbox,
                                language,
                                &mut enabled,
                                "OSC",
                            )
                            .changed()
                            {
                                actions.push(super::OscUiAction::SetOutputEnabled(enabled));
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let settings = components::responsive_settings_button(
                                    ui,
                                    "osc_format_settings",
                                    compact,
                                    language,
                                );
                                if components::animated_button(
                                    ui,
                                    crate::i18n::tr(language, "Clear"),
                                )
                                .clicked()
                                {
                                    plugin.clear_chatbox();
                                    actions.push(super::OscUiAction::ClearHostHistory);
                                }
                                settings
                            })
                            .inner
                        })
                        .inner;

                    ui.add_space(8.0);
                    changed |= components::audio_source_filter(
                        ui,
                        &mut plugin.draft_mut().audio_source_filter,
                        language,
                    );

                    settings.show_body_unindented(ui, |ui| {
                        ui.add_space(8.0);
                        if plugin.game() == super::super::OscGame::VRChat {
                            if let Some(receiver) = plugin.receiver.as_ref() {
                                let mut microphone_sync_enabled = microphone_sync_enabled;
                                if components::feature_checkbox(
                                    ui,
                                    crate::feature_access::Feature::MuteSync,
                                    language,
                                    &mut microphone_sync_enabled,
                                    crate::i18n::tr(language, "Sync game microphone"),
                                )
                                .on_hover_text(crate::i18n::tr(language, "Microphone controls work both ways. Enable Toggle Voice in VRChat. Muting in-game also returns the spatial microphone to its dock."))
                                .changed()
                                {
                                    actions.push(super::OscUiAction::SetMicrophoneSyncEnabled(microphone_sync_enabled));
                                }
                                if microphone_sync_enabled {
                                    ui.small(crate::i18n::tr(language, "Enable Toggle Voice in VRChat."));
                                    if let Some(status) = receiver.microphone.as_deref() {
                                        ui.small(crate::i18n::tr_dynamic(language, status));
                                    }
                                }
                                ui.add_space(8.0);
                                components::wavy_divider(ui, crate::ui::theme::text_strong());
                                ui.add_space(8.0);
                            }
                        }

                        ui.horizontal_wrapped(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(100.0, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(crate::i18n::tr(language, "Format:"))
                                            .color(crate::ui::theme::text_strong())
                                            .strong(),
                                    );
                                },
                            );

                            let format_resp = components::combobox_ui(
                                ui,
                                "osc_format_mode",
                                crate::i18n::tr(language, plugin.draft().format_mode.label_key()),
                                |ui| {
                                    let r1 = components::selectable_value(
                                        ui,
                                        &mut plugin.draft_mut().format_mode,
                                        OscFormatMode::BilingualSourceFirst,
                                        crate::i18n::tr(language, OscFormatMode::BilingualSourceFirst.label_key()),
                                    );
                                    let r2 = components::selectable_value(
                                        ui,
                                        &mut plugin.draft_mut().format_mode,
                                        OscFormatMode::BilingualTargetFirst,
                                        crate::i18n::tr(language, OscFormatMode::BilingualTargetFirst.label_key()),
                                    );
                                    let r3 = components::selectable_value(
                                        ui,
                                        &mut plugin.draft_mut().format_mode,
                                        OscFormatMode::Inline,
                                        crate::i18n::tr(language, OscFormatMode::Inline.label_key()),
                                    );
                                    let r4 = components::selectable_value(
                                        ui,
                                        &mut plugin.draft_mut().format_mode,
                                        OscFormatMode::TargetOnly,
                                        crate::i18n::tr(language, OscFormatMode::TargetOnly.label_key()),
                                    );
                                    r1.changed() || r2.changed() || r3.changed() || r4.changed()
                                },
                            );
                            if format_resp.inner.unwrap_or(false) {
                                changed = true;
                            }

                            ui.add_space(16.0);
                            let mut speaker_number_enabled = plugin.draft().show_speaker_number;
                            if components::feature_checkbox(
                                ui,
                                crate::feature_access::Feature::SpeakerNumbers,
                                language,
                                &mut speaker_number_enabled,
                                crate::i18n::tr(language, "Speaker numbers"),
                            )
                            .changed()
                            {
                                plugin.draft_mut().show_speaker_number = speaker_number_enabled;
                                actions.push(super::OscUiAction::SetSpeakerNumberVisible(
                                    speaker_number_enabled,
                                ));
                            }
                        });

                        ui.add_space(8.0);

                        let target_only = plugin.draft().format_mode == OscFormatMode::TargetOnly;
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(100.0, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(crate::i18n::tr(
                                            language,
                                            if target_only {
                                                "Between messages:"
                                            } else {
                                                "Message layout:"
                                            },
                                        ))
                                        .color(crate::ui::theme::text_strong())
                                        .strong(),
                                    );
                                },
                            );
                            let response = components::combobox_ui(
                                ui,
                                "osc_message_separator",
                                plugin
                                    .draft()
                                    .message_separator
                                    .layout_label(language, target_only),
                                |ui| {
                                    let mut selection_changed = false;
                                    for value in
                                        [OscMessageSeparator::NewLine, OscMessageSeparator::Space]
                                    {
                                        selection_changed |= components::selectable_value(
                                            ui,
                                            &mut plugin.draft_mut().message_separator,
                                            value,
                                            value.layout_label(language, target_only),
                                        )
                                        .changed();
                                    }
                                    selection_changed
                                },
                            );
                            if response.inner.unwrap_or(false) {
                                changed = true;
                            }
                        });
                        ui.add_space(8.0);

                        if lyrics_available && plugin.draft().lyric_budget().is_some()
                        {
                            ui.add_space(4.0);
                            let offset = plugin.draft().lyric_offset_seconds;
                            let mut slider = components::ModernSlider::new(
                                crate::i18n::tr(language, "Lyric timing offset:"),
                                &mut plugin.draft_mut().lyric_offset_seconds,
                                -1.0..=1.0,
                                default_lyric_offset_seconds(),
                            )
                            .step(0.1)
                            .precision(1)
                            .suffix(" s");
                            if offset != 0.0 {
                                let icon = if offset < 0.0 {
                                    xrtranslate_client_core::ui::icon!("rabbit.svg")
                                } else {
                                    xrtranslate_client_core::ui::icon!("turtle.svg")
                                };
                                slider = slider.value_badge(icon, format!("{:.1} s", offset.abs()));
                            }
                            changed |= slider.show(ui)
                            .on_hover_text(crate::i18n::tr(language, "Rabbit: earlier. Turtle: later. Only affects OSC lyrics."))
                            .changed();
                        }

                        ui.add_space(4.0);
                        let mut persistent_banners = plugin.draft().persistent_banners;
                        if components::feature_checkbox(
                            ui,
                            crate::feature_access::Feature::OscChatbox,
                            language,
                            &mut persistent_banners,
                            crate::i18n::tr(language, "Keep header and footer visible"),
                        )
                        .changed()
                        {
                            plugin.draft_mut().persistent_banners = persistent_banners;
                            changed = true;
                        }

                        ui.add_space(8.0);

                        ui.horizontal_wrapped(|ui| {
                            for (label, id, value, default) in [
                                ("Microphone prefix:", "osc_mic_prefix", 0, "🎤"),
                                ("System audio prefix:", "osc_sys_prefix", 1, "🔊"),
                                ("Typing prefix:", "osc_txt_prefix", 2, "💬"),
                            ] {
                                let label = crate::i18n::tr(language, label);
                                let font = egui::TextStyle::Body.resolve(ui.style());
                                let width = ui
                                    .painter()
                                    .layout_no_wrap(
                                        label.to_owned(),
                                        font,
                                        crate::ui::theme::text_strong(),
                                    )
                                    .size()
                                    .x
                                    .max(144.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(
                                        width,
                                        ui.text_style_height(&egui::TextStyle::Body)
                                            + ui.spacing().item_spacing.y
                                            + ui.spacing().interact_size.y,
                                    ),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(label)
                                                .color(crate::ui::theme::text_strong())
                                                .strong(),
                                        );
                                        ui.horizontal(|ui| {
                                            let prefix = match value {
                                                0 => &mut plugin.draft_mut().microphone_prefix,
                                                1 => &mut plugin.draft_mut().system_audio_prefix,
                                                _ => &mut plugin.draft_mut().typing_prefix,
                                            };
                                            let response = components::text_edit_ui(
                                                ui,
                                                id,
                                                egui::TextEdit::singleline(prefix)
                                                    .desired_width((width - 60.0).max(64.0))
                                                    .char_limit(MAX_PREFIX_LENGTH),
                                            );
                                            changed |= ui_director::egui::text_response(ui, label, prefix, response).changed();
                                            if components::reset_button(ui, id).clicked() {
                                                *prefix = default.into();
                                                changed = true;
                                            }
                                        });
                                    },
                                );
                            }
                        });

                        ui.add_space(4.0);

                        if components::modern_slider_f64(
                            ui,
                            &mut plugin.draft_mut().history_ttl_seconds,
                            10.0..=20.0,
                            15.0,
                            crate::i18n::tr(language, "Maximum display time:"),
                            "s",
                        )
                        .changed()
                        {
                            changed = true;
                        }
                    });
                })
            },
        );
    });

    if changed {
        actions.push(super::OscUiAction::ApplySettings);
    }
}
