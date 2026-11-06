use crate::{
    file_dialog::FileDialog,
    i18n::UiLanguage,
    media::{
        MediaAction,
        controller::MediaController,
        files::SubtitleChange,
        i18n::tr,
        task::{MEDIA_EXTENSIONS, MediaType, SUBTITLE_EXTENSIONS, detect_media_type},
    },
    ui::components,
};
use eframe::egui;

pub(in crate::media::ui) fn subtitle_header(
    controller: &mut MediaController,
    language: UiLanguage,
    ui: &mut egui::Ui,
) {
    let mut import = false;
    let mut attach = false;
    let available = controller.can_change_files();
    let standalone = controller.is_subtitle_task();
    ui.horizontal(|ui| {
        ui.strong(tr(language, "Subtitles"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(available, |ui| {
                import = components::round_icon_button(
                    ui,
                    tr(language, "Import subtitles"),
                    xrtranslate_client_core::ui::icon!("import.svg"),
                    28.0,
                )
                .clicked();
                if standalone {
                    attach = components::round_icon_button(
                        ui,
                        tr(language, "Attach media"),
                        xrtranslate_client_core::ui::icon!("file-video.svg"),
                        28.0,
                    )
                    .clicked();
                }
            });
        });
    });
    let key = controller.active_task_id.clone().unwrap_or_default();
    let subtitle = FileDialog::new()
        .add_filter("Subtitles", SUBTITLE_EXTENSIONS)
        .pick_file(ui.ctx(), &format!("media_subtitles_{key}"), import);
    let extensions = MEDIA_EXTENSIONS
        .iter()
        .copied()
        .filter(|ext| !SUBTITLE_EXTENSIONS.contains(ext))
        .collect::<Vec<_>>();
    let media = FileDialog::new()
        .add_filter("Media", &extensions)
        .pick_file(ui.ctx(), &format!("media_attachment_{key}"), attach);
    let dropped = available
        .then(|| {
            ui.input(|input| {
                input
                    .raw
                    .dropped_files
                    .first()
                    .map(|file| file.path().to_path_buf())
            })
        })
        .flatten();
    let result = if let Some(path) = subtitle {
        controller.import_subtitles(path)
    } else if let Some(path) = media {
        controller.attach_media(path)
    } else if let Some(path) = dropped {
        if detect_media_type(&path) == MediaType::Subtitles {
            controller.import_subtitles(path)
        } else if standalone {
            controller.attach_media(path)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    if let Err(error) = result {
        controller.error = Some(error);
    }
}

pub(super) fn confirm_change(
    controller: &mut MediaController,
    language: UiLanguage,
    ctx: &egui::Context,
) -> MediaAction {
    let Some(change) = &controller.pending_subtitle_change else {
        return MediaAction::None;
    };
    let translations = matches!(
        change,
        SubtitleChange::Action(MediaAction::TranslateSubtitles { .. })
    );
    let mut accept = false;
    let mut cancel = false;
    let response = egui::Modal::new(egui::Id::unique("media_replace_subtitles")).show(ctx, |ui| {
        ui.set_max_width(360.0);
        ui.heading(tr(
            language,
            if translations {
                "Replace translations?"
            } else {
                "Replace existing subtitles?"
            },
        ));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            cancel = components::secondary_button(ui, tr(language, "Cancel")).clicked();
            accept = components::primary_button(
                ui,
                tr(
                    language,
                    if translations {
                        "Retranslate all"
                    } else {
                        "Replace"
                    },
                ),
            )
            .clicked();
        });
    });
    if accept {
        if let Some(change) = controller.pending_subtitle_change.take() {
            return controller.apply_subtitle_change(change);
        }
    } else if cancel || response.should_close() {
        controller.pending_subtitle_change = None;
    }
    MediaAction::None
}
