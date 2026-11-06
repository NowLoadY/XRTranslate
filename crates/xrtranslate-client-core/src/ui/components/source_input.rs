use super::input_field;
#[cfg(not(target_os = "android"))]
use super::open_button;
use eframe::egui;

/// One source field for typed paths, file selection and drag-and-drop.
pub fn source_input(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    hint: &str,
    browse_label: &str,
    filters: &[(&str, &[&str])],
) -> bool {
    ui.push_id(id, |ui| {
        #[cfg(not(target_os = "android"))]
        let (mut changed, browse) = (
            input_field(ui, value, hint).changed(),
            open_button(ui, browse_label).clicked(),
        );
        #[cfg(target_os = "android")]
        let (mut changed, browse) = {
            let browse = super::secondary_button(ui, browse_label).clicked();
            let changed = if std::path::Path::new(value.as_str()).is_file() {
                let name = std::path::Path::new(value.as_str())
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                ui.add(egui::Label::new(name).wrap());
                false
            } else {
                input_field(ui, value, hint).changed()
            };
            (changed, browse)
        };
        let mut dialog = crate::file_dialog::FileDialog::new();
        for (label, extensions) in filters {
            dialog = dialog.add_filter(label, extensions);
        }
        let selected = dialog.pick_file(ui.ctx(), id, browse);
        let dropped = ui.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        });
        if let Some(path) = selected.or(dropped) {
            *value = path.to_string_lossy().into_owned();
            changed = true;
        }
        changed
    })
    .inner
}
