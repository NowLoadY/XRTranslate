use crate::{
    plugins::media::{MediaAction, controller::MediaController, i18n::tr, task::MediaType},
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
        egui::include_image!("../../../../resources/plugins/media/icon.svg"),
        tr(language, "Translate your media"),
        tr(
            language,
            "Choose or drop audio, video or subtitles, or paste a stream URL.",
        ),
        |ui| {
            if components::primary_button(ui, tr(language, "Import media")).clicked() {
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
                        crate::ui::layout::flow_row(ui, |ui| {
                            ui.label(egui::RichText::new(&task.title).strong());
                            ui.weak(tr(
                                language,
                                match task.media_type {
                                    MediaType::Video => "Video",
                                    MediaType::AudioOnly => "Audio",
                                    MediaType::Subtitles => "Subtitles",
                                },
                            ));
                            if components::secondary_button(ui, tr(language, "Open task")).clicked()
                            {
                                open = Some(task.id.clone());
                            }
                            ui.menu_button("…", |ui| {
                                super::export_controls(ui, language, &task.title, &task.subtitles);
                                if ui.button(tr(language, "Delete")).clicked() {
                                    delete = Some(task.id.clone());
                                    ui.close();
                                }
                            });
                        });
                        ui.weak(format!(
                            "{} · {}",
                            components::format_timestamp(
                                task.created_at_sec
                                    .saturating_mul(1000)
                                    .min(i64::MAX as u64) as i64
                            ),
                            super::format_time_ms(task.duration_ms)
                        ));
                        ui.label(format!(
                            "{} · {}",
                            task.subtitles.count(),
                            tr(language, "Subtitles Count")
                        ));
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
    action
}
