use crate::{
    media::{
        MediaAction, MediaUiSnapshot, backend::MediaSource, controller::MediaController, i18n::tr,
        task::DEFAULT_PAUSE_TOLERANCE,
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
    let standalone = controller.is_subtitle_task();
    let mut subtitles = controller
        .store
        .get(&id)
        .is_some_and(|task| task.translates_subtitles());
    let mut mode_changed = false;
    if !standalone && (controller.subtitles.count() > 0 || subtitles) {
        let idle = controller
            .store
            .get(&id)
            .is_some_and(|task| !task.is_task_running)
            && controller.subtitle_edit.is_none();
        mode_changed = ui
            .add_enabled_ui(idle, |ui| {
                components::segmented_switch(
                    ui,
                    &mut subtitles,
                    [
                        tr(snapshot.language, "Audio"),
                        tr(snapshot.language, "Subtitles"),
                    ],
                )
                .changed()
            })
            .inner;
        ui.add_space(8.0);
    }
    let can_play = controller.can_play();
    let language = snapshot.language;
    let optimizing = snapshot.optimizing_task.as_deref() == Some(&id);
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
    let mut recognition = task.recognition.clone();
    let mut source_changed = false;
    if mode_changed {
        task.subtitle_mode = if subtitles {
            crate::media::task::SubtitleMode::TranslateExisting
        } else {
            crate::media::task::SubtitleMode::RealtimeTranslation
        };
    }
    let has_audio = subtitles
        || task.audio_channels.is_empty()
        || task
            .audio_channels
            .iter()
            .any(|channel| channel.recognition);
    let local_file = matches!(
        task.source,
        MediaSource::LocalFile(_) | MediaSource::OnlineFile(_)
    );
    let preprocessing = task
        .recognition
        .preprocessing
        .unwrap_or(snapshot.default_preprocessing);
    let model_ready = subtitles || snapshot.preprocessing_models.is_ready(preprocessing);
    let mut start = false;
    let mut restart = false;
    let mut repair = false;
    let mut pause = false;
    let mut changed = mode_changed;
    let mut language_changed = false;
    let mut routing = false;
    let mut preprocessing_action = None;
    let can_edit = controller.subtitle_edit.is_none();
    let speech_progress = controller
        .speech_job
        .as_ref()
        .filter(|job| job.task_id == id)
        .map(|job| (job.progress(), job.is_waiting()));
    let mut generate_speech = None;
    let mut cancel_speech = false;
    let card_top = ui.next_widget_position();
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        if optimizing {
            ui.label(tr(language, "Optimizing subtitles"));
        }
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
        ui.add_enabled_ui(!task.is_task_running, |ui| {
            crate::ui::layout::flow_row(ui, |ui| {
                ui.label(tr(language, "Subtitle character limit"))
                    .on_hover_text(tr(language, "Original and translation combined"));
                changed |= components::number_input(
                    ui,
                    &mut task.subtitle_char_limit,
                    tr(language, "Subtitle character limit"),
                    super::super::super::subtitles::SUBTITLE_CHAR_LIMIT_RANGE,
                )
                .changed();
                if snapshot.can_optimize && count > 0 {
                    repair = components::secondary_button_enabled(
                        ui,
                        tr(
                            language,
                            if optimizing {
                                "Stop optimization"
                            } else {
                                "Optimize subtitles"
                            },
                        ),
                        can_edit
                            && snapshot
                                .languages
                                .for_text()
                                .select(&task.source_language, &task.target_language)
                                .is_ok(),
                    )
                    .clicked();
                }
            });
        });
        if !subtitles {
            ui.add_enabled_ui(!task.is_task_running, |ui| {
                let response = components::audio_preprocessing_selector(
                    ui,
                    "media_audio_preprocessing",
                    &mut recognition.preprocessing,
                    snapshot.default_preprocessing,
                    local_file,
                    &snapshot.preprocessing_models,
                    language,
                );
                changed |= response.response.changed();
                preprocessing_action = response.action;
            });
        }
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
                        if subtitles && count > 0 && translated == count {
                            "Retranslate all"
                        } else if subtitles && translated > 0 {
                            "Continue translation"
                        } else if subtitles {
                            "Translate subtitles"
                        } else if count > 0 {
                            "Recognize again"
                        } else {
                            "Recognize and translate"
                        },
                    ),
                    (!subtitles || count > 0)
                        && has_audio
                        && model_ready
                        && capabilities
                            .select(&task.source_language, &task.target_language)
                            .is_ok(),
                )
                .clicked();
                restart = start && subtitles && count > 0 && translated == count;
            }
            if subtitles {
                ui.weak(format!("{translated} / {count}"));
            } else {
                ui.weak(format!("{} · {count}", tr(language, "Subtitles Count")));
            }
            if subtitles && !task.is_task_running && translated > 0 && translated < count {
                components::icon_menu(
                    ui,
                    tr(language, "Task actions"),
                    xrtranslate_client_core::ui::icon!("more.svg"),
                    |ui| {
                        if components::secondary_button_enabled(
                            ui,
                            tr(language, "Retranslate all"),
                            capabilities
                                .select(&task.source_language, &task.target_language)
                                .is_ok(),
                        )
                        .clicked()
                        {
                            restart = true;
                            ui.close();
                        }
                    },
                );
            }
        });
        if let Some((progress, waiting)) = speech_progress {
            crate::ui::layout::flow_row(ui, |ui| {
                ui.spinner();
                ui.label(tr(
                    language,
                    if waiting {
                        "Waiting for service"
                    } else {
                        "Generating speech"
                    },
                ));
                cancel_speech = components::secondary_button(ui, tr(language, "Cancel")).clicked();
            });
            ui.add(egui::ProgressBar::new(progress).show_percentage());
        } else if snapshot.can_synthesize && local_file && count > 0 {
            let enabled = !task.is_task_running
                && !optimizing
                && can_edit
                && translated == count
                && controller.speech_job.is_none();
            let partial = task.speech_track.as_ref().is_some_and(|track| {
                let completed = track.completed(task);
                completed > 0 && completed < count
            });
            crate::ui::layout::flow_row(ui, |ui| {
                if components::secondary_button_enabled(
                    ui,
                    tr(
                        language,
                        if partial {
                            "Continue speech track"
                        } else if task.speech_track.is_some() {
                            "Regenerate speech track"
                        } else {
                            "Generate speech track"
                        },
                    ),
                    enabled,
                )
                .on_disabled_hover_text(tr(language, "Finish translating the subtitles first."))
                .clicked()
                {
                    generate_speech = Some(!partial && task.speech_track.is_some());
                }
                if partial {
                    ui.add_enabled_ui(enabled, |ui| {
                        components::icon_menu(
                            ui,
                            tr(language, "Task actions"),
                            xrtranslate_client_core::ui::icon!("more.svg"),
                            |ui| {
                                if components::secondary_button(
                                    ui,
                                    tr(language, "Regenerate speech track"),
                                )
                                .clicked()
                                {
                                    generate_speech = Some(true);
                                    ui.close();
                                }
                            },
                        );
                    });
                }
            });
        }
        if subtitles && task.is_task_running {
            let progress = if count == 0 {
                0.0
            } else {
                translated as f32 / count as f32
            };
            ui.add(egui::ProgressBar::new(progress.min(0.99)).show_percentage());
        } else if !subtitles {
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
            crate::ui::components::CollapsingHeader::new(tr(language, "Recognition Settings"))
                .show(ui, |ui| {
                    ui.add_enabled_ui(!task.is_task_running, |ui| {
                        source_changed = super::audio_tracks::recognition_source(
                            task,
                            controller
                                .backend
                                .as_ref()
                                .map(|backend| backend.audio_tracks())
                                .unwrap_or_default(),
                            language,
                            ui,
                        );
                        if source_changed
                            && task.audio_track != crate::media::task::AudioTrackSelection::Original
                            && recognition.preprocessing
                                == Some(xrtranslate_protocol::AudioPreprocessing::VocalSeparation)
                        {
                            recognition.preprocessing =
                                Some(xrtranslate_protocol::AudioPreprocessing::None);
                        }
                        changed |= components::ModernSlider::new(
                            tr(language, "Filter strength"),
                            &mut recognition.background_noise,
                            0.02..=0.95,
                            0.6,
                        )
                        .show(ui)
                        .changed();
                        let pause_ms = crate::pause_tolerance_to_ms(recognition.pause_tolerance);
                        changed |= components::ModernSlider::new(
                            tr(language, "Pause tolerance"),
                            &mut recognition.pause_tolerance,
                            0.0..=1.0,
                            DEFAULT_PAUSE_TOLERANCE,
                        )
                        .suffix(format!(" ({pause_ms} ms)"))
                        .show(ui)
                        .changed();
                        if !local_file {
                            changed |= ui
                                .checkbox(
                                    &mut recognition.continuous_recognition,
                                    crate::i18n::tr(language, "Pseudo-streaming"),
                                )
                                .changed();
                        }
                    });
                });
            if !task.audio_channels.is_empty() {
                crate::ui::components::CollapsingHeader::new(tr(language, "Channel routing")).show(
                    ui,
                    |ui| {
                        crate::ui::layout::flow_row(ui, |ui| {
                            for (label, mode) in
                                [("Dialogue", 0), ("Stereo", 1), ("All channels", 2)]
                            {
                                if mode == 0
                                    && !task.audio_channels.iter().any(|channel| channel.id == "fc")
                                {
                                    continue;
                                }
                                if components::secondary_button_enabled(
                                    ui,
                                    tr(language, label),
                                    !task.is_task_running,
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
                                ui.label(tr(language, "Playback"));
                            }
                            ui.label(tr(language, "Recognition"));
                            ui.end_row();
                            for channel in &mut task.audio_channels {
                                let label = tr(language, channel.label());
                                ui.label(label);
                                if can_play {
                                    routing |= components::checkbox_labeled(
                                        ui,
                                        &mut channel.playback,
                                        "",
                                        &format!("{label}: {}", tr(language, "Playback")),
                                    )
                                    .changed();
                                }
                                changed |= ui
                                    .add_enabled_ui(!task.is_task_running, |ui| {
                                        components::checkbox_labeled(
                                            ui,
                                            &mut channel.recognition,
                                            "",
                                            &format!("{label}: {}", tr(language, "Recognition")),
                                        )
                                        .changed()
                                    })
                                    .inner;
                                ui.end_row();
                            }
                        });
                    },
                );
            }
        }
    });
    ui_director::egui::feedback::record(
        ui,
        super::super::settings_target(&id),
        egui::Rect::from_min_max(card_top, ui.min_rect().max),
    );
    let recognition_changed = task.recognition != recognition;
    if recognition_changed && let Err(error) = controller.set_task_recognition(&id, recognition) {
        controller.error = Some(error);
    }
    if source_changed {
        controller.apply_audio_selection(true);
    }
    let request = ((start || restart) && !subtitles)
        .then(|| controller.store.get(&id).unwrap().translation_request());
    if routing {
        controller.apply_channel_routing();
    }
    if subtitles && language_changed {
        controller.subtitles.clear_translations();
    }
    if changed || language_changed {
        controller.save_active();
    }
    if cancel_speech {
        controller.cancel_speech();
    }
    if pause {
        MediaAction::StopTranslation
    } else if let Some(restart) = generate_speech {
        MediaAction::GenerateSpeech { restart }
    } else if let Some(action) = preprocessing_action {
        MediaAction::AudioPreprocessing(action)
    } else if repair {
        if optimizing {
            MediaAction::StopOptimization
        } else {
            MediaAction::OptimizeSubtitles
        }
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
