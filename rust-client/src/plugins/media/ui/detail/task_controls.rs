use crate::{
    plugins::media::{
        MediaAction, MediaTranslationRequest, MediaUiSnapshot, backend::MediaSource,
        controller::MediaController, i18n::tr,
    },
    ui::components,
};
use eframe::egui;

pub(super) fn render_task_control_card(
    controller: &mut MediaController,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    let Some(id) = controller.active_task_id.clone() else {
        return MediaAction::None;
    };
    let subtitles = controller.is_subtitle_task();
    let can_play = controller.can_play();
    let language = snapshot.language;
    let capabilities = if subtitles {
        snapshot.languages.for_text()
    } else {
        snapshot.languages
    };
    let count = controller.subtitles.count();
    let translated = controller
        .subtitles
        .cues()
        .iter()
        .filter(|cue| {
            cue.translated_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .count();
    let Some(task) = controller.store.get_mut(&id) else {
        return MediaAction::None;
    };
    let has_audio = task.audio_channels.is_empty()
        || task
            .audio_channels
            .iter()
            .any(|channel| channel.recognition);
    let mut start = false;
    let mut restart = false;
    let mut pause = false;
    let mut changed = false;
    let mut language_changed = false;
    let mut routing = false;
    components::card(ui, |ui| {
        ui.add_enabled_ui(!task.is_task_running, |ui| {
            language_changed = components::translation_language_selector(
                ui,
                "media_languages",
                &mut task.source_language,
                &mut task.target_language,
                capabilities,
                language,
            );
        });
        ui.add_space(10.0);
        crate::ui::layout::flow_row(ui, |ui| {
            if task.is_task_running {
                ui.spinner();
                pause = components::secondary_button(ui, tr(language, "Pause")).clicked();
            } else {
                start = components::primary_button_enabled(
                    ui,
                    tr(
                        language,
                        if subtitles && translated == count {
                            "Translation complete"
                        } else if subtitles {
                            "Translate subtitles"
                        } else {
                            "Start Task"
                        },
                    ),
                    (!subtitles || translated < count)
                        && has_audio
                        && capabilities
                            .select(&task.source_language, &task.target_language)
                            .is_ok(),
                )
                .clicked();
                if count > 0 {
                    ui.menu_button("…", |ui| {
                        if ui
                            .add_enabled(
                                has_audio,
                                egui::Button::new(tr(language, "Clear & Restart")),
                            )
                            .clicked()
                        {
                            restart = true;
                            ui.close();
                        }
                    });
                }
            }
            if subtitles {
                ui.weak(format!("{translated} / {count}"));
            } else {
                ui.weak(format!("{} · {count}", tr(language, "Subtitles Count")));
            }
        });
        if subtitles {
            ui.add(
                egui::ProgressBar::new(if count == 0 {
                    0.0
                } else {
                    translated as f32 / count as f32
                })
                .show_percentage(),
            );
        } else {
            if task.is_task_running {
                let progress = if controller.is_extracting {
                    controller.extraction_progress
                } else {
                    controller.recognition_progress
                };
                ui.label(tr(
                    language,
                    if controller.is_extracting {
                        "Audio Extraction"
                    } else {
                        "Speech Recognition & Subtitles"
                    },
                ));
                ui.add(
                    egui::ProgressBar::new(progress.unwrap_or(0.0))
                        .show_percentage()
                        .animate(progress.is_none()),
                );
            }
            egui::CollapsingHeader::new(tr(language, "Recognition Settings")).show(ui, |ui| {
                ui.add_enabled_ui(!task.is_task_running, |ui| {
                    changed |= components::ModernSlider::new(
                        tr(language, "VAD Sensitivity"),
                        &mut task.recognition.background_noise,
                        0.02..=0.95,
                        0.6,
                    )
                    .show(ui)
                    .changed();
                    changed |= components::ModernSlider::new(
                        tr(language, "Pause Tolerance & Sentence Continuity"),
                        &mut task.recognition.pause_tolerance,
                        0.0..=1.0,
                        1.0,
                    )
                    .show(ui)
                    .changed();
                    changed |= ui
                        .checkbox(
                            &mut task.recognition.continuous_recognition,
                            crate::i18n::tr(language, "Continuous recognition"),
                        )
                        .changed();
                });
            });
            if !task.audio_channels.is_empty() {
                egui::CollapsingHeader::new(tr(language, "Channel Routing & Separation")).show(
                    ui,
                    |ui| {
                        crate::ui::layout::flow_row(ui, |ui| {
                            for (label, mode) in [
                                ("Movie Dialogue Preset (5.1/7.1)", 0),
                                ("Stereo Default", 1),
                                ("Enable All", 2),
                            ] {
                                if mode == 0
                                    && !task.audio_channels.iter().any(|channel| channel.id == "fc")
                                {
                                    continue;
                                }
                                if ui
                                    .add_enabled(
                                        !task.is_task_running,
                                        egui::Button::new(tr(language, label)),
                                    )
                                    .clicked()
                                {
                                    for channel in &mut task.audio_channels {
                                        channel.recognition = match mode {
                                            0 => channel.id == "fc",
                                            1 => {
                                                channel.is_left
                                                    || channel.is_right
                                                    || channel.is_center
                                            }
                                            _ => true,
                                        };
                                        channel.playback = mode != 1 || channel.recognition;
                                    }
                                    changed = true;
                                    routing = true;
                                }
                            }
                        });
                        egui::Grid::new("media_channels").show(ui, |ui| {
                            ui.label(tr(language, "Channel"));
                            if can_play {
                                ui.label(tr(language, "Playback (Hear Audio)"));
                            }
                            ui.label(tr(language, "Recognition (Send to ASR)"));
                            ui.end_row();
                            for channel in &mut task.audio_channels {
                                ui.label(&channel.name);
                                if can_play {
                                    routing |= ui.checkbox(&mut channel.playback, "").changed();
                                }
                                changed |= ui
                                    .add_enabled(
                                        !task.is_task_running,
                                        egui::Checkbox::without_text(&mut channel.recognition),
                                    )
                                    .changed();
                                ui.end_row();
                            }
                        });
                    },
                );
            }
        }
    });
    let request = if (start || restart) && !subtitles {
        Some(match &task.source {
            MediaSource::LocalFile(path) => MediaTranslationRequest::ImportMediaFile {
                path: path.clone(),
                source_language: task.source_language.clone(),
                target_language: task.target_language.clone(),
                recognition: task.recognition.clone(),
                audio_channels: task.audio_channels.clone(),
            },
            MediaSource::NetworkStream(_) => MediaTranslationRequest::LiveStream {
                source_language: task.source_language.clone(),
                target_language: task.target_language.clone(),
                recognition: task.recognition.clone(),
                audio_channels: task.audio_channels.clone(),
            },
        })
    } else {
        None
    };
    if routing {
        controller.apply_channel_routing();
    }
    if subtitles && language_changed {
        controller.subtitles.clear_translations();
    }
    if changed || language_changed {
        controller.save_active();
    }
    if pause {
        MediaAction::StopTranslation
    } else if start || restart {
        if subtitles {
            MediaAction::TranslateSubtitles { restart }
        } else {
            MediaAction::StartTranslation {
                request: request.unwrap(),
                restart,
            }
        }
    } else {
        MediaAction::None
    }
}
