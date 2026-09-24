mod create;
mod library;
mod player;
mod subtitles;

use super::{
    VideoPlayerAction, VideoPlayerPlugin, VideoPlayerUiSnapshot,
    controller::{VideoPlayerController, VideoPlayerRoute},
    i18n::tr,
};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius};

pub(super) fn render(
    plugin: &mut VideoPlayerPlugin,
    snapshot: &VideoPlayerUiSnapshot,
    ui: &mut egui::Ui,
) -> VideoPlayerAction {
    plugin.controller.tick();
    let language = snapshot.language;

    match plugin.controller.route {
        VideoPlayerRoute::Library => library::render_library(&mut plugin.controller, language, ui),
        VideoPlayerRoute::Create => create::render_create(&mut plugin.controller, language, ui),
        VideoPlayerRoute::Player => player::render_player(&mut plugin.controller, language, ui),
    }
}

fn render_runtime_install_banner(
    controller: &mut VideoPlayerController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    if controller.backend.is_some() {
        return;
    }

    components::card(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🎬").size(22.0));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Video Player Runtime Missing"))
                            .size(16.0)
                            .strong()
                            .color(crate::ui::theme::text_strong()),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(tr(
                            language,
                            "Video playback and multi-track audio extraction require the mpv runtime library (mpv-2.dll). You can click the button below to download and configure it directly from the GitHub repository.",
                        ))
                        .size(12.5)
                        .color(crate::ui::theme::text_weak()),
                    );
                });
            });

            ui.add_space(10.0);

            let state = controller.mpv_installer.state().clone();
            match state {
                super::installer::MpvInstallState::Idle => {
                    if components::primary_button(
                        ui,
                        tr(language, "Download Player Runtime (46.8 MB)"),
                    )
                    .clicked()
                    {
                        let _ = controller.mpv_installer.start_download();
                    }
                }
                super::installer::MpvInstallState::Downloading { downloaded, total } => {
                    ui.horizontal(|ui| {
                        let ratio = if total > 0 {
                            (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let progress_text = format!(
                            "{} / {} ({:.1}%)",
                            components::format_file_size(downloaded),
                            components::format_file_size(total),
                            ratio * 100.0
                        );
                        ui.add(
                            egui::ProgressBar::new(ratio)
                                .text(progress_text)
                                .animate(true),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(tr(language, "Downloading runtime..."))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                    );
                }
                super::installer::MpvInstallState::Extracting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(tr(
                                language,
                                "Extracting and installing player runtime...",
                            ))
                            .size(13.0)
                            .color(crate::ui::theme::primary()),
                        );
                    });
                }
                super::installer::MpvInstallState::Failed(ref err) => {
                    components::error_notice(ui, language, err);
                    ui.add_space(6.0);
                    if components::primary_button(ui, tr(language, "Retry Download")).clicked() {
                        let _ = controller.mpv_installer.start_download();
                    }
                }
                super::installer::MpvInstallState::Ready => {
                    ui.label(
                        egui::RichText::new(tr(language, "Player runtime installed successfully!"))
                            .size(13.0)
                            .color(crate::ui::theme::success()),
                    );
                }
            }
        });
    });
    ui.add_space(14.0);
}

fn format_time_ms(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, s)
    } else {
        format!("{:02}:{:02}", mins, s)
    }
}

fn format_timestamp_date(timestamp_sec: u64) -> String {
    let dt = std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp_sec);
    if let Ok(duration) = std::time::SystemTime::now().duration_since(dt) {
        let mins = duration.as_secs() / 60;
        if mins < 1 {
            return "Just now".into();
        } else if mins < 60 {
            return format!("{}m ago", mins);
        }
        let hours = mins / 60;
        if hours < 24 {
            return format!("{}h ago", hours);
        }
        let days = hours / 24;
        return format!("{}d ago", days);
    }
    "Recently".into()
}

/// Compact dark-themed button for the floating video control bar.
/// Returns `true` if clicked.
fn dark_pill_button(ui: &mut egui::Ui, icon: &str, enabled: bool, accent: bool) -> bool {
    let desired = egui::vec2(32.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let pressed = response.is_pointer_button_down_on() && enabled;

        let bg = if pressed {
            Color32::from_rgba_unmultiplied(80, 100, 140, 180)
        } else if hovered {
            Color32::from_rgba_unmultiplied(55, 70, 100, 160)
        } else if accent {
            Color32::from_rgba_unmultiplied(37, 99, 235, 200)
        } else {
            Color32::from_rgba_unmultiplied(35, 45, 65, 140)
        };

        let text_color = if !enabled {
            Color32::from_rgb(90, 100, 115)
        } else if pressed {
            Color32::WHITE
        } else if hovered {
            Color32::from_rgb(220, 230, 245)
        } else {
            Color32::from_rgb(190, 200, 215)
        };

        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(14.0),
            text_color,
        );
    }

    response.clicked() && enabled
}
