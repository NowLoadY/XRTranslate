use super::super::{
    actions::UiAction,
    presentation::{format_duration, marker_label},
};
use crate::plugins::meeting::{controller::MeetingController, i18n::tr};
use crate::ui::components;
use eframe::egui;

pub(super) fn render_minutes(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    egui::ScrollArea::vertical()
        .id_salt("meeting_minutes_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let response = components::text_edit_ui(
                ui,
                "meeting_minutes_draft",
                egui::TextEdit::multiline(&mut controller.minutes_draft)
                    .hint_text(tr(
                        language,
                        "Write a summary and the points you want to keep.",
                    ))
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            controller.minutes_dirty |= response.changed();
            if components::primary_button_enabled(
                ui,
                tr(language, "Save minutes"),
                controller.minutes_dirty,
            )
            .clicked()
            {
                *action = UiAction::SaveMinutes;
            }
            ui.add_space(10.0);
            super::organization::render(controller, language, action, ui);

            let Some(bundle) = controller.bundle.as_ref() else {
                return;
            };
            if !bundle.segments.is_empty() {
                egui::CollapsingHeader::new(tr(language, "Add note")).show(ui, |ui| {
                    components::text_edit_ui(
                        ui,
                        "meeting_quick_note",
                        egui::TextEdit::multiline(&mut controller.quick_note)
                            .hint_text(tr(language, "Quick note linked to the latest message"))
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    if components::animated_button_enabled(
                        ui,
                        tr(language, "Add note"),
                        !controller.quick_note.trim().is_empty(),
                    )
                    .clicked()
                    {
                        *action = UiAction::QuickNote;
                    }
                });
            }
            if !bundle.markers.is_empty() {
                ui.add_space(8.0);
                components::section_heading(ui, tr(language, "User markers"));
            }
            for marker in &bundle.markers {
                ui.push_id(&marker.id, |ui| {
                    components::card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(marker_label(marker.kind, language)).size(12.0),
                            );
                            let timestamp = bundle
                                .segments
                                .iter()
                                .find(|segment| segment.id == marker.segment_id)
                                .map(|segment| format_duration(segment.start_ms))
                                .unwrap_or_else(|| tr(language, "View original").to_owned());
                            if ui.link(timestamp).clicked() {
                                *action = UiAction::JumpToEvidence(marker.segment_id.clone());
                            }
                        });
                        // Keep an edit separate from the live database refresh until saved.
                        let draft_id = ui.id().with("annotation_draft");
                        let mut draft = ui
                            .data(|data| data.get_temp::<String>(draft_id))
                            .unwrap_or_else(|| marker.text.clone());
                        let response = components::text_edit_ui(
                            ui,
                            "annotation_text",
                            egui::TextEdit::multiline(&mut draft)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        if response.changed() {
                            ui.data_mut(|data| data.insert_temp(draft_id, draft.clone()));
                        }
                        if draft == marker.text {
                            ui.data_mut(|data| data.remove::<String>(draft_id));
                        } else if components::animated_button(ui, tr(language, "Save")).clicked() {
                            let mut edited = marker.clone();
                            edited.text = draft;
                            *action = UiAction::SaveMarker(edited);
                        }
                    });
                });
                ui.add_space(6.0);
            }
        });
}
