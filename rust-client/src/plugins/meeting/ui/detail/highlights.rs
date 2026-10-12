use std::collections::HashMap;

use super::super::actions::UiAction;
use crate::{
    plugins::meeting::{i18n::tr, store::SegmentMarker},
    ui::components,
};
use eframe::egui;

/// Both the transcript and meeting summary edit the same draft by marker ID.
pub(super) fn note_editor(
    marker: &SegmentMarker,
    drafts: &mut HashMap<String, String>,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    ui.push_id(&marker.id, |ui| {
        let draft = drafts
            .entry(marker.id.clone())
            .or_insert_with(|| marker.text.clone());
        components::text_edit_ui(
            ui,
            "highlight_note",
            egui::TextEdit::multiline(draft)
                .hint_text(tr(language, "Add a note (optional)"))
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if *draft != marker.text
                && components::secondary_button(ui, tr(language, "Save")).clicked()
            {
                let mut edited = marker.clone();
                edited.text = draft.clone();
                *action = UiAction::SaveMarker(edited);
            }
            ui.menu_button("⋯", |ui| {
                let label = if draft.trim().is_empty() && marker.text.trim().is_empty() {
                    "Remove highlight"
                } else {
                    "Remove highlight and note"
                };
                if ui.button(tr(language, label)).clicked() {
                    *action = UiAction::RemoveMarker(marker.id.clone());
                    ui.close();
                }
            });
        });
    });
}
