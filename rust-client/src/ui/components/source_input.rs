use super::{input_field, open_button};
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
        let mut changed = input_field(ui, value, hint).changed();
        let browse = open_button(ui, browse_label).clicked();
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
