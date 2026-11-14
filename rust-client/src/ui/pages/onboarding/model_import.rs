use super::{components, egui, i18n, theme};
use crate::model_install::NativeModelTaskState;

#[derive(Clone, Copy)]
enum Choice {
    #[cfg(any(windows, target_os = "linux"))]
    Folder,
    File,
    Cancel,
}

pub(super) fn controls(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    let tasks = &app.model_task_manager;
    let importing = tasks.is_importing();
    let choice = ui
        .horizontal_wrapped(|ui| {
            let choice = button(ui, language, !tasks.is_busy(), importing);
            if importing {
                match tasks.state() {
                    NativeModelTaskState::Installing {
                        downloaded_bytes,
                        total_bytes,
                        ..
                    } => {
                        let fraction = *downloaded_bytes as f32 / (*total_bytes).max(1) as f32;
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(160.0)
                                .desired_height(3.0),
                        );
                        ui.weak(format!(
                            "{} · {:.0}%",
                            i18n::tr(language, "Importing"),
                            fraction * 100.0
                        ));
                    }
                    _ => {
                        ui.weak(i18n::tr(language, "Identifying model…"));
                    }
                }
            }
            choice
        })
        .inner;
    if let Some(choice) = choice {
        choose(app, choice);
    }
}

fn button(
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
                egui::Button::image_and_text(
                    egui::Image::new(icon)
                        .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        .tint(theme::text_weak()),
                    label,
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
        if components::button(ui, i18n::tr(language, "Model file or ZIP…")).clicked() {
            choice = Some(Choice::File);
            ui.close();
        }
    });
    choice
}

fn choose(app: &mut crate::XRTranslateApp, choice: Choice) {
    let request = match choice {
        Choice::Cancel => {
            app.model_task_manager.cancel_import();
            return;
        }
        #[cfg(any(windows, target_os = "linux"))]
        Choice::Folder => crate::file_dialog::FileDialog::new().start_pick_folder(),
        Choice::File => crate::file_dialog::FileDialog::new()
            .add_filter("Model", &["gguf", "onnx", "bin", "zip"])
            .start_pick(),
    };
    app.pending_file_dialogs.push((
        request,
        Box::new(move |app, path| {
            if let Err(error) = app.model_task_manager.import(app.project_root(), path) {
                app.notify_error("Could not import model.", error);
            }
        }),
    ));
}
