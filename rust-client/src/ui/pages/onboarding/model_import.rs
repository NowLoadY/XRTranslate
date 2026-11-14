use super::{components, egui, i18n, theme};
use crate::model_install::{NativeModelTaskManager, ResourceId};

#[derive(Clone, Copy)]
pub(super) enum Choice {
    #[cfg(any(windows, target_os = "linux"))]
    Folder,
    Zip,
    Cancel,
}

pub(super) fn button(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    enabled: bool,
    importing: bool,
) -> Option<Choice> {
    let label = i18n::tr(
        language,
        if importing {
            "Cancel import"
        } else {
            "Import model"
        },
    );
    let icon = if importing {
        egui::include_image!("../../../../../crates/xrtranslate-client-core/resources/icons/x.svg")
    } else {
        egui::include_image!(
            "../../../../../crates/xrtranslate-client-core/resources/icons/import.svg"
        )
    };
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(5.0, 4.0);
            let response = ui.add_enabled(
                enabled || importing,
                egui::Button::image(
                    egui::Image::new(icon)
                        .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        .tint(theme::text_weak()),
                )
                .min_size(egui::vec2(28.0, 28.0))
                .corner_radius(9),
            );
            ui_director::egui::button_response(
                ui,
                response.id,
                label,
                enabled || importing,
                response,
            )
        })
        .inner
        .on_hover_text(label);
    if importing {
        return response.clicked().then_some(Choice::Cancel);
    }
    let mut choice = None;
    egui::Popup::menu(&response).width(160.0).show(|ui| {
        #[cfg(any(windows, target_os = "linux"))]
        if components::button(ui, i18n::tr(language, "Model folder…")).clicked() {
            choice = Some(Choice::Folder);
            ui.close();
        }
        if components::button(ui, i18n::tr(language, "ZIP archive…")).clicked() {
            choice = Some(Choice::Zip);
            ui.close();
        }
    });
    choice
}

pub(super) fn progress(
    ui: &mut egui::Ui,
    language: i18n::UiLanguage,
    tasks: &NativeModelTaskManager,
    id: ResourceId,
) {
    if !tasks.is_importing(&id) {
        return;
    }
    let fraction = tasks
        .model_download_progress(id)
        .filter(|(_, total)| *total > 0)
        .map_or(0.0, |(bytes, total)| bytes as f32 / total as f32);
    ui.add_space(4.0);
    ui.add(
        egui::ProgressBar::new(fraction)
            .desired_width(ui.available_width())
            .desired_height(3.0),
    );
    ui.label(
        egui::RichText::new(format!(
            "{} · {:.0}%",
            i18n::tr(language, "Importing"),
            fraction * 100.0
        ))
        .size(11.0)
        .color(theme::text_weak()),
    );
}

pub(super) fn choose(app: &mut crate::XRTranslateApp, id: ResourceId, choice: Choice) {
    let request = match choice {
        Choice::Cancel => {
            app.model_task_manager.cancel_import();
            return;
        }
        #[cfg(any(windows, target_os = "linux"))]
        Choice::Folder => crate::file_dialog::FileDialog::new().start_pick_folder(),
        Choice::Zip => crate::file_dialog::FileDialog::new()
            .add_filter("ZIP", &["zip"])
            .start_pick(),
    };
    app.pending_file_dialogs.push((
        request,
        Box::new(move |app, path| {
            if let Err(error) = app.model_task_manager.import(app.project_root(), id, path) {
                app.notify_error("Could not import model.", error);
            }
        }),
    ));
}
