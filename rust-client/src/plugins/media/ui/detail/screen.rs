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
                    tr(language, "Export Subtitles"),
                    egui::include_image!("../../../../../resources/icons/export.svg"),
                    |ui| super::super::export_controls(ui, language, &title, &controller.subtitles),
                );
            }
        });
    }
    if action == MediaAction::StopTranslation {
        return action;
    }
    let height = ui.available_height();
    if controller.fullscreen_mode {
        render_viewport_card(controller, language, ui, height);
    } else {
        let stacked = layout::should_stack(ui.available_width(), 2, 380.0);
        layout::responsive_columns(ui, 2, 380.0, |ui, column| {
            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
            if column == 0 {
                egui::ScrollArea::vertical()
                    .id_salt("media_detail_controls")
                    .max_width(ui.available_width())
                    .max_height(if stacked { height * 0.35 } else { height })
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if controller.can_play() {
                            if controller.can_show_video() {
                                let video_height = height * if stacked { 0.25 } else { 0.5 };
                                render_viewport_card(controller, language, ui, video_height);
                            } else {
                                render_audio_card(controller, language, ui);
                            }
                            ui.add_space(8.0);
                        }
                        let next = ui
                            .add_enabled_ui(controller.subtitle_edit.is_none(), |ui| {
                                render_task_control_card(controller, snapshot, ui)
                            })
                            .inner;
                        if next != MediaAction::None {
                            action = next;
                        }
                    });
            } else {
                ui.push_id(controller.active_task_id.clone(), |ui| {
                    super::super::subtitles::render_subtitles_card(controller, language, ui);
                });
            }
        });
    }
    action
}
