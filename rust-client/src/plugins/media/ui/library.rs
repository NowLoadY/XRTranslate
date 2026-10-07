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
    crate::ui::layout::flow_row(ui, |ui| {
        ui.heading(tr(language, "Media"));
        if components::primary_button(ui, tr(language, "New media task")).clicked() {
            controller.open_create();
            action = MediaAction::StopTranslation;
        }
    });
    ui.add_space(12.0);
    super::render_runtime_install_banner(controller, language, ui);
    components::search_bar(
        ui,
        &mut controller.search_query,
        tr(language, "Search media tasks"),
    );
    ui.add_space(10.0);
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
