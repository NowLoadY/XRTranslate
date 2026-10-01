use crate::CaptureSource;
use crate::ui::components::{self, danger_button, status_badge};
use eframe::egui;
use std::hash::{Hash, Hasher};


fn recognition_history_fingerprint(
    entries: &[crate::history::RecognitionHistoryEntry],
    partial_text: &str,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.len().hash(&mut hasher);
    if let Some(first) = entries.first() {
        first.stream_id.hash(&mut hasher);
        first.turn_id.hash(&mut hasher);
        first.text.hash(&mut hasher);
    }
    if let Some(last) = entries.last() {
        last.stream_id.hash(&mut hasher);
        last.turn_id.hash(&mut hasher);
        last.speaker_id.hash(&mut hasher);
        last.text.hash(&mut hasher);
    }
    partial_text.hash(&mut hasher);
    hasher.finish()
}

fn translation_history_fingerprint(entries: &[crate::history::TranslationHistoryEntry]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.len().hash(&mut hasher);
    if let Some(first) = entries.first() {
        first.stream_id.hash(&mut hasher);
        first.turn_id.hash(&mut hasher);
        first.segment_index.hash(&mut hasher);
        first.source.hash(&mut hasher);
        first.translated.hash(&mut hasher);
    }
    if let Some(last) = entries.last() {
        last.stream_id.hash(&mut hasher);
        last.turn_id.hash(&mut hasher);
        last.segment_index.hash(&mut hasher);
        last.speaker_id.hash(&mut hasher);
        last.source.hash(&mut hasher);
        last.translated.hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FullscreenHistory {
    Recognition,
    Translation,
}

#[derive(Clone, Copy, Debug)]
struct HistoryFeedScale {
    text_size: f32,
    source_size: f32,
    speaker_size: f32,
    header_size: f32,
    min_row_height: f32,
    card_margin_x: i8,
    card_margin_y: i8,
    card_radius: u8,
    row_gap: f32,
}

impl HistoryFeedScale {
    fn compute(col_width: f32, available_height: f32) -> Self {
        let width_factor = ((col_width - 130.0) / 150.0).clamp(0.0, 1.0);
        let height_factor = ((available_height - 160.0) / 200.0).clamp(0.0, 1.0);
        let factor = width_factor.min(height_factor);

        let text_size = 10.5 + 2.5 * factor;
        let source_size = 9.0 + 2.5 * factor;
        let speaker_size = 9.5 + 2.0 * factor;
        let header_size = 12.0 + 3.0 * factor;
        let min_row_height = 36.0 + 24.0 * factor;
        let card_margin_x = (5.0 + 5.0 * factor).round() as i8;
        let card_margin_y = (3.0 + 5.0 * factor).round() as i8;
        let card_radius = (5.0 + 4.0 * factor).round() as u8;
        let row_gap = 4.0 + 4.0 * factor;

        Self {
            text_size,
            source_size,
            speaker_size,
            header_size,
            min_row_height,
            card_margin_x,
            card_margin_y,
            card_radius,
            row_gap,
        }
    }
}

fn history_card_with_activity(
    ui: &mut egui::Ui,
    id: egui::Id,
    activity: f32,
    row_height: f32,
    scale: &HistoryFeedScale,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let res = egui::Frame::new()
        .fill(crate::ui::theme::history_surface())
        .corner_radius(egui::CornerRadius::same(scale.card_radius))
        .inner_margin(egui::Margin::symmetric(scale.card_margin_x, scale.card_margin_y))
        .stroke(egui::Stroke::new(1.0, crate::ui::theme::border().gamma_multiply(0.35)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height((row_height - (scale.card_margin_y as f32 * 2.0)).max(0.0));
            crate::ui::animation::AnimationSystem::render_data_text(ui, id, activity, add_contents);
        });
    res.response.interact(egui::Sense::click())
}

fn wrapped_history_text_height(ui: &egui::Ui, text: &str, size: f32, width: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    ui.painter()
        .layout(
            text.to_owned(),
            egui::FontId::proportional(size),
            egui::Color32::WHITE,
            width.max(48.0),
        )
        .size()
        .y
}

fn history_row_height(
    ui: &egui::Ui,
    has_speaker: bool,
    source: Option<&str>,
    text: &str,
    scale: &HistoryFeedScale,
) -> f32 {
    let inset_h = (scale.card_margin_x as f32 * 2.0 + 16.0).max(24.0);
    let wrap_width = (ui.available_width() - inset_h).max(48.0);
    let mut content_height = crate::ui::theme::data_text_motion(ui.ctx()).max_offset;
    if has_speaker {
        content_height += if scale.speaker_size < 11.0 { 16.0 } else { 22.0 };
    }
    if let Some(source) = source.filter(|source| !source.is_empty()) {
        content_height += wrapped_history_text_height(ui, source, scale.source_size, wrap_width) + 4.0;
    }
    content_height += wrapped_history_text_height(ui, text, scale.text_size, wrap_width);

    let vertical_inset = (scale.card_margin_y as f32 * 2.0) + 6.0;
    (content_height + vertical_inset).max(scale.min_row_height)
}

fn history_activity(index: usize, row_count: usize) -> f32 {
    if row_count <= 1 {
        return 1.0;
    }
    index as f32 / (row_count - 1) as f32
}

#[cfg(test)]
mod virtual_history_tests {
    use super::*;

    #[test]
    fn history_activity_increases_towards_the_newest_row() {
        assert_eq!(history_activity(0, 0), 1.0);
        assert_eq!(history_activity(0, 1), 1.0);
        assert_eq!(history_activity(0, 5), 0.0);
        assert_eq!(history_activity(4, 5), 1.0);
    }
}

pub fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if let Some(mode) = app.fullscreen_history {
        render_fullscreen_history(app, ui, mode);
        return;
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr(app.ui_language, "Translation"))
                .size(22.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.max_rect().width() < 680.0 {
                ui.add_space(56.0);
            }
            let (is_active, is_error) = if app.connection_status.to_lowercase().contains("error") {
                (false, true)
            } else {
                (true, false)
            };
            let status = crate::i18n::tr_dynamic(app.ui_language, &app.connection_status);
            status_badge(ui, status.as_ref(), is_active, is_error);
        });
    });

    ui.add_space(14.0);

    let is_wide = (ui.available_width() > ui.available_height() && ui.available_width() >= 540.0)
        || ui.available_width() >= 800.0;

    if is_wide {
        let avail_w = ui.available_width();
        let avail_h = ui.available_height();
        let left_target_w = (avail_w * 0.42).clamp(300.0, 360.0);
        ui.horizontal_top(|ui| {
            let left_resp = ui.allocate_ui_with_layout(
                egui::vec2(left_target_w, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("translation_controls_scroll")
                        .show(ui, |ui| {
                            render_translation_controls(app, ui);
                        });
                },
            );
            let actual_left_w = left_resp.response.rect.width();
            let remaining_w = (avail_w - actual_left_w - ui.spacing().item_spacing.x - 4.0).max(100.0);
            ui.allocate_ui_with_layout(
                egui::vec2(remaining_w, avail_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    render_history_feeds(app, ui, remaining_w, avail_h);
                },
            );
        });
    } else {
        render_translation_controls(app, ui);
        ui.add_space(10.0);
        let avail_h = ui.available_height();
        let avail_w = ui.available_width();
        render_history_feeds(app, ui, avail_w, avail_h);
    }
}

fn render_translation_controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    components::card(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let capabilities = app.language_capabilities();
        if components::translation_language_selector(
            ui,
            "translation_page",
            &mut app.source_lang,
            &mut app.target_lang,
            capabilities,
            app.ui_language,
        ) {
            app.apply_language_route();
        }
        let tts_configured = app.service_config.tts_is_configured();
        let compact = ui.available_width() < 600.0;
        let mut details = crate::ui::layout::flow_row(ui, |ui| {
            let details = ui
                .horizontal(|ui| {
                    if app.translation_enabled {
                        if danger_button(ui, crate::i18n::tr(app.ui_language, "Stop Translation"))
                            .clicked()
                        {
                            app.stop();
                        }
                    } else if components::primary_button(
                        ui,
                        crate::i18n::tr(app.ui_language, "Start Translation"),
                    )
                    .clicked()
                    {
                        app.start(Some(ui.ctx().clone()));
                    }
                    components::responsive_settings_button(
                        ui,
                        "translation_settings",
                        compact,
                        app.ui_language,
                    )
                })
                .inner;
            let mut tts_enabled = app.tts_enabled;
            let tts_response = ui.add_enabled_ui(tts_configured, |ui| {
                if components::feature_checkbox(
                    ui,
                    crate::feature_access::Feature::TtsPlayback,
                    app.ui_language,
                    &mut tts_enabled,
                    crate::i18n::tr(app.ui_language, "TTS"),
                )
                .changed()
                {
                    app.set_tts_enabled(tts_enabled);
                }
            });
            if !tts_configured {
                tts_response
                    .response
                    .on_disabled_hover_text(crate::i18n::tr(
                        app.ui_language,
                        "Configure a TTS provider in Settings to enable TTS playback.",
                    ));
            }

            details
        });
        crate::ui::layout::flow_row(ui, |ui| {
            render_capture_control(app, ui, CaptureSource::Microphone);
            if crate::audio::AudioSystem::supports_system_audio()
                && (app
                    .capture_source
                    .routes()
                    .contains(&CaptureSource::SystemAudio)
                    || app.live_input_requested(CaptureSource::SystemAudio, true))
            {
                render_capture_control(app, ui, CaptureSource::SystemAudio);
            }
        });
        details.show_body_unindented(ui, |ui| {
            ui.add_space(14.0);
            crate::ui::layout::flow_row(ui, |ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(app.ui_language, "Source:"))
                        .color(crate::ui::theme::text_strong())
                        .strong(),
                );
                let previous_source = app.capture_source;
                let selected_source_text = match (&app.capture_source, &app.system_audio_input) {
                    (
                        CaptureSource::SystemAudio,
                        crate::SystemAudioInputSelection::Application { application },
                    ) => format!(
                        "{} · {}",
                        crate::i18n::tr(app.ui_language, "Application audio"),
                        application.display_name
                    ),
                    (
                        CaptureSource::Both,
                        crate::SystemAudioInputSelection::Application { application },
                    ) => format!(
                        "{} · {}",
                        crate::i18n::tr(app.ui_language, "Both"),
                        application.display_name
                    ),
                    (CaptureSource::Microphone, _) => {
                        crate::i18n::tr(app.ui_language, "Microphone").to_owned()
                    }
                    (CaptureSource::SystemAudio, _) => {
                        crate::i18n::tr(app.ui_language, "System Audio").to_owned()
                    }
                    (CaptureSource::Both, _) => crate::i18n::tr(app.ui_language, "Both").to_owned(),
                };
                components::combobox_ui(ui, "capture_source", selected_source_text, |ui| {
                    ui.selectable_value(
                        &mut app.capture_source,
                        CaptureSource::Microphone,
                        crate::i18n::tr(app.ui_language, "Microphone"),
                    );
                    let system_audio_available = !app.loopback_devices.is_empty()
                        || !app.audio_applications.is_empty()
                        || matches!(
                            &app.system_audio_input,
                            crate::SystemAudioInputSelection::Application { .. }
                        );
                    ui.add_enabled_ui(system_audio_available, |ui| {
                        ui.selectable_value(
                            &mut app.capture_source,
                            CaptureSource::SystemAudio,
                            crate::i18n::tr(app.ui_language, "System Audio"),
                        );
                        ui.selectable_value(
                            &mut app.capture_source,
                            CaptureSource::Both,
                            crate::i18n::tr(app.ui_language, "Both"),
                        );
                    });
                });
                if app.capture_source != previous_source {
                    app.switch_capture_source(previous_source);
                }
            });
            ui.add_space(8.0);
            render_input_channels(app, ui, render_capture_device_selector);

            ui.add_space(8.0);
            egui::CollapsingHeader::new(crate::i18n::tr(app.ui_language, "Recognition settings"))
                .id_salt("recognition_settings")
                .show(ui, |ui| {
                    render_input_channels(app, ui, render_input_adaptation);
                    if let Some(config) = &app.selected_input_config {
                        ui.weak(format!(
                            "{} Hz, {} ch ({})",
                            config.sample_rate, config.channels, config.sample_format
                        ));
                    }
                });
            ui.add_space(12.0);

            crate::ui::layout::flow_row(ui, |ui| {
                ui.add_space(8.0);
                let mic_capturing = matches!(
                    app.capture_source,
                    CaptureSource::Microphone | CaptureSource::Both
                );
                let status = app.voice_clone_state().cloned();
                let busy = status.as_ref().is_some_and(|status| {
                    matches!(
                        status.state,
                        xrtranslate_protocol::VoiceClonePhase::Collecting
                            | xrtranslate_protocol::VoiceClonePhase::Registering
                    )
                });
                let label = match status.as_ref().map(|status| status.state) {
                    Some(xrtranslate_protocol::VoiceClonePhase::Collecting) => status
                        .as_ref()
                        .map(|status| {
                            format!(
                                "{} {:.1}/{:.1}s",
                                crate::i18n::tr(app.ui_language, "Collecting voice…"),
                                status.collected_seconds,
                                status.required_seconds
                            )
                        })
                        .unwrap(),
                    Some(xrtranslate_protocol::VoiceClonePhase::Registering) => {
                        crate::i18n::tr(app.ui_language, "Creating voice…").into()
                    }
                    _ => crate::i18n::tr(app.ui_language, "Clone microphone voice").into(),
                };
                let enabled = app.host_channels().any(|channel| {
                    channel.source == CaptureSource::Microphone && channel.capturing
                }) && mic_capturing
                    && !busy
                    && tts_configured;
                let response = components::animated_button_enabled(ui, &label, enabled);
                let clicked = response.clicked();
                if let Some(message) = status.as_ref().and_then(|status| status.message.as_deref())
                {
                    response.on_hover_text(message);
                } else if !tts_configured {
                    response.on_disabled_hover_text(crate::i18n::tr(
                        app.ui_language,
                        "Configure a TTS provider in Settings to enable voice cloning.",
                    ));
                } else if !mic_capturing {
                    response.on_disabled_hover_text(crate::i18n::tr(
                        app.ui_language,
                        "Start microphone translation to clone your voice.",
                    ));
                }
                if clicked {
                    app.begin_voice_clone();
                }
                if status.as_ref().is_some_and(|status| {
                    status.state == xrtranslate_protocol::VoiceClonePhase::Ready
                }) {
                    ui.label(egui::RichText::new("OK").color(egui::Color32::from_rgb(5, 150, 105)));
                }

                if crate::feature_access::is_available(
                    crate::feature_access::Feature::FloatingSubtitles,
                ) {
                    ui.add_space(12.0);
                    let mut floating_enabled = app.floating_subtitles_enabled;
                    if components::feature_checkbox(
                        ui,
                        crate::feature_access::Feature::FloatingSubtitles,
                        app.ui_language,
                        &mut floating_enabled,
                        crate::i18n::tr(app.ui_language, "Floating subtitles"),
                    )
                    .changed()
                    {
                        app.set_floating_subtitles_enabled(floating_enabled);
                    }
                }
            });
        });
        if let Some(message) = app.voice_clone_state().and_then(|status| {
            (status.state == xrtranslate_protocol::VoiceClonePhase::Failed)
                .then_some(status.message.as_deref())
                .flatten()
        }) {
            ui.add_space(8.0);
            components::error_notice(ui, app.ui_language, message);
        }
    });
}

fn render_history_section(
    ui: &mut egui::Ui,
    title: &str,
    scale: &HistoryFeedScale,
    language: crate::i18n::UiLanguage,
    add_contents: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    let padding_x = if scale.header_size < 14.0 { 8 } else { 12 };
    let padding_y = if scale.header_size < 14.0 { 6 } else { 10 };
    ui.push_id(title, |ui| {
        let mut requested_fullscreen = false;
        let frame_resp = egui::Frame::new()
            .fill(crate::ui::theme::surface_subtle())
            .corner_radius(egui::CornerRadius::same(scale.card_radius + 2))
            .inner_margin(egui::Margin::symmetric(padding_x, padding_y))
            .stroke(egui::Stroke::new(1.0, crate::ui::theme::border()))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let available_w = ui.available_width();
                ui.horizontal(|ui| {
                    ui.set_width(available_w);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if components::animated_button(ui, "⛶")
                            .on_hover_text(crate::i18n::tr(language, "Fullscreen"))
                            .clicked()
                        {
                            requested_fullscreen = true;
                        }

                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let title_resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(title)
                                        .size(scale.header_size)
                                        .color(crate::ui::theme::text_strong())
                                        .strong(),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            );
                            if title_resp.clicked() {
                                requested_fullscreen = true;
                            }
                            if title_resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            title_resp.on_hover_text(crate::i18n::tr(language, "Click to view full screen"));
                        });
                    });
                });
                ui.add_space((scale.header_size * 0.25).round().max(2.0));
                let body_fullscreen = add_contents(ui);
                requested_fullscreen || body_fullscreen
            });
        let card_clicked = frame_resp.response.interact(egui::Sense::click()).clicked();
        frame_resp.inner || card_clicked
    })
    .inner
}

fn render_history_feeds(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    available_width: f32,
    available_height: f32,
) {
    let stack_history = crate::ui::layout::should_stack(available_width, 2, 140.0);
    let col_width = if stack_history {
        available_width
    } else {
        (available_width - ui.spacing().item_spacing.x) * 0.5
    };
    let scale = HistoryFeedScale::compute(col_width, available_height);
    let compact_history_height = ((available_height - 20.0) / 2.0).clamp(100.0, 400.0);

    let mut enter_recognition_fullscreen = false;
    let mut enter_translation_fullscreen = false;

    ui.columns(if stack_history { 1 } else { 2 }, |columns| {
        let mut rec_card_clicked = false;
        let rec_fullscreen = render_history_section(
            &mut columns[0],
            &format!(
                "{} ({})",
                crate::i18n::tr(app.ui_language, "Recognition History"),
                app.recognition_history.len()
            ),
            &scale,
            app.ui_language,
            |ui| {
                let history_height = if stack_history {
                    compact_history_height
                } else {
                    (available_height - 10.0).max(180.0)
                };
                ui.set_min_height(history_height);
                let scroll_state_id = ui.make_persistent_id("recognition_history_scroll_state");
                let previous_fingerprint = ui.memory(|memory| {
                    memory
                        .data
                        .get_temp::<u64>(scroll_state_id)
                        .unwrap_or_default()
                });
                let current_fingerprint =
                    recognition_history_fingerprint(&app.recognition_history, &app.partial_text);
                let has_partial = !app.partial_text.is_empty();
                let row_count = app.recognition_history.len() + usize::from(has_partial);
                let should_scroll = row_count > 0 && current_fingerprint != previous_fingerprint;

                let frame_res = egui::Frame::new()
                    .fill(crate::ui::theme::history_viewport())
                    .corner_radius(egui::CornerRadius::same(scale.card_radius))
                    .inner_margin(egui::Margin::symmetric(scale.card_margin_x, scale.card_margin_y))
                    .show(ui, |ui| {
                        ui.set_height((history_height - 8.0).max(0.0));
                        if row_count == 0 {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(app.ui_language, "No speech"))
                                    .color(crate::ui::theme::text_weak())
                                    .italics()
                                    .size(scale.text_size),
                            );
                        } else {
                            let row_heights = app
                                .recognition_history
                                .iter()
                                .map(|entry| {
                                    history_row_height(
                                        ui,
                                        crate::compact_speaker_label(&entry.speaker_id).is_some(),
                                        None,
                                        &entry.text,
                                        &scale,
                                    )
                                    })
                                .chain(has_partial.then(|| {
                                    history_row_height(ui, false, None, &app.partial_text, &scale)
                                }))
                                .collect::<Vec<_>>();
                            crate::ui::layout::show_variable_virtual_rows(
                                ui,
                                "recognition_history_scroll",
                                &row_heights,
                                scale.row_gap,
                                should_scroll,
                                |ui, index, row_height| {
                                    if let Some(entry) = app.recognition_history.get(index) {
                                        let activity = if entry.live {
                                            1.0
                                        } else {
                                            history_activity(index, row_count)
                                        };
                                        let row_id = ui.make_persistent_id((
                                            "recognition_history_data",
                                            index,
                                        ));
                                        let card_resp = history_card_with_activity(
                                            ui,
                                            row_id,
                                            activity,
                                            row_height,
                                            &scale,
                                            |ui| {
                                                if let Some(speaker) =
                                                    crate::compact_speaker_label(&entry.speaker_id)
                                                {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(speaker)
                                                                .color(crate::ui::theme::primary_dark())
                                                                .size(scale.speaker_size)
                                                                .strong(),
                                                        );
                                                    });
                                                    ui.add_space(2.0);
                                                }
                                                render_text_with_term_matches(
                                                    ui,
                                                    &entry.text,
                                                    &entry.activation_matches,
                                                    &entry.context_matches,
                                                    crate::ui::theme::text_normal(),
                                                    false,
                                                    scale.text_size,
                                                );
                                            },
                                        );
                                        if card_resp.clicked() {
                                            rec_card_clicked = true;
                                        }
                                    } else {
                                        let row_id = ui.make_persistent_id((
                                            "recognition_history_data",
                                            index,
                                        ));
                                        let card_resp = history_card_with_activity(
                                            ui,
                                            row_id,
                                            1.0,
                                            row_height,
                                            &scale,
                                            |ui| {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.label(
                                                        egui::RichText::new("• • •")
                                                            .color(crate::ui::theme::primary())
                                                            .size((scale.text_size - 1.5).max(9.0))
                                                            .strong(),
                                                    );
                                                    ui.add_space(2.0);
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(&app.partial_text)
                                                                .color(
                                                                    crate::ui::theme::primary_dark(),
                                                                )
                                                                .size(scale.text_size)
                                                                .italics(),
                                                        )
                                                        .wrap(),
                                                    )
                                                });
                                            },
                                        );
                                        if card_resp.clicked() {
                                            rec_card_clicked = true;
                                        }
                                    }
                                },
                            );
                        }
                    });

                let viewport_resp = frame_res.response.interact(egui::Sense::click());
                let clicked = viewport_resp.clicked();
                if viewport_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                viewport_resp.on_hover_text(crate::i18n::tr(app.ui_language, "Click to view full screen"));

                ui.memory_mut(|memory| {
                    memory
                        .data
                        .insert_temp(scroll_state_id, current_fingerprint);
                });
                clicked || rec_card_clicked
            },
        );
        if rec_fullscreen {
            enter_recognition_fullscreen = true;
        }

        if stack_history {
            columns[0].add_space(10.0);
        }
        let translation_column = if stack_history {
            &mut columns[0]
        } else {
            &mut columns[1]
        };
        let mut trans_card_clicked = false;
        let trans_fullscreen = render_history_section(
            translation_column,
            &format!(
                "{} ({})",
                crate::i18n::tr(app.ui_language, "Translation History"),
                app.translations.len()
            ),
            &scale,
            app.ui_language,
            |ui| {
                let history_height = if stack_history {
                    compact_history_height
                } else {
                    (available_height - 10.0).max(180.0)
                };
                ui.set_min_height(history_height);
                let scroll_state_id = ui.make_persistent_id("translation_history_scroll_state");
                let previous_fingerprint = ui
                    .memory(|memory| memory.data.get_temp::<u64>(scroll_state_id))
                    .unwrap_or_default();
                let current_fingerprint = translation_history_fingerprint(&app.translations);
                let row_count = app.translations.len();
                let should_scroll = row_count > 0 && current_fingerprint != previous_fingerprint;

                let frame_res = egui::Frame::new()
                    .fill(crate::ui::theme::history_viewport())
                    .corner_radius(egui::CornerRadius::same(scale.card_radius))
                    .inner_margin(egui::Margin::symmetric(scale.card_margin_x, scale.card_margin_y))
                    .show(ui, |ui| {
                        ui.set_height((history_height - 8.0).max(0.0));
                        if app.translations.is_empty() {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    app.ui_language,
                                    "No translations",
                                ))
                                .color(crate::ui::theme::text_weak())
                                .italics()
                                .size(scale.text_size),
                            );
                        } else {
                            let row_heights = app
                                .translations
                                .iter()
                                .map(|entry| {
                                    history_row_height(
                                        ui,
                                        crate::compact_speaker_label(&entry.speaker_id).is_some(),
                                        Some(&entry.source),
                                        &entry.translated,
                                        &scale,
                                    )
                                })
                                .collect::<Vec<_>>();
                            crate::ui::layout::show_variable_virtual_rows(
                                ui,
                                "translation_history_scroll",
                                &row_heights,
                                scale.row_gap,
                                should_scroll,
                                |ui, index, row_height| {
                                    let entry = &app.translations[index];
                                    let row_id =
                                        ui.make_persistent_id(("translation_history_data", index));
                                    let card_resp = history_card_with_activity(
                                        ui,
                                        row_id,
                                        if entry.live {
                                            1.0
                                        } else {
                                            history_activity(index, row_count)
                                        },
                                        row_height,
                                        &scale,
                                        |ui| {
                                            if let Some(speaker) =
                                                crate::compact_speaker_label(&entry.speaker_id)
                                            {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(speaker)
                                                            .color(crate::ui::theme::primary_dark())
                                                            .size(scale.speaker_size)
                                                            .strong(),
                                                    );
                                                });
                                                ui.add_space(2.0);
                                            }
                                            if !entry.source.is_empty() {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(&entry.source)
                                                            .color(crate::ui::theme::text_weak())
                                                            .size(scale.source_size),
                                                    )
                                                    .wrap(),
                                                );
                                                ui.add_space(1.5);
                                            }
                                            render_text_with_term_matches(
                                                ui,
                                                &entry.translated,
                                                &entry.term_matches,
                                                &[],
                                                crate::ui::theme::text_strong(),
                                                true,
                                                scale.text_size,
                                            );
                                        },
                                    );
                                    if card_resp.clicked() {
                                        trans_card_clicked = true;
                                    }
                                },
                            );
                        }
                    });

                let viewport_resp = frame_res.response.interact(egui::Sense::click());
                let clicked = viewport_resp.clicked();
                if viewport_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                viewport_resp.on_hover_text(crate::i18n::tr(app.ui_language, "Click to view full screen"));

                ui.memory_mut(|memory| {
                    memory
                        .data
                        .insert_temp(scroll_state_id, current_fingerprint);
                });
                clicked || trans_card_clicked
            },
        );
        if trans_fullscreen {
            enter_translation_fullscreen = true;
        }
    });

    if enter_recognition_fullscreen {
        app.fullscreen_history = Some(FullscreenHistory::Recognition);
        ui.ctx().request_repaint();
    } else if enter_translation_fullscreen {
        app.fullscreen_history = Some(FullscreenHistory::Translation);
        ui.ctx().request_repaint();
    }
}

fn render_fullscreen_history(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    mode: FullscreenHistory,
) {
    let language = app.ui_language;
    ui.horizontal(|ui| {
        let back_label = format!("← {}", crate::i18n::tr(language, "Back"));
        if components::animated_button(ui, &back_label).clicked() {
            app.fullscreen_history = None;
            ui.ctx().request_repaint();
        }
        ui.add_space(8.0);
        let title = match mode {
            FullscreenHistory::Recognition => format!(
                "{} ({})",
                crate::i18n::tr(language, "Recognition History"),
                app.recognition_history.len()
            ),
            FullscreenHistory::Translation => format!(
                "{} ({})",
                crate::i18n::tr(language, "Translation History"),
                app.translations.len()
            ),
        };
        ui.label(
            egui::RichText::new(title)
                .size(18.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
    });

    ui.add_space(10.0);

    egui::ScrollArea::vertical()
        .id_salt("fullscreen_history_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match mode {
                FullscreenHistory::Recognition => {
                    if app.recognition_history.is_empty() && app.partial_text.is_empty() {
                        components::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "No speech"))
                                    .color(crate::ui::theme::text_weak())
                                    .italics()
                                    .size(14.0),
                            );
                        });
                        return;
                    }

                    for entry in &app.recognition_history {
                        components::history_entry_card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if let Some(speaker) = crate::compact_speaker_label(&entry.speaker_id) {
                                ui.horizontal(|ui| {
                                    components::speaker_badge(ui, &speaker);
                                });
                                ui.add_space(4.0);
                            }
                            render_text_with_term_matches(
                                ui,
                                &entry.text,
                                &entry.activation_matches,
                                &entry.context_matches,
                                crate::ui::theme::text_normal(),
                                false,
                                15.5,
                            );
                        });
                        ui.add_space(8.0);
                    }

                    if !app.partial_text.is_empty() {
                        components::history_entry_card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new("• • •")
                                        .color(crate::ui::theme::primary())
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.add_space(4.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&app.partial_text)
                                            .color(crate::ui::theme::primary_dark())
                                            .size(15.5)
                                            .italics(),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                    }
                }
                FullscreenHistory::Translation => {
                    if app.translations.is_empty() {
                        components::card(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "No translations"))
                                    .color(crate::ui::theme::text_weak())
                                    .italics()
                                    .size(14.0),
                            );
                        });
                        return;
                    }

                    for entry in &app.translations {
                        components::history_entry_card(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if let Some(speaker) = crate::compact_speaker_label(&entry.speaker_id) {
                                ui.horizontal(|ui| {
                                    components::speaker_badge(ui, &speaker);
                                });
                                ui.add_space(4.0);
                            }
                            if !entry.source.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&entry.source)
                                            .color(crate::ui::theme::text_weak())
                                            .size(13.5),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(4.0);
                            }
                            render_text_with_term_matches(
                                ui,
                                &entry.translated,
                                &entry.term_matches,
                                &[],
                                crate::ui::theme::text_strong(),
                                true,
                                16.0,
                            );
                        });
                        ui.add_space(8.0);
                    }
                }
            }
        });
}

fn render_text_with_term_matches(
    ui: &mut egui::Ui,
    text: &str,
    primary_matches: &[xrtranslate_protocol::CorpusTermMatch],
    secondary_matches: &[xrtranslate_protocol::CorpusTermMatch],
    base_color: egui::Color32,
    strong: bool,
    font_size: f32,
) -> egui::Response {
    let mut matches = secondary_matches
        .iter()
        .map(|term_match| (term_match, false))
        .chain(primary_matches.iter().map(|term_match| (term_match, true)))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .start_byte
            .cmp(&right.0.start_byte)
            // Prefer activations when spans coincide.
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| right.0.end_byte.cmp(&left.0.end_byte))
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut cursor = 0usize;
        for (term_match, primary) in matches {
            let (Ok(start), Ok(end)) = (
                usize::try_from(term_match.start_byte),
                usize::try_from(term_match.end_byte),
            ) else {
                continue;
            };
            if start < cursor
                || end <= start
                || end > text.len()
                || !text.is_char_boundary(start)
                || !text.is_char_boundary(end)
                || text.get(start..end) != Some(term_match.text.as_str())
            {
                continue;
            }
            if cursor < start {
                let mut text = egui::RichText::new(&text[cursor..start])
                    .color(base_color)
                    .size(font_size);
                if strong {
                    text = text.strong();
                }
                ui.add(egui::Label::new(text).wrap());
            }
            let tooltip = term_match
                .sources
                .iter()
                .map(|source| {
                    format!(
                        "{}\n{} / {}\n{}",
                        source.title, source.domain, source.subdomain, source.corpus_id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut highlighted = egui::RichText::new(&text[start..end])
                .color(if primary {
                    crate::ui::theme::primary_dark()
                } else {
                    egui::Color32::from_rgb(96, 165, 250)
                })
                .size(font_size);
            if primary {
                highlighted = highlighted.strong();
            }
            ui.add(egui::Label::new(highlighted).wrap())
                .on_hover_text(tooltip);
            cursor = end;
        }
        if cursor < text.len() {
            let mut trailing = egui::RichText::new(&text[cursor..])
                .color(base_color)
                .size(font_size);
            if strong {
                trailing = trailing.strong();
            }
            ui.add(egui::Label::new(trailing).wrap());
        }
    })
    .response
}

fn render_input_adaptation(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    source: CaptureSource,
) {
    if app.capture_source == CaptureSource::Both {
        let title = match source {
            CaptureSource::Microphone => crate::i18n::tr(app.ui_language, "Microphone").to_string(),
            CaptureSource::SystemAudio => {
                crate::i18n::tr(app.ui_language, "System Audio").to_string()
            }
            CaptureSource::Both => unreachable!(),
        };
        ui.label(
            egui::RichText::new(title)
                .color(crate::ui::theme::text_strong())
                .size(13.5)
                .strong(),
        );
        ui.add_space(4.0);
    }
    let language = app.ui_language;
    let recognize_when = crate::i18n::tr(language, "Recognize when:");
    let speak = crate::i18n::tr(language, "Speak");
    let always = crate::i18n::tr(language, "Always");
    let vad_sensitivity = crate::i18n::tr(language, "VAD Sensitivity");
    let pause_tolerance = crate::i18n::tr(language, "Pause tolerance");
    let changed = {
        let recognition = app.recognition_settings_mut(source);
        let timing_changed = ui
            .horizontal(|ui| {
                ui.label(
                    egui::RichText::new(recognize_when)
                        .color(crate::ui::theme::text_strong())
                        .strong(),
                );
                let previous = recognition.continuous_recognition;
                let selected_timing_text = if recognition.continuous_recognition {
                    always
                } else {
                    speak
                };
                components::combobox_ui(
                    ui,
                    ("recognition_timing", source),
                    selected_timing_text,
                    |ui| {
                        ui.selectable_value(&mut recognition.continuous_recognition, false, speak);
                        ui.selectable_value(&mut recognition.continuous_recognition, true, always);
                    },
                );
                recognition.continuous_recognition != previous
            })
            .inner;
        let background_response = components::modern_slider_f32(
            ui,
            &mut recognition.background_noise,
            0.05..=0.8,
            0.30,
            vad_sensitivity,
            &[],
        );
        let background_changed = background_response.drag_stopped()
            || (background_response.changed() && !background_response.dragged());
        let pause_changed = if recognition.continuous_recognition {
            false
        } else {
            let response = components::modern_slider_f32(
                ui,
                &mut recognition.pause_tolerance,
                0.0..=1.0,
                0.10,
                pause_tolerance,
                &[],
            );
            response.drag_stopped() || (response.changed() && !response.dragged())
        };
        timing_changed || background_changed || pause_changed
    };
    if changed {
        app.set_audio_adaptation(source);
    }
}

fn render_capture_control(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    source: CaptureSource,
) {
    let microphone = source == CaptureSource::Microphone;
    let enabled = if microphone {
        app.microphone_enabled
    } else {
        app.host_input_active(source)
    };
    let (id, label) = match (microphone, enabled) {
        (true, true) => (
            "microphone_input",
            "Turn off microphone input (including meetings)",
        ),
        (true, false) => (
            "microphone_input",
            "Turn on microphone input (including meetings)",
        ),
        (false, true) => ("system_audio_input", "Turn off system audio translation"),
        (false, false) => ("system_audio_input", "Turn on system audio translation"),
    };
    let size = egui::vec2(
        components::INPUT_TOGGLE_SIZE + ui.spacing().item_spacing.x + components::AUDIO_METER_WIDTH,
        components::INPUT_TOGGLE_SIZE,
    );
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if components::input_toggle(
                ui,
                id,
                enabled,
                microphone,
                crate::i18n::tr(app.ui_language, label),
            )
            .clicked()
            {
                if microphone {
                    app.set_microphone_enabled(!enabled, Some(ui.ctx().clone()));
                } else {
                    app.set_system_audio_enabled(!enabled, Some(ui.ctx().clone()));
                }
            }
            render_audio_level(app, ui, source);
        },
    );
}

fn render_audio_level(app: &crate::XRTranslateApp, ui: &mut egui::Ui, source: CaptureSource) {
    let (id, level, vad_active) = match source {
        CaptureSource::Microphone => ("microphone", &app.input_level, &app.microphone_vad_active),
        CaptureSource::SystemAudio => (
            "system_audio",
            &app.loopback_level,
            &app.loopback_vad_active,
        ),
        CaptureSource::Both => return,
    };
    let level = f32::from_bits(level.load(std::sync::atomic::Ordering::Relaxed)).clamp(0.0, 1.0);
    let decibels = 20.0 * level.max(0.000_001).log10();
    let raw_fraction = ((decibels + 60.0) / 60.0).clamp(0.0, 1.0);
    let active = vad_active.load(std::sync::atomic::Ordering::Relaxed);

    components::segmented_audio_meter(ui, id, raw_fraction, active, app.input_capturing(source));
}

fn render_input_channels(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    render: fn(&mut crate::XRTranslateApp, &mut egui::Ui, CaptureSource),
) {
    let sources = app.capture_source.routes();
    let columns = if crate::ui::layout::should_stack(ui.available_width(), sources.len(), 360.0) {
        1
    } else {
        sources.len()
    };
    ui.columns(columns, |columns| {
        for (index, &source) in sources.iter().enumerate() {
            let column = index % columns.len();
            let ui = &mut columns[column];
            if index > 0 && column == 0 {
                ui.add_space(8.0);
            }
            ui.push_id(source, |ui| render(app, ui, source));
        }
    });
}

fn render_capture_device_selector(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    source: CaptureSource,
) {
    let microphone = source == CaptureSource::Microphone;
    ui.label(crate::i18n::tr(
        app.ui_language,
        if microphone {
            "Microphone"
        } else {
            "System Audio"
        },
    ));
    if microphone {
        let previous_device = app.selected_device_id.clone();
        let current_name = app
            .devices
            .iter()
            .find(|device| device.id == app.selected_device_id)
            .map(|device| device.name.as_str())
            .unwrap_or(crate::i18n::tr(app.ui_language, "Default microphone"));
        let mut options = vec![(
            String::new(),
            crate::i18n::tr(app.ui_language, "Default microphone").to_string(),
        )];
        options.extend(
            app.devices
                .iter()
                .map(|device| (device.id.clone(), device.name.clone())),
        );
        if components::searchable_combobox(
            ui,
            "mic_device_selector",
            current_name,
            &mut app.selected_device_id,
            &options,
        ) {
            app.switch_capture_device(source, previous_device);
        }
    } else {
        render_system_audio_input_selector(app, ui, "loopback_device_selector");
    }
}

fn render_system_audio_input_selector(
    app: &mut crate::XRTranslateApp,
    ui: &mut egui::Ui,
    combo_id: &'static str,
) {
    match app.system_audio_input.clone() {
        crate::SystemAudioInputSelection::Application { application } => {
            let available = app
                .audio_applications
                .iter()
                .any(|candidate| candidate.id == application.id.0);
            let status = if available {
                crate::i18n::tr(app.ui_language, "Application audio")
            } else {
                crate::i18n::tr(app.ui_language, "Not running")
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} · {status}", application.display_name)).color(
                        if available {
                            crate::ui::theme::text_strong()
                        } else {
                            crate::ui::theme::danger()
                        },
                    ),
                )
                .truncate(),
            )
            .on_hover_text(format!(
                "{}\n{}",
                application.display_name,
                crate::i18n::tr(
                    app.ui_language,
                    "Configured by the applied Audio Studio route",
                )
            ));
            if components::animated_button(
                ui,
                crate::i18n::tr(app.ui_language, "Edit in Audio Studio"),
            )
            .clicked()
            {
                app.open_audio_studio();
            }
        }
        crate::SystemAudioInputSelection::Endpoint { .. } => {
            if app.loopback_devices.is_empty() {
                ui.label(crate::i18n::tr(
                    app.ui_language,
                    "System audio capture is unavailable on this host",
                ));
                return;
            }
            let previous_device = app.selected_loopback_device_id.clone();
            let current_name = app
                .loopback_devices
                .iter()
                .find(|device| device.id == app.selected_loopback_device_id)
                .map(|device| device.name.as_str())
                .unwrap_or(crate::i18n::tr(
                    app.ui_language,
                    "Default render output (loopback)",
                ));
            let mut loopback_options = vec![(
                String::new(),
                crate::i18n::tr(app.ui_language, "Default render output (loopback)").to_string(),
            )];
            for device in &app.loopback_devices {
                loopback_options.push((device.id.clone(), device.name.clone()));
            }
            if components::searchable_combobox(
                ui,
                combo_id,
                current_name,
                &mut app.selected_loopback_device_id,
                &loopback_options,
            ) {
                app.switch_capture_device(CaptureSource::SystemAudio, previous_device);
            }
        }
    }
}
