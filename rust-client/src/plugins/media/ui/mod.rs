mod create;
mod detail;
mod library;
mod subtitles;

use super::{
    MediaAction, MediaPlugin, MediaUiSnapshot,
    controller::{MediaController, MediaRoute},
    i18n::tr,
};
use crate::ui::components;
use eframe::egui;

pub(super) fn render(
    plugin: &mut MediaPlugin,
    snapshot: &MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    let language = snapshot.language;

    let action = match plugin.controller.route {
        MediaRoute::Library => library::render_library(&mut plugin.controller, language, ui),
        MediaRoute::Create => create::render_create(&mut plugin.controller, language, ui),
        MediaRoute::Detail => detail::render_detail(&mut plugin.controller, snapshot, ui),
    };
    crate::ui::notifications::observe_error(
        ui.ctx(),
        egui::Id::new("media_task_error"),
        tr(language, "Could not complete the media task."),
        plugin.controller.error.take().as_deref(),
    );
    action
}

fn render_runtime_install_banner(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) {
    let install_error = match controller.mpv_installer.state() {
        super::installer::MpvInstallState::Failed(error) => Some(error.as_str()),
        _ => None,
    };
    crate::ui::notifications::observe_error(
        ui.ctx(),
        egui::Id::new("player_runtime_download"),
        crate::i18n::tr(
            language,
            "Could not download the player runtime. Please retry.",
        ),
        install_error,
    );
    if controller.backend.is_some() {
        return;
    }

    if !cfg!(all(windows, feature = "mpv")) {
        return;
    }
    egui::CollapsingHeader::new(tr(language, "Optional playback support")).show(ui, |ui| {
        match controller.mpv_installer.state() {
            super::installer::MpvInstallState::Downloading { downloaded, total } => {
                let fraction = if *total == 0 {
                    0.0
                } else {
                    *downloaded as f32 / *total as f32
                };
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
            }
            super::installer::MpvInstallState::Extracting => {
                ui.spinner();
            }
            _ => {
                if components::secondary_button(ui, tr(language, "Install playback support"))
                    .clicked()
                    && let Err(error) = controller.mpv_installer.start_download()
                {
                    controller.error = Some(error);
                }
            }
        }
    });
}

fn export_controls(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    title: &str,
    subtitles: &super::subtitles::SubtitleTimeline,
) {
    if subtitles.count() == 0 {
        return;
    }
    let stem = std::path::Path::new(title)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("subtitles");
    for (label, extension) in [
        ("Export SRT", "srt"),
        ("Export VTT", "vtt"),
        ("Export LRC", "lrc"),
    ] {
        if ui.button(tr(language, label)).clicked() {
            crate::ui::notifications::observe_error(ui.ctx(), ui.id().with("export"), "", None);
            let content = match extension {
                "vtt" => subtitles.export_vtt(),
                "lrc" => subtitles.export_lrc(Some(stem)),
                _ => subtitles.export_srt(),
            };
            if let Err(error) = crate::file_dialog::FileDialog::new()
                .set_file_name(format!("{stem}.{extension}"))
                .add_filter("Subtitles", &[extension])
                .save(content)
            {
                crate::ui::notifications::observe_error(
                    ui.ctx(),
                    ui.id().with("export"),
                    crate::i18n::tr(language, "Could not complete the file operation."),
                    Some(&error),
                );
            }
            ui.close();
        }
    }
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
