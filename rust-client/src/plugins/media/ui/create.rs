use crate::{
    plugins::media::{MediaAction, controller::MediaController, i18n::tr, task},
    ui::components,
};
use eframe::egui;

pub(super) fn render_create(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> MediaAction {
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
            ui.label(tr(
                language,
                "Choose or drop audio, video or subtitles, or paste a stream URL.",
            ));
            ui.add_space(12.0);
            components::source_input(
                ui,
                "media_source_file",
                &mut controller.draft_source,
                tr(language, "File path or stream URL"),
                tr(language, "Choose file"),
                &[("Media", task::MEDIA_EXTENSIONS), ("All files", &["*"])],
            );
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
                !controller.draft_source.trim().is_empty(),
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
