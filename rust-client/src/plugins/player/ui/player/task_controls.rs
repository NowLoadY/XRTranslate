use super::super::format_time_ms;
use crate::plugins::player::{
    PlayerTranslationRequest, VideoPlayerAction, backend::MediaSource,
    controller::VideoPlayerController, i18n::tr, task::VideoSubtitleMode,
};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

pub(super) fn render_task_control_card(
    controller: &mut VideoPlayerController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> VideoPlayerAction {
    let mut action = VideoPlayerAction::None;

    if controller.fullscreen_mode {
        return action;
    }

    let Some(active_id) = controller.active_task_id.clone() else {
        return action;
    };

    let Some(task) = controller.store.get_mut(&active_id) else {
        return action;
    };

    let mut routing_changed = false;
    let mut task_settings_changed = false;
    let mut do_start = false;
    let mut do_pause = false;
    let mut do_restart = false;

    ui.add_space(10.0);

    components::card(ui, |ui| {
        ui.vertical(|ui| {
            // Header: Task Status and Start / Pause / Restart Buttons
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(tr(language, "Task Configuration"))
                        .size(16.0)
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );

                ui.add_space(8.0);

                if task.is_task_running {
                    ui.label(
                        egui::RichText::new(format!("● {}", tr(language, "Running")))
                            .size(11.5)
                            .color(Color32::from_rgb(22, 101, 52))
                            .strong(),
                    );
                } else if task.subtitles.count() > 0 {
                    ui.label(
                        egui::RichText::new(format!("✓ {}", tr(language, "Completed")))
                            .size(11.5)
                            .color(Color32::from_rgb(79, 70, 229))
                            .strong(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!("○ {}", tr(language, "Idle / Ready")))
                            .size(11.5)
                            .color(crate::ui::theme::text_weak()),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if components::animated_button(ui, tr(language, "Clear & Restart")).clicked() {
                        do_restart = true;
                    }

                    ui.add_space(6.0);

                    if task.is_task_running {
                        if components::animated_button(ui, tr(language, "Pause Task")).clicked() {
                            do_pause = true;
                        }
                    } else {
                        if components::primary_button(ui, tr(language, "Start Task")).clicked() {
                            do_start = true;
                        }
                    }
                });
            });

            ui.add_space(10.0);

            // Processing Progress Section (Audio Extraction & Recognition Progress Bars)
            let is_extracting = controller.is_extracting;
            let extract_frac = controller
                .extraction_progress
                .unwrap_or(if task.subtitles.count() > 0 || (task.is_task_running && !is_extracting) {
                    1.0
                } else {
                    0.0
                })
                .clamp(0.0, 1.0);
            let total_dur_ms = task.duration_ms;
            let last_cue_end_ms = task.subtitles.cues().last().map(|c| c.end_ms).unwrap_or(0);
            let recog_pos_ms = controller
                .recognize_position
                .map(|p| p.as_millis() as i64)
                .unwrap_or(last_cue_end_ms);
            let recog_frac = if total_dur_ms > 0 {
                (recog_pos_ms as f32 / total_dur_ms as f32).clamp(0.0, 1.0)
            } else {
                controller
                    .recognition_progress
                    .unwrap_or(if task.subtitles.count() > 0 && !task.is_task_running {
                        1.0
                    } else {
                        0.0
                    })
                    .clamp(0.0, 1.0)
            };

            Frame::new()
                .fill(Color32::from_rgb(248, 250, 252))
                .stroke(Stroke::new(1.0, Color32::from_rgb(226, 232, 240)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("📊 {}", tr(language, "Processing Progress")))
                                .size(13.5)
                                .strong()
                                .color(crate::ui::theme::text_strong()),
                        );
                        if task.subtitles.count() > 0 {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                Frame::new()
                                    .fill(Color32::from_rgb(238, 242, 255))
                                    .corner_radius(CornerRadius::same(4))
                                    .inner_margin(Margin::symmetric(6, 2))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "✓ {} {}",
                                                task.subtitles.count(),
                                                tr(language, "cues generated")
                                            ))
                                            .size(11.0)
                                            .color(Color32::from_rgb(79, 70, 229))
                                            .strong(),
                                        );
                                    });
                            });
                        }
                    });

                    ui.add_space(8.0);

                    // Step 1: Audio Extraction Progress Bar
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(tr(language, "Audio Extraction"))
                                .size(12.0)
                                .color(crate::ui::theme::text_strong()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let pos_str = controller
                                .extract_position
                                .map(|p| format_time_ms(p.as_millis() as i64))
                                .unwrap_or_else(|| "00:00".into());
                            let dur_str = controller
                                .extract_duration
                                .map(|d| format_time_ms(d.as_millis() as i64))
                                .unwrap_or_else(|| {
                                    if total_dur_ms > 0 {
                                        format_time_ms(total_dur_ms)
                                    } else {
                                        "--:--".into()
                                    }
                                });
                            ui.label(
                                egui::RichText::new(format!("{pos_str} / {dur_str} ({:.0}%)", extract_frac * 100.0))
                                    .size(11.5)
                                    .color(crate::ui::theme::text_weak())
                                    .monospace(),
                            );
                        });
                    });
                    ui.add_space(2.0);
                    let extract_bar = egui::ProgressBar::new(extract_frac)
                        .show_percentage()
                        .animate(is_extracting);
                    ui.add(extract_bar);

                    ui.add_space(8.0);

                    // Step 2: Speech Recognition & Subtitle Generation Progress Bar
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(tr(language, "Speech Recognition & Subtitles"))
                                .size(12.0)
                                .color(crate::ui::theme::text_strong()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let pos_str = format_time_ms(recog_pos_ms);
                            let dur_str = if total_dur_ms > 0 {
                                format_time_ms(total_dur_ms)
                            } else {
                                "--:--".into()
                            };
                            ui.label(
                                egui::RichText::new(format!("{pos_str} / {dur_str} ({:.0}%)", recog_frac * 100.0))
                                    .size(11.5)
                                    .color(crate::ui::theme::text_weak())
                                    .monospace(),
                            );
                        });
                    });
                    ui.add_space(2.0);
                    let is_recog_active = task.is_task_running && !is_extracting;
                    let recog_bar = egui::ProgressBar::new(recog_frac)
                        .show_percentage()
                        .animate(is_recog_active);
                    ui.add(recog_bar);
                });

            ui.add_space(14.0);

            // Row 1: Languages & Subtitle Mode
            ui.columns(3, |cols| {
                // Col 0: Source Language
                cols[0].vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Source Language"))
                            .size(12.5)
                            .color(crate::ui::theme::text_weak()),
                    );
                    ui.add_space(3.0);
                    let source_text = match task.source_language.as_str() {
                        "auto" => tr(language, "Auto Detect").to_owned(),
                        "zh" => tr(language, "Chinese").to_owned(),
                        "ja" => tr(language, "Japanese").to_owned(),
                        "en" => tr(language, "English").to_owned(),
                        "ko" => tr(language, "Korean").to_owned(),
                        _ => task.source_language.clone(),
                    };
                    crate::ui::components::combobox_ui(
                        ui,
                        "player_source_lang_select",
                        source_text,
                        |ui| {
                            let options = [
                                ("auto", tr(language, "Auto Detect")),
                                ("zh", tr(language, "Chinese")),
                                ("ja", tr(language, "Japanese")),
                                ("en", tr(language, "English")),
                                ("ko", tr(language, "Korean")),
                            ];
                            for (val, label) in options {
                                if ui.selectable_value(&mut task.source_language, val.into(), label).changed() {
                                    task_settings_changed = true;
                                }
                            }
                        },
                    );
                });

                // Col 1: Target Language
                cols[1].vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Target Language"))
                            .size(12.5)
                            .color(crate::ui::theme::text_weak()),
                    );
                    ui.add_space(3.0);
                    let target_text = match task.target_language.as_str() {
                        "zh" => tr(language, "Chinese").to_owned(),
                        "zh-TW" => tr(language, "Traditional Chinese").to_owned(),
                        "ja" => tr(language, "Japanese").to_owned(),
                        "en" => tr(language, "English").to_owned(),
                        "ko" => tr(language, "Korean").to_owned(),
                        _ => task.target_language.clone(),
                    };
                    crate::ui::components::combobox_ui(
                        ui,
                        "player_target_lang_select",
                        target_text,
                        |ui| {
                            let options = [
                                ("zh", tr(language, "Chinese")),
                                ("zh-TW", tr(language, "Traditional Chinese")),
                                ("ja", tr(language, "Japanese")),
                                ("en", tr(language, "English")),
                                ("ko", tr(language, "Korean")),
                            ];
                            for (val, label) in options {
                                if ui.selectable_value(&mut task.target_language, val.into(), label).changed() {
                                    task_settings_changed = true;
                                }
                            }
                        },
                    );
                });

                // Col 2: Subtitle Mode
                cols[2].vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Subtitle Mode"))
                            .size(12.5)
                            .color(crate::ui::theme::text_weak()),
                    );
                    ui.add_space(3.0);
                    let mode_str = match &task.subtitle_mode {
                        VideoSubtitleMode::RealtimeTranslation => tr(language, "Real-time speech recognition & translation"),
                        VideoSubtitleMode::ImportedSrt(_) => tr(language, "Use imported/existing subtitles"),
                        VideoSubtitleMode::None => tr(language, "No subtitles (Video playback only)"),
                    };
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::Label::new(
                            egui::RichText::new(mode_str)
                                .size(13.0)
                                .color(crate::ui::theme::text_strong()),
                        )
                        .truncate(),
                    );
                });
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(12.0);

            // Row 2: VAD Sensitivity Tuning (Slider for Continuous Noise Adaptation)
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "VAD Sensitivity / Background Audio"))
                            .size(13.5)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );

                    let val = task.recognition.background_noise;
                    let label = if val <= 0.20 {
                        tr(language, "High (0.15 - Heavy BGM / Music)")
                    } else if val <= 0.40 {
                        tr(language, "Medium (0.35 - Light BGM)")
                    } else {
                        tr(language, "Standard (0.50 - Pure Speech)")
                    };

                    ui.label(
                        egui::RichText::new(format!("• {label}"))
                            .size(12.0)
                            .color(crate::ui::theme::primary())
                            .strong(),
                    );
                });

                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "Lower threshold improves recognition under heavy BGM and music, while higher threshold filters background noise.",
                    ))
                    .size(11.5)
                    .color(crate::ui::theme::text_weak()),
                );

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.selectable_label((task.recognition.background_noise - 0.15).abs() < 0.05, tr(language, "High (0.15 - Heavy BGM / Music)")).clicked() {
                        task.recognition.background_noise = 0.15;
                        task_settings_changed = true;
                    }
                    if ui.selectable_label((task.recognition.background_noise - 0.35).abs() < 0.05, tr(language, "Medium (0.35 - Light BGM)")).clicked() {
                        task.recognition.background_noise = 0.35;
                        task_settings_changed = true;
                    }
                    if ui.selectable_label((task.recognition.background_noise - 0.50).abs() < 0.05, tr(language, "Standard (0.50 - Pure Speech)")).clicked() {
                        task.recognition.background_noise = 0.50;
                        task_settings_changed = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Custom Threshold:"))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                    );
                    let slider = egui::Slider::new(&mut task.recognition.background_noise, 0.02..=0.95)
                        .show_value(true)
                        .fixed_decimals(2);
                    if ui.add_sized([160.0, 18.0], slider).changed() {
                        task_settings_changed = true;
                    }
                });

                ui.add_space(12.0);

                // Pause Tolerance & Sentence Continuity
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Pause Tolerance & Sentence Continuity"))
                            .size(13.5)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );

                    let p_val = task.recognition.pause_tolerance;
                    let p_ms = (240.0 + p_val.clamp(0.0, 1.0) * 960.0).round() as u32;
                    let p_label = if p_val <= 0.35 {
                        tr(language, "Quick / Short (0.30 - Fast Turnaround)")
                    } else if p_val <= 0.70 {
                        tr(language, "Balanced (0.60 - Standard)")
                    } else {
                        tr(language, "Continuous (1.00 - Anti-Fragmentation)")
                    };

                    ui.label(
                        egui::RichText::new(format!("• {p_label} ({p_ms} ms)"))
                            .size(12.0)
                            .color(crate::ui::theme::primary())
                            .strong(),
                    );
                });

                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "Higher values prevent broken sentences during natural pauses, breathing, or background music; lower values output subtitles with shorter turnarounds.",
                    ))
                    .size(11.5)
                    .color(crate::ui::theme::text_weak()),
                );

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.selectable_label((task.recognition.pause_tolerance - 0.30).abs() < 0.05, tr(language, "Quick / Short (0.30 - 528ms)")).clicked() {
                        task.recognition.pause_tolerance = 0.30;
                        task_settings_changed = true;
                    }
                    if ui.selectable_label((task.recognition.pause_tolerance - 0.60).abs() < 0.05, tr(language, "Balanced (0.60 - 816ms)")).clicked() {
                        task.recognition.pause_tolerance = 0.60;
                        task_settings_changed = true;
                    }
                    if ui.selectable_label((task.recognition.pause_tolerance - 1.00).abs() < 0.05, tr(language, "Continuous (1.00 - 1200ms)")).clicked() {
                        task.recognition.pause_tolerance = 1.00;
                        task_settings_changed = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Custom Interval:"))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                    );
                    let slider = egui::Slider::new(&mut task.recognition.pause_tolerance, 0.0..=1.0)
                        .show_value(true)
                        .fixed_decimals(2);
                    if ui.add_sized([160.0, 18.0], slider).changed() {
                        task_settings_changed = true;
                    }
                });
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(12.0);

            // Row 3: Multi-channel Routing & Audio Track Separation
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Channel Routing & Separation"))
                            .size(13.5)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );

                    let ch_count = task.audio_channels.len();
                    let layout_text = match ch_count {
                        1 => "1.0 Mono",
                        2 => "2.0 Stereo",
                        3 => "2.1 Stereo + Subwoofer",
                        4 => "4.0 Quadraphonic",
                        5 => "5.0 Surround",
                        6 => "5.1 Surround",
                        8 => "7.1 Surround",
                        _ => "Custom Multi-channel",
                    };

                    ui.label(
                        egui::RichText::new(format!("• {layout_text} ({ch_count} ch)"))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ch_count >= 5 {
                            if ui.button(tr(language, "Dialogue Only")).clicked() {
                                for ch in &mut task.audio_channels {
                                    if ch.id == "fc" {
                                        ch.playback = true;
                                        ch.recognition = true;
                                    } else {
                                        ch.playback = true;
                                        ch.recognition = false;
                                    }
                                }
                                routing_changed = true;
                                task_settings_changed = true;
                            }
                        }
                        if ui.button(tr(language, "Stereo Default")).clicked() {
                            for ch in &mut task.audio_channels {
                                if ch.is_left || ch.is_right || ch.is_center {
                                    ch.playback = true;
                                    ch.recognition = true;
                                } else {
                                    ch.playback = false;
                                    ch.recognition = false;
                                }
                            }
                            routing_changed = true;
                            task_settings_changed = true;
                        }
                        if ui.button(tr(language, "Enable All")).clicked() {
                            for ch in &mut task.audio_channels {
                                ch.playback = true;
                                ch.recognition = true;
                            }
                            routing_changed = true;
                            task_settings_changed = true;
                        }
                    });
                });

                ui.add_space(6.0);

                Frame::new()
                    .fill(Color32::from_rgb(248, 250, 252))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(226, 232, 240)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        egui::Grid::new("player_channels_matrix_grid")
                            .num_columns(3)
                            .spacing([16.0, 8.0])
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(tr(language, "Channel")).strong().size(12.0));
                                ui.label(egui::RichText::new(tr(language, "Playback (Hear Audio)")).strong().size(12.0));
                                ui.label(egui::RichText::new(tr(language, "Recognition (Send to ASR)")).strong().size(12.0));
                                ui.end_row();

                                for ch in &mut task.audio_channels {
                                    if ch.id == "fc" {
                                        ui.label(
                                            egui::RichText::new(format!("⭐ {}", ch.name))
                                                .color(crate::ui::theme::primary())
                                                .strong(),
                                        );
                                    } else {
                                        ui.label(&ch.name);
                                    }

                                    if ui.checkbox(&mut ch.playback, "").changed() {
                                        routing_changed = true;
                                    }
                                    if ui.checkbox(&mut ch.recognition, "").changed() {
                                        task_settings_changed = true;
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        });
    });

    if do_restart {
        if let Some(task) = controller.store.get(&active_id) {
            let source = task.source.clone();
            let source_language = task.source_language.clone();
            let target_language = task.target_language.clone();
            let recognition = task.recognition.clone();
            let audio_channels = task.audio_channels.clone();
            controller.clear_and_restart_task();
            match &source {
                MediaSource::LocalFile(path) => {
                    action = VideoPlayerAction::StartTranslation(
                        PlayerTranslationRequest::ImportMediaFile {
                            path: path.clone(),
                            source_language,
                            target_language,
                            recognition,
                            audio_channels,
                        },
                    );
                }
                MediaSource::NetworkStream(_) => {
                    action =
                        VideoPlayerAction::StartTranslation(PlayerTranslationRequest::LiveStream {
                            source_language,
                            target_language,
                            recognition,
                            audio_channels,
                        });
                }
            }
        }
    } else if do_pause {
        controller.pause_task();
        action = VideoPlayerAction::StopTranslation;
    } else if do_start {
        if let Some(task) = controller.store.get(&active_id) {
            let source = task.source.clone();
            let source_language = task.source_language.clone();
            let target_language = task.target_language.clone();
            let recognition = task.recognition.clone();
            let audio_channels = task.audio_channels.clone();
            controller.start_task();
            match &source {
                MediaSource::LocalFile(path) => {
                    action = VideoPlayerAction::StartTranslation(
                        PlayerTranslationRequest::ImportMediaFile {
                            path: path.clone(),
                            source_language,
                            target_language,
                            recognition,
                            audio_channels,
                        },
                    );
                }
                MediaSource::NetworkStream(_) => {
                    action =
                        VideoPlayerAction::StartTranslation(PlayerTranslationRequest::LiveStream {
                            source_language,
                            target_language,
                            recognition,
                            audio_channels,
                        });
                }
            }
        }
    } else if routing_changed {
        controller.apply_channel_routing();
    } else if task_settings_changed {
        let _ = controller.store.save_to_dir(&controller.storage_dir);
    }

    action
}
