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
        let controls_height = controls_height(ui, language);
        let draft = drafts
            .entry(marker.id.clone())
            .or_insert_with(|| marker.text.clone());
        if components::text_edit_ui(
            ui,
            "highlight_note",
            egui::TextEdit::multiline(draft)
                .id(egui::Id::unique(("meeting_note", &marker.id)))
                .hint_text(tr(language, "Add a note (optional)"))
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        )
        .changed()
        {
            ui.ctx()
                .request_discard("Remeasure the edited meeting note");
        }
        ui.horizontal(|ui| {
            ui.set_min_height(controls_height);
            if components::secondary_button_enabled(ui, tr(language, "Save"), *draft != marker.text)
                .clicked()
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

pub(super) fn editor_height(
    ui: &egui::Ui,
    width: f32,
    draft: &str,
    language: crate::i18n::UiLanguage,
) -> f32 {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let line_height =
        ui.fonts_mut(|fonts| fonts.row_height(&font)) + ui.spacing().extra_text_line_spacing;
    // Match TextEdit's default multiline layout and its (4, 2) inner margin.
    let mut job = egui::text::LayoutJob::simple(
        draft.to_owned(),
        font,
        crate::ui::theme::text_strong(),
        (width.max(24.0) - 8.0).max(1.0),
    );
    job.keep_trailing_whitespace = true;
    for section in &mut job.sections {
        section.format.line_height = Some(line_height);
    }
    let mut text_height = ui.painter().layout_job(job).size().y.max(2.0 * line_height);
    if draft.is_empty() {
        text_height = text_height.max(
            super::rows::text(
                ui,
                egui::RichText::new(tr(language, "Add a note (optional)")),
                (width.max(24.0) - 8.0).max(1.0),
            )
            .size()
            .y,
        );
    }
    text_height + 4.0 + ui.spacing().item_spacing.y + controls_height(ui, language)
}

fn controls_height(ui: &egui::Ui, language: crate::i18n::UiLanguage) -> f32 {
    let save = super::rows::text(
        ui,
        egui::RichText::new(tr(language, "Save"))
            .size(13.0)
            .strong(),
        f32::INFINITY,
    )
    .size()
    .y + 12.0
        + if crate::ui::theme::is_hand_drawn(ui.ctx()) {
            0.0
        } else {
            2.0
        };
    save.max(ui.spacing().interact_size.y)
        .max(ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().button_padding.y)
}
