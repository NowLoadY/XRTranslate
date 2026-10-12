use super::super::{actions::UiAction, presentation::format_duration};
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
            if !bundle.markers.is_empty() {
                ui.add_space(8.0);
                components::section_heading(ui, tr(language, "Highlights"));
            }
            for marker in &bundle.markers {
                ui.push_id(&marker.id, |ui| {
                    components::card(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(tr(language, "Highlight")).size(12.0));
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
                        if let Some(segment) = bundle
                            .segments
                            .iter()
                            .find(|segment| segment.id == marker.segment_id)
                        {
                            ui.add(
                                egui::Label::new(
                                    segment
                                        .translated_text
                                        .as_deref()
                                        .unwrap_or(&segment.original_text),
                                )
                                .wrap(),
                            );
                        }
                        super::highlights::note_editor(
                            marker,
                            &mut controller.marker_drafts,
                            language,
                            action,
                            ui,
                        );
                    });
                });
                ui.add_space(6.0);
            }
        });
}
