pub(super) mod batch;

use crate::{
    i18n::UiLanguage,
    media::{controller::MediaController, i18n::tr, subtitles::SubtitleCue, task::MediaTask},
    ui::{components, theme},
};
use eframe::egui;

#[derive(Clone)]
pub(super) struct VoicePayload {
    task: String,
    voice: String,
}

pub(super) enum Action {
    Generate { cue: String, voice: Option<String> },
    Preview { clip: String, previous: bool },
    Restore(String),
    Delete(String),
}

pub(super) struct Controls {
    pub generate: bool,
    pub assign: bool,
    pub pending: Option<crate::media::speech::CueSpeech>,
    pub playing: bool,
    pub restore: bool,
}

impl Action {
    pub(super) fn apply(self, controller: &mut MediaController) {
        match self {
            Self::Generate { cue, voice } => controller.start_cue_with_voice(cue, voice),
            Self::Preview { clip, previous } => controller.audition_speech_take(&clip, previous),
            Self::Restore(clip) => controller.restore_speech(&clip),
            Self::Delete(clip) => controller.delete_speech(&clip),
        }
    }
}

fn voice_name(task: &MediaTask, id: &str, language: UiLanguage) -> String {
    task.speech_voices
        .iter()
        .enumerate()
        .find(|(_, voice)| voice.id == id)
        .map(|(index, voice)| {
            if voice.name.is_empty() {
                format!("{} {}", tr(language, "Voice"), index + 1)
            } else {
                voice.name.clone()
            }
        })
        .unwrap_or_else(|| tr(language, "Original voice").into())
}

pub(super) fn shelf(
    controller: &mut MediaController,
    language: UiLanguage,
    can_clone: bool,
    ui: &mut egui::Ui,
) {
    let Some(task) = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get(id))
    else {
        return;
    };
    let has_vocals = task
        .separated_audio
        .as_ref()
        .is_some_and(|stems| stems.vocals.is_file());
    if task.speech_voices.is_empty() && !has_vocals {
        return;
    }
    let mut preview = None;
    let mut rename = None;
    let mut remove = None;
    let mut collect = None;
    let mut clear = false;
    let busy = controller.voice_library.busy();
    crate::ui::layout::flow_row(ui, |ui| {
        ui.weak(tr(language, "Project voices")).on_hover_text(tr(
            language,
            "Click to listen. Drag a voice onto a subtitle to dub it.",
        ));
        if busy {
            ui.spinner();
        }
        if let Some([start, end]) = task.speech_reference_range {
            if components::secondary_button_enabled(
                ui,
                tr(language, "Save selected voice"),
                !busy && end - start >= 5000,
            )
            .clicked()
            {
                collect = Some(true);
            }
            clear = components::icon_button(
                ui,
                tr(language, "Clear voice reference"),
                xrtranslate_client_core::ui::icon!("x.svg"),
            )
            .clicked();
        } else if task.speech_voices.is_empty()
            && components::secondary_button_enabled(
                ui,
                tr(language, "Extract voices"),
                !busy && task.subtitles.count() > 0,
            )
            .clicked()
        {
            collect = Some(false);
        }
    });
    if !task.speech_voices.is_empty() {
        egui::ScrollArea::horizontal()
            .id_salt(("voice_shelf", &task.id))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for voice in &task.speech_voices {
                        ui.push_id(&voice.id, |ui| {
                            let name = voice_name(task, &voice.id, language);
                            let response = ui.add(
                                egui::Button::new(&name)
                                    .corner_radius(18)
                                    .selected(controller.voice_library.previewing(&voice.id))
                                    .sense(if can_clone {
                                        egui::Sense::click_and_drag()
                                    } else {
                                        egui::Sense::click()
                                    }),
                            );
                            let response = ui_director::egui::button_response(
                                ui,
                                response.id,
                                &name,
                                true,
                                response,
                            )
                            .on_hover_text(tr(
                                language,
                                "Click to listen. Drag a voice onto a subtitle to dub it.",
                            ));
                            if can_clone {
                                response.dnd_set_drag_payload(VoicePayload {
                                    task: task.id.clone(),
                                    voice: voice.id.clone(),
                                });
                            }
                            if response.clicked() {
                                preview = Some(voice.id.clone());
                            }
                            response.context_menu(|ui| {
                                let state = ui.make_persistent_id("voice_name");
                                let mut value = ui
                                    .data_mut(|data| data.get_temp::<String>(state))
                                    .unwrap_or_else(|| name.clone());
                                components::text_edit_ui(
                                    ui,
                                    tr(language, "Voice name"),
                                    egui::TextEdit::singleline(&mut value).desired_width(160.0),
                                );
                                ui.data_mut(|data| data.insert_temp(state, value.clone()));
                                if components::secondary_button(ui, tr(language, "Rename"))
                                    .clicked()
                                {
                                    rename = Some((voice.id.clone(), value));
                                    ui.close();
                                }
                                let in_use = task.speech_track.as_ref().is_some_and(|track| {
                                    track.clips.iter().any(|clip| {
                                        [Some(clip), clip.previous.as_deref()]
                                            .into_iter()
                                            .flatten()
                                            .any(|clip| {
                                                clip.origin
                                                    .as_ref()
                                                    .and_then(|origin| origin.voice_id.as_deref())
                                                    == Some(&voice.id)
                                            })
                                    })
                                });
                                if components::secondary_button_enabled(
                                    ui,
                                    tr(language, "Remove voice"),
                                    !in_use && controller.speech_job.is_none(),
                                )
                                .clicked()
                                {
                                    remove = Some(voice.id.clone());
                                    ui.close();
                                }
                            });
                        });
                    }
                });
            });
    }
    ui.add_space(4.0);
    if let Some(id) = preview {
        controller.audition_project_voice(&id);
    }
    if let Some((id, name)) = rename {
        controller.rename_project_voice(&id, name);
    }
    if let Some(id) = remove {
        controller.remove_project_voice(&id);
    }
    if let Some(selected) = collect {
        controller.collect_project_voices(selected);
    }
    if clear {
        controller.set_vocal_reference(None);
    }
}

pub(super) fn drop_on(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    task: &MediaTask,
    cue: &SubtitleCue,
    enabled: bool,
) -> Option<Action> {
    if !enabled
        || cue
            .translated_text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
    {
        return None;
    }
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("voice_drop", &cue.id)),
        egui::Sense::hover(),
    );
    if response
        .dnd_hover_payload::<VoicePayload>()
        .is_some_and(|payload| payload.task == task.id)
    {
        ui.painter().rect_stroke(
            rect,
            10,
            egui::Stroke::new(2.0, theme::primary()),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    }
    response
        .dnd_release_payload::<VoicePayload>()
        .filter(|payload| payload.task == task.id)
        .map(|payload| Action::Generate {
            cue: cue.id.clone(),
            voice: Some(payload.voice.clone()),
        })
}

pub(super) fn footer(
    ui: &mut egui::Ui,
    task: &MediaTask,
    cue: &SubtitleCue,
    language: UiLanguage,
    controls: Controls,
) -> Option<Action> {
    let track = task.speech_track.as_ref();
    let clip = track.and_then(|track| track.clip_for_cue(cue));
    let current = track.is_some_and(|track| track.current(task, cue));
    let mut action = None;
    ui.horizontal(|ui| {
        let assigned = clip
            .and_then(|clip| clip.origin.as_ref())
            .and_then(|origin| origin.voice_id.as_deref());
        let name = assigned
            .map(|id| voice_name(task, id, language))
            .unwrap_or_else(|| {
                tr(
                    language,
                    if task.speech_use_source_voice {
                        "Original voice"
                    } else {
                        "TTS Center voice"
                    },
                )
                .into()
            });
        let can_assign = controls.assign && !task.speech_voices.is_empty();
        let response = ui
            .add_enabled_ui(can_assign, |ui| {
                ui.add_sized(
                    [124.0_f32.min(ui.available_width() * 0.32), 28.0],
                    egui::Button::new(egui::RichText::new(&name).size(13.0))
                        .corner_radius(14)
                        .truncate(),
                )
            })
            .inner
            .on_hover_text(&name);
        let response = ui_director::egui::button_response(
            ui,
            response.id,
            tr(language, "Choose voice"),
            can_assign,
            response,
        );
        egui::Popup::menu(&response).show(|ui| {
            ui.weak(tr(language, "Dub this subtitle with"));
            for voice in &task.speech_voices {
                if components::secondary_button_enabled(
                    ui,
                    &voice_name(task, &voice.id, language),
                    controls.assign,
                )
                .clicked()
                {
                    action = Some(Action::Generate {
                        cue: cue.id.clone(),
                        voice: Some(voice.id.clone()),
                    });
                    ui.close();
                }
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(clip) = clip {
                components::icon_menu(
                    ui,
                    tr(language, "Speech options"),
                    xrtranslate_client_core::ui::icon!("more.svg"),
                    |ui| {
                        if clip.previous.is_some() {
                            if components::secondary_button(
                                ui,
                                tr(language, "Listen to previous take"),
                            )
                            .clicked()
                            {
                                action = Some(Action::Preview {
                                    clip: clip.id.clone(),
                                    previous: true,
                                });
                                ui.close();
                            }
                            if components::secondary_button_enabled(
                                ui,
                                tr(language, "Restore previous take"),
                                controls.restore,
                            )
                            .clicked()
                            {
                                action = Some(Action::Restore(clip.id.clone()));
                                ui.close();
                            }
                        }
                        if components::secondary_button_enabled(
                            ui,
                            tr(language, "Delete this speech"),
                            controls.restore,
                        )
                        .clicked()
                        {
                            action = Some(Action::Delete(clip.id.clone()));
                            ui.close();
                        }
                    },
                );
            }
            if let Some(pending) = controls.pending {
                ui.weak(tr(
                    language,
                    match pending {
                        crate::media::speech::CueSpeech::Queued => "Queued",
                        crate::media::speech::CueSpeech::Generating => "Dubbing…",
                    },
                ));
                ui.spinner();
            } else {
                let label = tr(
                    language,
                    if current {
                        "Redub"
                    } else if clip.is_some() {
                        "Update dub"
                    } else {
                        "Dub"
                    },
                );
                let enabled = controls.generate
                    && cue
                        .translated_text
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty());
                let response = if clip.is_some() && !current {
                    components::primary_button_enabled(ui, label, enabled)
                } else {
                    components::secondary_button_enabled(ui, label, enabled)
                };
                if response.clicked() {
                    action = Some(Action::Generate {
                        cue: cue.id.clone(),
                        voice: None,
                    });
                }
            }
            if let Some(clip) = clip
                && components::icon_button(
                    ui,
                    tr(
                        language,
                        if controls.playing {
                            "Stop preview"
                        } else {
                            "Listen to speech"
                        },
                    ),
                    if controls.playing {
                        xrtranslate_client_core::ui::icon!("stop.svg")
                    } else {
                        xrtranslate_client_core::ui::icon!("play.svg")
                    },
                )
                .clicked()
            {
                action = Some(Action::Preview {
                    clip: clip.id.clone(),
                    previous: false,
                });
            }
        });
    });
    action
}
