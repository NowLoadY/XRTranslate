use crate::{
    plugins::media::{
        MediaAction, MediaTranslationRequest, MediaUiSnapshot, backend::MediaSource,
        controller::MediaController, i18n::tr, task::DEFAULT_PAUSE_TOLERANCE,
    },
    ui::components,
};
use eframe::egui;
use xrtranslate_protocol::AudioPreprocessing;

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
    let translation_progress = controller.translation_progress();
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
    let local_file = matches!(task.source, MediaSource::LocalFile(_));
    let preprocessing = task
        .recognition
        .preprocessing
        .unwrap_or(snapshot.default_preprocessing);
    let model_ready = subtitles || snapshot.preprocessing_models.is_ready(preprocessing);
    let mut start = false;
    let mut restart = false;
    let mut pause = false;
    let mut changed = false;
    let mut language_changed = false;
    let mut routing = false;
    let mut preprocessing_action = None;
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
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
                        && model_ready
                        && capabilities
                            .select(&task.source_language, &task.target_language)
                            .is_ok(),
                )
                .clicked();
            }
            if subtitles {
                ui.weak(format!("{translated} / {count}"));
            } else {
                ui.weak(format!("{} · {count}", tr(language, "Subtitles Count")));
            }
            if subtitles && !task.is_task_running && count > 0 {
                egui::menu::MenuButton::from_button(
                    egui::Button::new("…")
                        .frame(false)
                        .min_size(egui::vec2(28.0, 32.0)),
                )
                .ui(ui, |ui| {
                    if ui
                        .add_enabled(
                            has_audio,
                            egui::Button::new(tr(language, "Translate again")).frame(false),
                        )
                        .on_hover_text(tr(language, "Clear existing results and translate again."))
                        .clicked()
                    {
                        restart = true;
                        ui.close();
                    }
                })
                .0
                .on_hover_text(tr(language, "Task actions"));
            }
        });
        if subtitles {
            let progress = if count == 0 {
                0.0
            } else {
                translated as f32 / count as f32
            };
            ui.add(
                egui::ProgressBar::new(if task.is_task_running {
                    progress.min(0.99)
                } else {
                    progress
                })
                .show_percentage(),
            );
        } else {
            ui.add_enabled_ui(!task.is_task_running, |ui| {
                let response = components::audio_preprocessing_selector(
                    ui,
                    "media_audio_preprocessing",
                    &mut task.recognition.preprocessing,
                    snapshot.default_preprocessing,
                    local_file,
                    &snapshot.preprocessing_models,
                    language,
                );
                changed |= response.response.changed();
                preprocessing_action = response.action;
            });
            if task.is_task_running {
                let progress = if controller.is_extracting {
                    controller.extraction_progress
                } else {
                    translation_progress
                };
                ui.label(tr(
                    language,
                    if controller.is_extracting {
                        "Audio Extraction"
                    } else if controller
                        .recognition_progress
                        .is_some_and(|fraction| fraction >= 1.0)
                    {
                        "Finishing translation"
                    } else {
                        "Translating audio"
                    },
                ));
                let bar = egui::ProgressBar::new(progress.unwrap_or(0.0).min(0.99));
                ui.add(if progress.is_some() {
                    bar.show_percentage()
                } else {
                    bar.animate(true)
                });
            }
            egui::CollapsingHeader::new(tr(language, "Recognition Settings")).show(ui, |ui| {
                ui.add_enabled_ui(!task.is_task_running, |ui| {
                    if task
                        .recognition
                        .preprocessing
                        .unwrap_or(snapshot.default_preprocessing)
                        != AudioPreprocessing::VocalSeparation
                    {
                        changed |= components::ModernSlider::new(
                            tr(language, "VAD Sensitivity"),
                            &mut task.recognition.background_noise,
                            0.02..=0.95,
                            0.6,
                        )
                        .show(ui)
                        .changed();
                    }
                    let pause_ms = crate::pause_tolerance_to_ms(task.recognition.pause_tolerance);
                    changed |= components::ModernSlider::new(
                        tr(language, "Pause tolerance"),
                        &mut task.recognition.pause_tolerance,
                        0.0..=1.0,
                        DEFAULT_PAUSE_TOLERANCE,
                    )
                    .suffix(format!(" ({pause_ms} ms)"))
                    .show(ui)
                    .changed();
                    if !local_file {
                        changed |= ui
                            .checkbox(
                                &mut task.recognition.continuous_recognition,
                                crate::i18n::tr(language, "Pseudo-streaming"),
                            )
                            .changed();
                    }
                });
            });
            if !task.audio_channels.is_empty() {
                egui::CollapsingHeader::new(tr(language, "Channel routing")).show(ui, |ui| {
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
                                            channel.is_left || channel.is_right || channel.is_center
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
                });
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
    } else if let Some(action) = preprocessing_action {
        MediaAction::AudioPreprocessing(action)
    } else if start || restart {
        if subtitles {
            MediaAction::TranslateSubtitles { restart }
        } else {
            MediaAction::StartTranslation {
                request: request.unwrap(),
            }
        }
    } else {
        MediaAction::None
    }
}
