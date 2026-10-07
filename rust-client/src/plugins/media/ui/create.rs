use crate::{
    plugins::media::{MediaAction, controller::MediaController, i18n::tr},
    ui::components,
};
use eframe::egui;

pub(super) fn render_create(
    controller: &mut MediaController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> MediaAction {
    let mut action = MediaAction::None;
    crate::ui::layout::flow_row(ui, |ui| {
        ui.heading(tr(language, "New media task"));
        if components::secondary_button(ui, tr(language, "Back to Library")).clicked() {
            controller.open_library();
            action = MediaAction::StopTranslation;
        }
    });
    ui.add_space(14.0);
    components::card(ui, |ui| {
        if components::segmented_switch(
            ui,
            &mut controller.draft_subtitles,
            [tr(language, "Audio / video"), tr(language, "Subtitle file")],
        )
        .changed()
        {
            controller.draft_source.clear();
        }
        ui.add_space(12.0);
        ui.label(tr(
            language,
            if controller.draft_subtitles {
                "Translate subtitles while keeping their timestamps."
            } else {
                "Create a task from an audio file, video file or stream."
            },
        ));
        ui.add_space(8.0);
        components::input_field(
            ui,
            &mut controller.draft_source,
            tr(
                language,
                if controller.draft_subtitles {
                    "Choose an SRT or VTT file"
                } else {
                    "File path or stream URL"
                },
            ),
        );
        let browse = components::secondary_button(ui, tr(language, "Browse...")).clicked();
        let extensions: &[&str] = if controller.draft_subtitles {
            &["srt", "vtt"]
        } else {
            &[
                "mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v", "mp3", "wav", "flac",
                "aac", "ogg", "m4a", "opus", "wma", "ape", "alac",
            ]
        };
        if let Some(path) = crate::file_dialog::FileDialog::new()
            .add_filter("Media", extensions)
            .pick_file(ui.ctx(), "media_source_file", browse)
        {
            controller.draft_source = path.to_string_lossy().into_owned();
        }
        if let Some(path) = ui.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        }) {
            controller.draft_subtitles = super::super::task::detect_media_type(&path)
                == super::super::task::MediaType::Subtitles;
            controller.draft_source = path.to_string_lossy().into_owned();
        }
        ui.add_space(12.0);
        ui.label(tr(language, "Task Title (Optional)"));
        components::input_field(
            ui,
            &mut controller.draft_title,
            tr(language, "Task Title (Optional)"),
        );
        ui.add_space(16.0);
        if components::primary_button_enabled(
            ui,
            tr(language, "Create task"),
            !controller.draft_source.trim().is_empty(),
        )
        .clicked()
        {
            match controller.start_draft_task() {
                Ok(_) => {}
                Err(error) => controller.error = Some(error),
            }
        }
    });
    action
}
