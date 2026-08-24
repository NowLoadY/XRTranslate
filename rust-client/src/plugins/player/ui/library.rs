use super::{format_time_ms, format_timestamp_date, render_runtime_install_banner};
use crate::plugins::player::{
    VideoPlayerAction, controller::VideoPlayerController, i18n::tr, task::MediaType,
};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin};

pub(super) fn render_library(
    controller: &mut VideoPlayerController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> VideoPlayerAction {
    let mut action = VideoPlayerAction::None;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(tr(language, "Video Tasks"))
                .size(22.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::primary_button(ui, tr(language, "New Video")).clicked() {
                controller.open_create();
            }
        });
    });

    ui.add_space(14.0);

    render_runtime_install_banner(controller, language, ui);

    if let Some(error) = &controller.error {
        components::danger_alert(ui, error);
        ui.add_space(10.0);
    }

    components::search_bar(
        ui,
        &mut controller.search_query,
        tr(language, "Search videos..."),
    );
    ui.add_space(12.0);

    let search_lower = controller.search_query.trim().to_lowercase();
    let filtered_tasks: Vec<_> = controller
        .store
        .tasks
        .iter()
        .filter(|task| {
            if search_lower.is_empty() {
                true
            } else {
                task.title.to_lowercase().contains(&search_lower)
            }
        })
        .cloned()
        .collect();

    if filtered_tasks.is_empty() {
        components::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(
                    egui::RichText::new(tr(language, "No video tasks yet"))
                        .size(17.0)
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "Create a new video playback and translation task to get started.",
                    ))
                    .size(13.0)
                    .color(crate::ui::theme::text_weak()),
                );
                ui.add_space(18.0);
                if components::primary_button(ui, tr(language, "New Video")).clicked() {
                    controller.open_create();
                }
                ui.add_space(24.0);
            });
        });
        return action;
    }

    let mut task_to_play = None;
    let mut task_to_delete = None;
    let mut srt_to_export: Option<(String, String)> = None;
    let mut lrc_to_export: Option<(String, String)> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for task in &filtered_tasks {
                components::card(ui, |ui| {
                    ui.vertical(|ui| {
                        // 1. Header Row (Badge + Title + Metadata)
                        ui.horizontal(|ui| {
                            let (badge_bg, badge_fg, badge_text) =
                                match (&task.source, task.media_type) {
                                    (super::super::backend::MediaSource::NetworkStream(_), _) => (
                                        Color32::from_rgb(236, 253, 245),
                                        Color32::from_rgb(5, 150, 105),
                                        tr(language, "STREAM"),
                                    ),
                                    (_, MediaType::AudioOnly) => (
                                        Color32::from_rgb(243, 232, 255),
                                        Color32::from_rgb(147, 51, 234),
                                        tr(language, "AUDIO FILE"),
                                    ),
                                    (_, MediaType::Video) => (
                                        Color32::from_rgb(238, 242, 255),
                                        Color32::from_rgb(79, 70, 229),
                                        tr(language, "VIDEO FILE"),
                                    ),
                                };

                            Frame::new()
                                .fill(badge_bg)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(6, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(badge_text)
                                            .color(badge_fg)
                                            .strong()
                                            .size(11.0),
                                    );
                                });

                            ui.add_space(8.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(&task.title)
                                        .size(16.0)
                                        .strong()
                                        .color(crate::ui::theme::text_strong()),
                                );
                                ui.add_space(3.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} • {} {} • {} → {} • {}",
                                        format_time_ms(task.duration_ms),
                                        task.subtitles.count(),
                                        tr(language, "Subtitles Count"),
                                        task.source_language.to_uppercase(),
                                        task.target_language.to_uppercase(),
                                        format_timestamp_date(task.created_at_sec)
                                    ))
                                    .size(12.0)
                                    .color(crate::ui::theme::text_weak()),
                                );
                            });
                        });

                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // 2. Action Buttons Row
                        ui.horizontal_wrapped(|ui| {
                            if components::primary_button_with_id(
                                ui,
                                ("play", task.created_at_sec, task.title.as_str()),
                                tr(language, "Play"),
                            )
                            .clicked()
                            {
                                task_to_play = Some(task.clone());
                            }

                            ui.add_space(6.0);

                            if task.subtitles.count() > 0 {
                                let stem = task
                                    .title
                                    .trim_end_matches(".mp3")
                                    .trim_end_matches(".wav")
                                    .trim_end_matches(".flac")
                                    .trim_end_matches(".m4a")
                                    .trim_end_matches(".mp4")
                                    .trim_end_matches(".mkv")
                                    .to_string();

                                if components::animated_button_with_id(
                                    ui,
                                    ("export_lrc", task.created_at_sec, task.title.as_str()),
                                    tr(language, "Export LRC"),
                                )
                                .clicked()
                                {
                                    lrc_to_export = Some((
                                        format!("{}.lrc", stem),
                                        task.subtitles.export_lrc(Some(&stem)),
                                    ));
                                }
                                ui.add_space(4.0);
                                if components::animated_button_with_id(
                                    ui,
                                    ("export_srt", task.created_at_sec, task.title.as_str()),
                                    tr(language, "Export SRT"),
                                )
                                .clicked()
                                {
                                    srt_to_export = Some((
                                        format!("{}.srt", stem),
                                        task.subtitles.export_srt(),
                                    ));
                                }
                                ui.add_space(6.0);
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if components::danger_button_with_id(
                                        ui,
                                        ("delete", task.created_at_sec, task.title.as_str()),
                                        tr(language, "Delete"),
                                    )
                                    .clicked()
                                    {
                                        task_to_delete = Some(task.id.clone());
                                    }
                                },
                            );
                        });
                    });
                });
                ui.add_space(10.0);
            }
        });

    if let Some(task) = task_to_play {
        if controller.backend.is_none() && !controller.try_init_backend() {
            controller.error = Some(
                tr(
                    language,
                    "Please download and install the player runtime first",
                )
                .into(),
            );
        } else if let Ok(_) = controller.play_task(&task.id) {
            action = VideoPlayerAction::None;
        }
    }

    if let Some(id) = task_to_delete {
        let was_active = controller.active_task_id.as_deref() == Some(&id);
        controller.delete_task(&id);
        if was_active {
            action = VideoPlayerAction::StopTranslation;
        }
    }

    if let Some((default_name, srt)) = srt_to_export {
        if let Some(save_path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Subtitles", &["srt"])
            .save_file()
        {
            let _ = std::fs::write(save_path, srt);
        }
    }

    if let Some((default_name, lrc)) = lrc_to_export {
        if let Some(save_path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Lyrics", &["lrc"])
            .save_file()
        {
            let _ = std::fs::write(save_path, lrc);
        }
    }

    action
}
