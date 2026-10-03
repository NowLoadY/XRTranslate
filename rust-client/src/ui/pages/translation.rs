use crate::CaptureSource;
use crate::ui::components::{self, danger_button, status_badge};
use eframe::egui;

pub(crate) mod history;
pub use history::FullscreenHistory;
use history::{render_fullscreen_history, render_history_feeds};

pub fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    if let Some(mode) = app.fullscreen_history {
        render_fullscreen_history(app, ui, mode);
        return;
    }

    egui::Panel::bottom("translation_text_composer")
        .show_separator_line(false)
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            ui.add_space(10.0);
            app.render_text_composer(ui);
        });

    egui::ScrollArea::vertical()
        .id_salt("translation_page_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| render_content(app, ui));
}

fn render_content(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
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
            let remaining_w =
                (avail_w - actual_left_w - ui.spacing().item_spacing.x - 4.0).max(100.0);
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
        if components::translation_language_selector_with_options(
            ui,
            "translation_page",
            &mut app.source_lang,
            &mut app.target_lang,
            &mut app.additional_target_lang,
            &mut app.asr_only,
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

            if crate::feature_access::is_available(
                crate::feature_access::Feature::FloatingSubtitles,
            ) {
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

            details
        });
        crate::ui::layout::flow_row(ui, |ui| {
            for source in CaptureSource::Both.routes() {
                if app.input_control_visible(*source, true) {
                    render_capture_control(app, ui, *source);
                }
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
    let enabled = app.input_enabled(source);
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
