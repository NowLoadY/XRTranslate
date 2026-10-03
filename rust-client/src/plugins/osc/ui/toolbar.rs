use super::super::runtime::{
    BannerConfig, BannerContentType, MAX_PREFIX_LENGTH, OscFormatMode, OscMessageSeparator,
};
use crate::ui::{
    components::{self, card},
    layout,
};
use eframe::egui;

pub fn render_toolbar(
    plugin: &mut super::super::OscPlugin,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    mute_gate_enabled: bool,
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

                    settings.show_body_unindented(ui, |ui| {
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

                        let banner_width = banner_field_width(
                            ui,
                            crate::i18n::tr(language, "Header:"),
                            &plugin.draft().header_config,
                            language,
                        ) + banner_field_width(
                            ui,
                            crate::i18n::tr(language, "Footer:"),
                            &plugin.draft().footer_config,
                            language,
                        ) + ui.spacing().item_spacing.x;
                        let stack_banners = banner_width > ui.available_width();
                        ui.horizontal_wrapped(|ui| {
                            for (label, id, header) in [
                                ("Header:", "header_type_combo", true),
                                ("Footer:", "footer_type_combo", false),
                            ] {
                                if !header && stack_banners {
                                    ui.end_row();
                                }
                                let banner = if header {
                                    &mut plugin.draft_mut().header_config
                                } else {
                                    &mut plugin.draft_mut().footer_config
                                };
                                let label = crate::i18n::tr(language, label);
                                let width = banner_field_width(ui, label, banner, language);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(width, ui.spacing().interact_size.y),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        changed |=
                                            render_banner_selector(ui, label, id, banner, language);
                                    },
                                );
                            }
                        });

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
                                            changed |= components::text_edit_ui(
                                                ui,
                                                id,
                                                egui::TextEdit::singleline(prefix)
                                                    .desired_width((width - 60.0).max(64.0))
                                                    .char_limit(MAX_PREFIX_LENGTH),
                                            )
                                            .changed();
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

    layout::flow_row(ui, |ui| {
        ui.label(
            egui::RichText::new(label)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );

        let combo_resp =
            components::combobox_ui(ui, combo_id, banner.content_type.label(language), |ui| {
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
            });

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

fn banner_field_width(
    ui: &egui::Ui,
    label: &str,
    banner: &BannerConfig,
    language: crate::i18n::UiLanguage,
) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, crate::ui::theme::text_strong())
        .size()
        .x;
    let combo = layout::control_width(ui, banner.content_type.label(language), None, 96.0, 240.0);
    let extra = match banner.content_type {
        BannerContentType::CustomText => 150.0 + ui.spacing().item_spacing.x,
        BannerContentType::CpuStatus | BannerContentType::GpuStatus => {
            ui.painter()
                .layout_no_wrap(
                    crate::i18n::tr(language, "Full Name").to_owned(),
                    egui::TextStyle::Body.resolve(ui.style()),
                    crate::ui::theme::text_strong(),
                )
                .size()
                .x
                + ui.spacing().icon_width
                + 2.0 * ui.spacing().item_spacing.x
        }
        _ => 0.0,
    };
    (label_width + combo + extra + ui.spacing().item_spacing.x + 4.0).min(ui.max_rect().width())
}
