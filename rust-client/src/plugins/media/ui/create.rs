use crate::{
    plugins::media::{MediaAction, controller::MediaController, i18n::tr, task},
    ui::components,
};
use eframe::egui;

pub(super) fn render_create(
    controller: &mut MediaController,
    snapshot: &crate::plugins::media::MediaUiSnapshot,
    ui: &mut egui::Ui,
) -> MediaAction {
    let language = snapshot.language;
    if controller.resource_browser.choosing {
        controller.resource_browser.render_picker(
            ui,
            language,
            crate::media_import::discovery::MediaKind::Recorded,
        );
        return MediaAction::None;
    }
    let mut action = MediaAction::None;
    components::page_header(ui, tr(language, "Import media"), |ui| {
        if components::back_button(ui, tr(language, "Back to Library")).clicked() {
            controller.open_library();
            action = MediaAction::StopTranslation;
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        components::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let changed = components::source_input(
                ui,
                "media_source_file",
                &mut controller.draft_source,
                tr(language, "File, link or share text"),
                tr(language, "Choose file"),
                &[("Media", task::MEDIA_EXTENSIONS), ("All files", &["*"])],
            );
            if changed {
                controller.resource_browser.invalidate();
                controller.resource_browser.url = controller.draft_source.clone();
                controller.error = None;
            }
            let online =
                crate::media_import::discovery::input_url(&controller.draft_source).is_some();
            if online {
                controller.resource_browser.url = controller.draft_source.clone();
                match controller.resource_browser.render_selection(
                    ui,
                    language,
                    &snapshot.download_proxy_url,
                ) {
                    Ok(true) => controller.error = None,
                    Err(error) => controller.error = Some(error),
                    _ => {}
                }
                if controller
                    .resource_browser
                    .selected_resource()
                    .is_some_and(|episode| {
                        episode.kind != crate::media_import::discovery::MediaKind::Recorded
                    })
                {
                    components::validation_notice(
                        ui,
                        language,
                        tr(
                            language,
                            if controller.resource_browser.selected_resource().is_some_and(
                                |episode| {
                                    episode.kind
                                        == crate::media_import::discovery::MediaKind::Unknown
                                },
                            ) {
                                "Cannot determine whether this source is recorded"
                            } else {
                                "Live streams are not supported here"
                            },
                        ),
                    );
                }
            }
            ui.add_space(12.0);
            egui::CollapsingHeader::new(tr(language, "Task Title (Optional)")).show(ui, |ui| {
                components::input_field(
                    ui,
                    &mut controller.draft_title,
                    tr(language, "Task Title (Optional)"),
                );
            });
            ui.add_space(12.0);
            if components::primary_button_enabled(
                ui,
                tr(language, "Open media"),
                !controller.draft_source.trim().is_empty()
                    && (!online
                        || controller.resource_browser.selected_resource().is_some_and(
                            |episode| {
                                episode.kind == crate::media_import::discovery::MediaKind::Recorded
                            },
                        )),
            )
            .clicked()
            {
                if let Err(error) = controller.start_draft_task() {
                    controller.error = Some(error);
                }
            }
        });
    });
    action
}
