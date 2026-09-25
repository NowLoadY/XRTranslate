use super::super::subtitles::render_subtitles_card;
use super::{
    media::{render_audio_card, render_viewport_card},
    task_controls::render_task_control_card,
};
use crate::plugins::player::{VideoPlayerAction, VideoPlayerUiSnapshot, controller::VideoPlayerController, i18n::tr};
use crate::ui::components;
use eframe::egui;

pub(in crate::plugins::player::ui) fn render_player(
    controller: &mut VideoPlayerController,
    snapshot: &VideoPlayerUiSnapshot,
    ui: &mut egui::Ui,
) -> VideoPlayerAction {
    let language = snapshot.language;
    let mut action = VideoPlayerAction::None;
    let is_audio_only = controller.is_audio_only_task();

    // Handle ESC key to exit fullscreen
    if controller.fullscreen_mode && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        controller.fullscreen_mode = false;
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }

    if !controller.fullscreen_mode {
        ui.horizontal(|ui| {
            if components::animated_button(ui, tr(language, "Back to Library")).clicked() {
                controller.open_library();
                action = VideoPlayerAction::StopTranslation;
            }

            ui.add_space(8.0);
            let title = if let Some(src) = &controller.current_source {
                src.display_title()
            } else {
                tr(language, "Video Player").to_string()
            };

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if components::animated_button(ui, tr(language, "New Video")).clicked() {
                    controller.open_create();
                    action = VideoPlayerAction::StopTranslation;
                }

                if controller.subtitles.count() > 0 {
                    let base_title = controller
                        .current_source
                        .as_ref()
                        .map(|s| s.display_title())
                        .unwrap_or_else(|| "media".to_string());

                    let stem = base_title
                        .trim_end_matches(".mp3")
                        .trim_end_matches(".wav")
                        .trim_end_matches(".flac")
                        .trim_end_matches(".m4a")
                        .trim_end_matches(".mp4")
                        .trim_end_matches(".mkv");

                    ui.add_space(8.0);
                    if components::animated_button(ui, tr(language, "Export LRC")).clicked() {
                        let lrc_name = format!("{}.lrc", stem);
                        if let Some(save_path) = rfd::FileDialog::new()
                            .set_file_name(&lrc_name)
                            .add_filter("Lyrics", &["lrc"])
                            .save_file()
                        {
                            let _ = std::fs::write(
                                save_path,
                                controller.subtitles.export_lrc(Some(stem)),
                            );
                        }
                    }

                    ui.add_space(8.0);
                    if components::animated_button(ui, tr(language, "Export SRT")).clicked() {
                        let srt_name = format!("{}.srt", stem);
                        if let Some(save_path) = rfd::FileDialog::new()
                            .set_file_name(&srt_name)
                            .add_filter("Subtitles", &["srt"])
                            .save_file()
                        {
                            let _ = std::fs::write(save_path, controller.subtitles.export_srt());
                        }
                    }
                }

                ui.add_space(12.0);

                let title_width = ui.available_width().max(60.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add_sized(
                        [title_width, 28.0],
                        egui::Label::new(
                            egui::RichText::new(title)
                                .size(17.0)
                                .color(crate::ui::theme::text_strong())
                                .strong(),
                        )
                        .truncate(),
                    );
                });
            });
        });

        ui.add_space(10.0);
    }

    if let Some(error) = &controller.error {
        components::error_notice(ui, language, error);
        ui.add_space(10.0);
    }

    if controller.fullscreen_mode && !is_audio_only {
        render_viewport_card(controller, language, ui);
    } else {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if is_audio_only {
                    render_audio_card(controller, language, ui);
                } else {
                    render_viewport_card(controller, language, ui);
                }
                let task_action = render_task_control_card(controller, snapshot, ui);
                if task_action != VideoPlayerAction::None {
                    action = task_action;
                }
                render_subtitles_card(controller, language, ui);
                ui.add_space(16.0);
            });
    }

    action
}
