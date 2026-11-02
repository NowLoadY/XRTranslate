use super::{
    media::{render_audio_card, render_viewport_card},
    task_controls::render_task_control_card,
};
use crate::{
    plugins::media::{MediaAction, MediaUiSnapshot, controller::MediaController, i18n::tr},
    ui::{components, layout},
};
use eframe::egui;

pub(in crate::plugins::media::ui) fn render_detail(
    controller: &mut MediaController,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    let language = snapshot.language;
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
            if controller.subtitles.count() > 0 {
                components::icon_menu(
                    ui,
                    tr(language, "Export"),
                    egui::include_image!("../../../../../resources/icons/export.svg"),
                    |ui| {
                        export_video = super::super::export_controls(
                            ui,
                            language,
                            &title,
                            &controller.subtitles,
                            video_available,
                        );
                    },
                );
            }
        });
        if export_video && let Some(id) = controller.active_task_id.clone() {
            controller.start_video_export(&id, ui.ctx());
        }
    }
    if action == MediaAction::StopTranslation {
        return action;
    }
    let height = ui.available_height();
    if controller.fullscreen_mode {
        render_viewport_card(controller, language, ui, height);
    } else {
        let stacked = layout::should_stack(ui.available_width(), 2, 380.0);
        if stacked && controller.subtitle_edit.is_some() {
            ui.push_id(controller.active_task_id.clone(), |ui| {
                super::super::subtitles::render_subtitles_card(controller, language, ui);
            });
            return action;
        }
        if stacked {
            action = egui::ScrollArea::vertical()
                .id_salt("media_detail_stacked")
                .show(ui, |ui| {
                    let action = render_controls(controller, snapshot, ui, height * 0.35);
                    ui.add_space(12.0);
                    ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.65), |ui| {
                        ui.push_id(controller.active_task_id.clone(), |ui| {
                            super::super::subtitles::render_subtitles_card(
                                controller, language, ui,
                            );
                        });
                    });
                    action
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
                    super::super::subtitles::render_subtitles_card(controller, language, ui);
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
    if controller.can_play() {
        if controller.can_show_video() {
            render_viewport_card(controller, snapshot.language, ui, video_height);
        } else {
            render_audio_card(controller, snapshot.language, ui);
        }
        ui.add_space(8.0);
    }
    ui.add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
        render_task_control_card(controller, snapshot, ui)
    })
    .inner
}
