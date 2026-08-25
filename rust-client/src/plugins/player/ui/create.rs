use super::render_runtime_install_banner;
use crate::plugins::player::{
    VideoPlayerAction,
    controller::VideoPlayerController,
    i18n::tr,
    task::{MediaType, detect_media_type},
};
use crate::ui::components;
use eframe::egui::{self, Color32};
use std::path::Path;

pub(super) fn render_create(
    controller: &mut VideoPlayerController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> VideoPlayerAction {
    let mut action = VideoPlayerAction::None;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(tr(language, "New Video Task"))
                .size(22.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::animated_button(ui, tr(language, "Back to Library")).clicked() {
                controller.open_library();
            }
        });
    });

    ui.add_space(14.0);

    render_runtime_install_banner(controller, language, ui);

    if let Some(error) = &controller.error {
        components::danger_alert(ui, error);
        ui.add_space(10.0);
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            components::card(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Media Source"))
                            .size(15.0)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        let text_width = (ui.available_width() - 95.0).max(100.0);
                        components::text_edit_ui(
                            ui,
                            "player_draft_source",
                            egui::TextEdit::singleline(&mut controller.draft_source)
                                .hint_text(tr(
                                    language,
                                    "Enter stream URL or choose local media file...",
                                ))
                                .desired_width(text_width),
                        );

                        if components::primary_button(ui, tr(language, "Browse...")).clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter(
                                    "All Media Files",
                                    &[
                                        "mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v",
                                        "mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma",
                                        "ape", "alac",
                                    ],
                                )
                                .add_filter(
                                    "Video Files",
                                    &["mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v"],
                                )
                                .add_filter(
                                    "Audio Files",
                                    &[
                                        "mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma",
                                        "ape", "alac",
                                    ],
                                )
                                .pick_file()
                            {
                                controller.draft_source = path.to_string_lossy().to_string();
                            }
                        }
                    });

                    // Detection indicator
                    let draft_trim = controller.draft_source.trim();
                    if !draft_trim.is_empty()
                        && !draft_trim.starts_with("http://")
                        && !draft_trim.starts_with("https://")
                    {
                        let path = Path::new(draft_trim);
                        if path.is_file() {
                            let media_type = detect_media_type(path);
                            ui.add_space(6.0);
                            match media_type {
                                MediaType::AudioOnly => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "🎵 {}",
                                            tr(language, "Audio file detected")
                                        ))
                                        .size(12.0)
                                        .color(Color32::from_rgb(126, 34, 206))
                                        .strong(),
                                    );
                                }
                                MediaType::Video => {
                                    ui.label(
                                        egui::RichText::new("🎬 Video file detected")
                                            .size(12.0)
                                            .color(Color32::from_rgb(67, 56, 202))
                                            .strong(),
                                    );
                                }
                            }
                        }
                    }

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(16.0);

                    ui.label(
                        egui::RichText::new(tr(language, "Task Title (Optional)"))
                            .size(15.0)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(8.0);
                    components::text_edit_ui(
                        ui,
                        "player_draft_title",
                        egui::TextEdit::singleline(&mut controller.draft_title)
                            .hint_text(tr(language, "Leave empty to use file name"))
                            .desired_width(ui.available_width()),
                    );

                    ui.add_space(20.0);

                    ui.horizontal(|ui| {
                        if components::primary_button(ui, tr(language, "Create & Play")).clicked() {
                            if controller.backend.is_none() && !controller.try_init_backend() {
                                controller.error = Some(
                                    tr(
                                        language,
                                        "Please download and install the player runtime first",
                                    )
                                    .into(),
                                );
                            } else {
                                match controller.start_draft_task() {
                                    Ok(_) => {
                                        controller.error = None;
                                        action = VideoPlayerAction::None;
                                    }
                                    Err(e) => {
                                        controller.error = Some(e);
                                    }
                                }
                            }
                        }

                        ui.add_space(8.0);
                        if components::animated_button(ui, tr(language, "Back to Library"))
                            .clicked()
                        {
                            controller.open_library();
                            action = VideoPlayerAction::StopTranslation;
                        }
                    });
                });
            });
            ui.add_space(16.0);
        });

    action
}
