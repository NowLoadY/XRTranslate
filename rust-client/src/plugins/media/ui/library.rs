use crate::{
    plugins::media::{MediaAction, controller::MediaController, i18n::tr},
    ui::components,
};
use eframe::egui;

pub(super) fn render_library(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> MediaAction {
    let mut action = MediaAction::None;
    components::page_header(ui, tr(language, "Media"), |_| {});
    components::library_start(
        ui,
        &[
            components::LibraryHint {
                icon: egui::include_image!("../../../../resources/icons/file-audio.svg"),
                caption: "MP3 · WAV · FLAC",
                tooltip: &format!(
                    "{} · {}",
                    tr(language, "Audio"),
                    super::super::task::AUDIO_EXTENSIONS.join(" · ")
                ),
            },
            components::LibraryHint {
                icon: egui::include_image!("../../../../resources/icons/file-video.svg"),
                caption: "MP4 · MKV · WEBM",
                tooltip: &format!(
                    "{} · MP4 · MKV · WEBM · AVI · MOV · FLV · TS · M4V · MPEG · MPG · WMV",
                    tr(language, "Video")
                ),
            },
            components::LibraryHint {
                icon: egui::include_image!("../../../../resources/icons/file-subtitles.svg"),
                caption: "SRT · VTT",
                tooltip: tr(language, "Subtitles"),
            },
            components::LibraryHint {
                icon: egui::include_image!("../../../../resources/icons/stream.svg"),
                caption: "URL",
                tooltip: tr(language, "File path or stream URL"),
            },
        ],
        |ui| {
            #[cfg(target_os = "android")]
            let import = components::primary_button(ui, tr(language, "Import media"));
            #[cfg(not(target_os = "android"))]
            let import = components::round_icon_button(
                ui,
                tr(language, "Import media"),
                egui::include_image!("../../../../resources/icons/import.svg"),
                40.0,
            );
            if import.clicked() {
                controller.open_create();
                action = MediaAction::StopTranslation;
            }
        },
    );
    if let Some(path) = ui.input(|input| {
        input
            .raw
            .dropped_files
            .first()
            .map(|file| file.path().to_path_buf())
    }) {
        controller.open_create();
        controller.draft_source = path.to_string_lossy().into_owned();
        return MediaAction::StopTranslation;
    }
    super::render_runtime_install_banner(controller, language, ui);
    if controller.store.tasks.is_empty() {
        return action;
    }
    let limit = components::library_navigation(
        ui,
        language,
        &mut controller.browse_all,
        &mut controller.search_query,
        controller.store.tasks.len(),
        tr(language, "Search media tasks"),
    );
    let query = controller.search_query.trim().to_lowercase();
    let mut open = None;
    let mut delete = None;
    let mut export_video = None;
    let mut visible = 0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for task in controller
                .store
                .tasks
                .iter()
                .filter(|task| query.is_empty() || task.title.to_lowercase().contains(&query))
                .take(limit)
            {
                visible += 1;
                ui.push_id(&task.id, |ui| {
                    components::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    components::icon_menu(
                                        ui,
                                        tr(language, "Task actions"),
                                        egui::include_image!(
                                            "../../../../resources/icons/more.svg"
                                        ),
                                        |ui| {
                                            if super::export_controls(
                                                ui,
                                                language,
                                                &task.title,
                                                &task.subtitles,
                                                controller.video_export.is_none() && super::super::video_export::VideoExport::supported(task),
                                            ) { export_video = Some(task.id.clone()); }
                                            if ui.button(tr(language, "Delete")).clicked() {
                                                delete = Some(task.id.clone());
                                                ui.close();
                                            }
                                        },
                                    );
                                    if components::open_button(ui, tr(language, "Open task"))
                                        .clicked()
                                    {
                                        open = Some(task.id.clone());
                                    }
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&task.title).strong(),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&task.title);
                                        },
                                    );
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} · {}: {}",
                                components::format_timestamp(
                                    task.created_at_sec
                                        .saturating_mul(1000)
                                        .min(i64::MAX as u64)
                                        as i64
                                ),
                                super::format_time_ms(task.duration_ms),
                                tr(language, "Subtitles Count"),
                                task.subtitles.count()
                            ))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                        );
                    })
                });
                ui.add_space(8.0);
            }
            if visible == 0 {
                components::card(ui, |ui| {
                    ui.label(tr(language, "No matching media tasks"));
                });
            }
        });
    if let Some(id) = open
        && let Err(error) = controller.open_task(&id)
    {
        controller.error = Some(error);
    }
    if let Some(id) = delete {
        controller.delete_task(&id);
    }
    if let Some(id) = export_video {
        controller.start_video_export(&id, ui.ctx());
    }
    action
}
