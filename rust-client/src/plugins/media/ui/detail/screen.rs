use super::{
    media::{render_audio_card, render_viewport_card},
    task_controls::render_task_control_card,
};
use crate::{
    plugins::media::{MediaAction, MediaUiSnapshot, controller::MediaController, i18n::tr},
    ui::components,
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
        crate::ui::layout::flow_row(ui, |ui| {
            if components::secondary_button(ui, tr(language, "Back to Library")).clicked() {
                controller.open_library();
                action = MediaAction::StopTranslation;
            }
            let title = controller
                .active_task_id
                .as_deref()
                .and_then(|id| controller.store.get(id))
                .map(|task| task.title.as_str())
                .unwrap_or("Media");
            ui.label(egui::RichText::new(title).size(18.0).strong());
            if controller.subtitles.count() > 0 {
                ui.menu_button(tr(language, "Export Subtitles"), |ui| {
                    super::super::export_controls(ui, language, title, &controller.subtitles)
                });
            }
        });
        ui.add_space(12.0);
    }
    if action == MediaAction::StopTranslation {
        return action;
    }
    if controller.fullscreen_mode {
        render_viewport_card(controller, language, ui);
    } else {
        egui::ScrollArea::vertical()
            .id_salt("media_detail_controls")
            .max_height((ui.available_height() - 160.0).max(100.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if controller.can_play() {
                    if !controller.can_show_video() {
                        render_audio_card(controller, language, ui);
                    } else {
                        render_viewport_card(controller, language, ui);
                    }
                }
                let next = render_task_control_card(controller, snapshot, ui);
                if next != MediaAction::None {
                    action = next;
                }
            });
        super::super::subtitles::render_subtitles_card(controller, language, ui);
    }
    action
}
