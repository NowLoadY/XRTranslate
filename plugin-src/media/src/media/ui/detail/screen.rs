use super::{
    media::{render_audio_card, render_viewport_card},
    task_controls::render_task_control_card,
};
use crate::{
    media::{MediaAction, MediaUiSnapshot, controller::MediaController, i18n::tr},
    ui::{components, layout},
};
use eframe::egui;

pub(in crate::media::ui) fn render_detail(
    controller: &mut MediaController,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    if let Some(task) = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get(id))
    {
        ui_director::egui::record_location(ui, &task.id, &task.title);
    }
    let blocked = controller.pending_subtitle_change.is_some();
    let mut action = ui
        .add_enabled_ui(!blocked, |ui| {
            render_detail_contents(controller, snapshot, ui)
        })
        .inner;
    // Handle after rendering so text editors and focused controls get their keys first.
    if !blocked
        && controller.can_play()
        && !ui.ctx().text_edit_focused()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space))
    {
        controller.toggle_play();
    }
    if controller.subtitles.count() > 0
        && matches!(
            action,
            MediaAction::StartTranslation { .. }
                | MediaAction::TranslateSubtitles { restart: true }
        )
    {
        controller.pending_subtitle_change =
            Some(crate::media::files::SubtitleChange::Action(action));
        action = MediaAction::None;
    }
    let confirmed = super::files::confirm_change(controller, snapshot.language, ui.ctx());
    if confirmed != MediaAction::None {
        confirmed
    } else {
        action
    }
}

fn render_detail_contents(
    controller: &mut MediaController,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    let language = snapshot.language;
    let optimizing = controller.active_task_id.is_some()
        && snapshot.optimizing_task == controller.active_task_id;
    let mut action = MediaAction::None;
    if controller.fullscreen_mode
        && (!controller.can_show_video() || ui.input(|i| i.key_pressed(egui::Key::Escape)))
    {
        controller.fullscreen_mode = false;
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
    if !controller.fullscreen_mode {
        let mut export_video = false;
        let mut export_speech = false;
        let speech = controller
            .active_task_id
            .as_deref()
            .and_then(|id| controller.store.get(id))
            .and_then(|task| task.speech_track.as_ref())
            .map(|track| (track.path.clone(), track.language.clone(), track.rendered));
        let video_available = controller.video_export.is_none()
            && controller
                .active_task_id
                .as_deref()
                .and_then(|id| controller.store.get(id))
                .is_some_and(super::super::super::video_export::VideoExport::supported);
        let title = controller
            .active_task_id
            .as_deref()
            .and_then(|id| controller.store.get(id))
            .map(|task| task.title.clone())
            .unwrap_or_else(|| tr(language, "Media").to_owned());
        components::page_header(ui, &title, |ui| {
            if ui
                .add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
                    components::back_button(ui, tr(language, "Back to Library"))
                })
                .inner
                .on_disabled_hover_text(tr(language, "Save or cancel this edit first."))
                .clicked()
            {
                controller.open_library();
                action = MediaAction::StopTranslation;
            }
            if controller.subtitles.count() > 0 || speech.is_some() || video_available {
                components::icon_menu(
                    ui,
                    tr(language, "Export"),
                    xrtranslate_client_core::ui::icon!("export.svg"),
                    |ui| {
                        export_video = super::super::export_controls(
                            ui,
                            language,
                            &title,
                            &controller.subtitles,
                            video_available,
                        );
                        if let Some((path, _, ready)) = &speech {
                            export_speech = components::secondary_button_enabled(
                                ui,
                                tr(language, "Export speech track"),
                                *ready && path.is_file(),
                            )
                            .clicked();
                        }
                    },
                );
            }
        });
        if export_speech && let Some((speech_path, speech_language, _)) = speech {
            let stem = std::path::Path::new(&title)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let name = format!("{stem}-{speech_language}");
            let volume = controller
                .active_task_id
                .as_deref()
                .and_then(|id| controller.store.get(id))
                .map_or(100, |task| task.audio_mix.speech);
            #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
            if let Err(error) =
                crate::export::write_file(&name, crate::export::Format::Wav, move |output| {
                    if output
                        .canonicalize()
                        .ok()
                        .zip(speech_path.canonicalize().ok())
                        .is_some_and(|(a, b)| a == b)
                    {
                        return Err(
                            "Choose a different file to preserve the original audio.".into()
                        );
                    }
                    let temporary = tempfile::Builder::new()
                        .suffix(".wav")
                        .tempfile_in(output.parent().ok_or("Invalid output directory")?)
                        .map_err(|e| e.to_string())?
                        .into_temp_path();
                    super::super::super::backend::mpv::encode_audio_volume(
                        &speech_path,
                        &temporary,
                        volume,
                        &std::sync::atomic::AtomicBool::new(false),
                    )?;
                    temporary.persist(output).map_err(|e| e.to_string())?;
                    Ok(())
                })
            {
                controller.error = Some(error);
            }
            #[cfg(not(all(feature = "mpv", any(windows, target_os = "linux"))))]
            if let Err(error) = std::fs::read(&speech_path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| crate::export::save(&name, crate::export::Format::Wav, bytes))
            {
                controller.error = Some(error);
            }
            let _ = volume;
        }
        if export_video && let Some(id) = controller.active_task_id.clone() {
            controller.save_active();
            controller.pending_video_export = Some(id);
        }
    }
    if action == MediaAction::StopTranslation {
        return action;
    }
    if controller.is_subtitle_task() {
        let action = ui
            .add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
                render_task_control_card(controller, snapshot, ui)
            })
            .inner;
        ui.add_space(12.0);
        ui.push_id(controller.active_task_id.clone(), |ui| {
            super::super::subtitles::render_subtitles_card(
                controller,
                language,
                optimizing,
                snapshot.can_synthesize,
                ui,
            );
        });
        return action;
    }
    let height = ui.available_height();
    if controller.fullscreen_mode {
        render_viewport_card(controller, language, ui, height);
    } else {
        let stacked = layout::should_stack(ui.available_width(), 2, 380.0);
        if stacked
            && controller
                .subtitle_edit
                .as_ref()
                .is_some_and(|edit| !edit.dragging_time)
        {
            ui.push_id(controller.active_task_id.clone(), |ui| {
                super::super::subtitles::render_subtitles_card(
                    controller,
                    language,
                    optimizing,
                    snapshot.can_synthesize,
                    ui,
                );
            });
            return action;
        }
        if stacked {
            action = egui::ScrollArea::vertical()
                .id_salt("media_detail_stacked")
                .show(ui, |ui| {
                    render_player(controller, snapshot.language, ui, height * 0.35);
                    ui.add_space(12.0);
                    let list_height = if controller.subtitles.count() > 0 {
                        height * 0.5
                    } else {
                        0.0
                    };
                    ui.allocate_ui(egui::vec2(ui.available_width(), list_height), |ui| {
                        ui.push_id(controller.active_task_id.clone(), |ui| {
                            super::super::subtitles::render_subtitles_card(
                                controller,
                                language,
                                optimizing,
                                snapshot.can_synthesize,
                                ui,
                            );
                        });
                    });
                    ui.add_space(12.0);
                    ui.add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
                        render_task_control_card(controller, snapshot, ui)
                    })
                    .inner
                })
                .inner;
        } else {
            ui.columns(2, |columns| {
                action = egui::ScrollArea::vertical()
                    .id_salt("media_detail_controls")
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(&mut columns[0], |ui| {
                        render_controls(controller, snapshot, ui, height * 0.5)
                    })
                    .inner;
                columns[1].push_id(controller.active_task_id.clone(), |ui| {
                    super::super::subtitles::render_subtitles_card(
                        controller,
                        language,
                        optimizing,
                        snapshot.can_synthesize,
                        ui,
                    );
                });
            });
        }
    }
    action
}

fn render_controls(
    controller: &mut MediaController,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
    video_height: f32,
) -> MediaAction {
    render_player(controller, snapshot.language, ui, video_height);
    ui.add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
        render_task_control_card(controller, snapshot, ui)
    })
    .inner
}

fn render_player(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
    video_height: f32,
) {
    if controller.can_play() {
        if controller.can_show_video() {
            render_viewport_card(controller, language, ui, video_height);
        } else {
            render_audio_card(controller, language, ui);
        }
        ui.add_space(8.0);
    }
}
