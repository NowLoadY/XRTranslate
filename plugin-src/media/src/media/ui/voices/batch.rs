//! Project-wide dubbing belongs beside its timeline; each button previews its scope.
use super::*;
use crate::media::{backend::MediaSource, speech::Scope};

pub(in crate::media::ui) fn toolbar(
    controller: &mut MediaController,
    language: UiLanguage,
    optimizing: bool,
    can_synthesize: bool,
    can_clone: bool,
    ui: &mut egui::Ui,
) -> Option<Scope> {
    let task_id = controller.active_task_id.as_deref()?;
    let task = controller.store.get(task_id)?;
    if task.subtitles.count() == 0
        || !matches!(
            task.source,
            MediaSource::LocalFile(_) | MediaSource::OnlineFile(_)
        )
        || (!can_synthesize && task.speech_track.is_none())
    {
        return None;
    }
    let job = controller
        .speech_job
        .as_ref()
        .filter(|job| job.task_id == task.id);
    let queued = task
        .subtitles
        .cues()
        .iter()
        .any(|cue| controller.cue_speech(&cue.id).is_some());
    let ready = can_synthesize
        && !task.is_task_running
        && !optimizing
        && controller.subtitle_edit.is_none()
        && controller.speech_edit.is_none()
        && controller.speech_job.is_none()
        && task.subtitles.cues().iter().all(|cue| {
            cue.translated_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        });
    let source_available = can_clone
        && (!task.speech_voices.is_empty()
            || task
                .separated_audio
                .as_ref()
                .is_some_and(|stems| stems.vocals.is_file()));
    let source_voice = if task.speech_track.is_some() {
        task.speech_use_source_voice
    } else {
        source_available
    };
    let mut hovered = None;
    let mut generate = None;
    let mut stop = false;
    crate::ui::layout::flow_row(ui, |ui| {
        ui.weak(tr(language, "Speech track"));
        if job.is_some() || queued {
            ui.spinner();
            if let Some(job) = job {
                ui.add(
                    egui::ProgressBar::new(job.progress())
                        .desired_width(96.0)
                        .show_percentage(),
                )
                .on_hover_text(tr(
                    language,
                    if job.is_waiting() {
                        "Waiting for service"
                    } else {
                        "Generating speech"
                    },
                ));
            } else {
                ui.weak(tr(language, "Queued"));
            }
            stop = components::icon_button(
                ui,
                tr(language, "Stop project dubbing"),
                xrtranslate_client_core::ui::icon!("stop.svg"),
            )
            .clicked();
        } else {
            for scope in [Scope::Missing, Scope::Changed] {
                let count = task
                    .subtitles
                    .cues()
                    .iter()
                    .filter(|cue| scope.contains(task, cue))
                    .count();
                if count == 0 {
                    continue;
                }
                let label = tr(
                    language,
                    match scope {
                        Scope::Missing if task.speech_track.is_none() => "Dub all · {count}",
                        Scope::Missing => "Dub missing · {count}",
                        _ => "Update changed · {count}",
                    },
                )
                .replace("{count}", &count.to_string());
                let response = components::secondary_button_enabled(
                    ui,
                    &label,
                    ready && (!source_voice || source_available),
                );
                if response.hovered() {
                    hovered = Some(scope);
                }
                if response.clicked() {
                    generate = Some((scope, source_voice, false));
                }
            }
            ui.add_enabled_ui(ready, |ui| {
                components::icon_menu(
                    ui,
                    tr(language, "All speech"),
                    xrtranslate_client_core::ui::icon!("more.svg"),
                    |ui| {
                        for (label, source, reset_voice) in [
                            ("Redub all", source_voice, false),
                            ("Dub all with original voices", true, true),
                            ("Dub all with TTS Center voice", false, true),
                        ] {
                            if (source && reset_voice && !source_available)
                                || (!reset_voice && task.speech_track.is_none())
                            {
                                continue;
                            }
                            let response = components::secondary_button_enabled(
                                ui,
                                tr(language, label),
                                !source || source_available,
                            );
                            if response.hovered() {
                                hovered = Some(Scope::All);
                            }
                            if response.clicked() {
                                generate = Some((Scope::All, source, reset_voice));
                                ui.close();
                            }
                        }
                    },
                );
            });
        }
    });
    if let Some((scope, source, reset_voice)) = generate {
        controller
            .store
            .get_mut(controller.active_task_id.as_deref().unwrap())
            .unwrap()
            .speech_use_source_voice = source;
        if reset_voice {
            controller.start_speech(true);
        } else {
            controller.start_speech_scope(scope, false);
        }
    }
    if stop {
        controller.cancel_project_speech();
    }
    hovered
}
