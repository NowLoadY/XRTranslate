use super::super::runtime::{
    BannerConfig, BannerContentType, MAX_PREFIX_LENGTH, OscFormatMode, OscMessageSeparator,
};
use crate::ui::components::{self, card};
use eframe::egui;

pub fn render_toolbar(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    mute_gate_enabled: bool,
    actions: &mut Vec<super::OscUiAction>,
) {
    let mut changed = false;

    card(ui, |ui| {
        components::feature_ui(
            ui,
            crate::feature_access::Feature::OscChatbox,
            language,
            |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if components::feature_checkbox(
                            ui,
                            crate::feature_access::Feature::OscChatbox,
                            language,
                            &mut plugin.draft_mut().enabled,
                            "OSC",
                        )
                        .changed()
                        {
                            changed = true;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if components::animated_button(ui, crate::i18n::tr(language, "Clear"))
                                .clicked()
                            {
                                plugin.clear_chatbox();
                                actions.push(super::OscUiAction::ClearHostHistory);
                            }
                        });
                    });

                    ui.add_space(8.0);
                    let mut mute_gate_enabled = mute_gate_enabled;
                    if components::feature_checkbox(
                        ui,
                        crate::feature_access::Feature::MuteSync,
                        language,
                        &mut mute_gate_enabled,
                        crate::i18n::tr(language, "Pause while muted"),
                    )
                    .changed()
                    {
                        actions.push(super::OscUiAction::SetMuteGateEnabled(mute_gate_enabled));
                    }

                    ui.add_space(8.0);
                    components::wavy_divider(ui, crate::ui::theme::text_strong());
                    ui.add_space(8.0);

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
                            plugin.draft().format_mode.label(language),
                            |ui| {
                                let r1 = ui.selectable_value(
                                    &mut plugin.draft_mut().format_mode,
                                    OscFormatMode::BilingualSourceFirst,
                                    OscFormatMode::BilingualSourceFirst.label(language),
                                );
                                let r2 = ui.selectable_value(
                                    &mut plugin.draft_mut().format_mode,
                                    OscFormatMode::BilingualTargetFirst,
                                    OscFormatMode::BilingualTargetFirst.label(language),
                                );
                                let r3 = ui.selectable_value(
                                    &mut plugin.draft_mut().format_mode,
                                    OscFormatMode::Inline,
                                    OscFormatMode::Inline.label(language),
                                );
                                let r4 = ui.selectable_value(
                                    &mut plugin.draft_mut().format_mode,
                                    OscFormatMode::TargetOnly,
                                    OscFormatMode::TargetOnly.label(language),
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
                            let _ = plugin.apply_draft();
                            actions.push(super::OscUiAction::SetSpeakerNumberVisible(
                                speaker_number_enabled,
                            ));
                            actions.push(super::OscUiAction::SaveSettings);
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
                                    selection_changed |= ui
                                        .selectable_value(
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

                    if render_banner_selector(
                        ui,
                        crate::i18n::tr(language, "Header:"),
                        "header_type_combo",
                        &mut plugin.draft_mut().header_config,
                        language,
                    ) {
                        changed = true;
                    }

                    ui.add_space(8.0);

                    if render_banner_selector(
                        ui,
                        crate::i18n::tr(language, "Footer:"),
                        "footer_type_combo",
                        &mut plugin.draft_mut().footer_config,
                        language,
                    ) {
                        changed = true;
                    }

                    ui.add_space(8.0);

                    for (label, value) in [
                        ("Microphone prefix:", 0usize),
                        ("System audio prefix:", 1usize),
                        ("Typing prefix:", 2usize),
                    ] {
                        let row_changed = ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(100.0, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(crate::i18n::tr(language, label))
                                            .color(crate::ui::theme::text_strong())
                                            .strong(),
                                    );
                                },
                            );
                            let mut local_changed = false;
                            match value {
                                0 => {
                                    let edit_resp = components::text_edit_ui(
                                        ui,
                                        "osc_mic_prefix",
                                        egui::TextEdit::singleline(
                                            &mut plugin.draft_mut().microphone_prefix,
                                        )
                                        .hint_text("e.g. 🎤")
                                        .desired_width(180.0)
                                        .char_limit(MAX_PREFIX_LENGTH),
                                    );
                                    if edit_resp.changed() {
                                        local_changed = true;
                                    }
                                    let reset_resp = components::reset_button(ui, "osc_mic_prefix");
                                    if reset_resp.clicked() {
                                        plugin.draft_mut().microphone_prefix = "🎤".into();
                                        local_changed = true;
                                    }
                                }
                                1 => {
                                    let edit_resp = components::text_edit_ui(
                                        ui,
                                        "osc_sys_prefix",
                                        egui::TextEdit::singleline(
                                            &mut plugin.draft_mut().system_audio_prefix,
                                        )
                                        .hint_text("e.g. 🔊")
                                        .desired_width(180.0)
                                        .char_limit(MAX_PREFIX_LENGTH),
                                    );
                                    if edit_resp.changed() {
                                        local_changed = true;
                                    }
                                    let reset_resp = components::reset_button(ui, "osc_sys_prefix");
                                    if reset_resp.clicked() {
                                        plugin.draft_mut().system_audio_prefix = "🔊".into();
                                        local_changed = true;
                                    }
                                }
                                _ => {
                                    let edit_resp = components::text_edit_ui(
                                        ui,
                                        "osc_txt_prefix",
                                        egui::TextEdit::singleline(
                                            &mut plugin.draft_mut().typing_prefix,
                                        )
                                        .hint_text("e.g. 💬")
                                        .desired_width(180.0)
                                        .char_limit(MAX_PREFIX_LENGTH),
                                    );
                                    if edit_resp.changed() {
                                        local_changed = true;
                                    }
                                    let reset_resp = components::reset_button(ui, "osc_txt_prefix");
                                    if reset_resp.clicked() {
                                        plugin.draft_mut().typing_prefix = "💬".into();
                                        local_changed = true;
                                    }
                                }
                            };
                            local_changed
                        }).inner;
                        changed |= row_changed;
                        ui.add_space(4.0);
                    }

                    ui.add_space(4.0);

                    if components::modern_slider_f64(
                        ui,
                        &mut plugin.draft_mut().history_ttl_seconds,
                        10.0..=20.0,
                        15.0,
                        "TTL:",
                        "s",
                    )
                    .changed()
                    {
                        changed = true;
                    }
                })
            },
        );
    });

    if changed {
        actions.push(super::OscUiAction::SettingsApplied(plugin.apply_draft()));
        actions.push(super::OscUiAction::SaveSettings);
    }
}

fn render_banner_selector(
    ui: &mut egui::Ui,
    label: &str,
    combo_id: &str,
    banner: &mut BannerConfig,
    language: crate::i18n::UiLanguage,
) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(100.0, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .color(crate::ui::theme::text_strong())
                        .strong(),
                );
            },
        );

        let combo_resp = components::combobox_ui(
            ui,
            combo_id,
            banner.content_type.label(language),
            |ui| {
                let r1 = ui.selectable_value(
                    &mut banner.content_type,
                    BannerContentType::None,
                    BannerContentType::None.label(language),
                );
                let r2 = ui.selectable_value(
                    &mut banner.content_type,
                    BannerContentType::CustomText,
                    BannerContentType::CustomText.label(language),
                );
                let r3 = ui.selectable_value(
                    &mut banner.content_type,
                    BannerContentType::SystemTime,
                    BannerContentType::SystemTime.label(language),
                );
                let r4 = ui.selectable_value(
                    &mut banner.content_type,
                    BannerContentType::CpuStatus,
                    BannerContentType::CpuStatus.label(language),
                );
                let r5 = ui.selectable_value(
                    &mut banner.content_type,
                    BannerContentType::GpuStatus,
                    BannerContentType::GpuStatus.label(language),
                );
                r1.changed() || r2.changed() || r3.changed() || r4.changed() || r5.changed()
            },
        );

        if combo_resp.inner.unwrap_or(false) {
            changed = true;
        }

        ui.add_space(8.0);

        match banner.content_type {
            BannerContentType::None => {}
            BannerContentType::CustomText => {
                if components::text_edit_ui(
                    ui,
                    combo_id,
                    egui::TextEdit::singleline(&mut banner.custom_text)
                        .hint_text("e.g. [AFK] or [CN/JP]")
                        .desired_width(150.0),
                )
                .changed()
                {
                    changed = true;
                }
            }
            BannerContentType::SystemTime => {}
            BannerContentType::CpuStatus | BannerContentType::GpuStatus => {
                if components::checkbox(
                    ui,
                    &mut banner.show_device_name,
                    crate::i18n::tr(language, "Full Name"),
                )
                .changed()
                {
                    changed = true;
                }
            }
        }
    });

    changed
}
