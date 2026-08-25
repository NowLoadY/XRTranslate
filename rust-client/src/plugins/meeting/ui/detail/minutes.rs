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
    ui.label(
        egui::RichText::new(tr(
            language,
            "Editable Markdown minutes. Nothing is generated automatically.",
        ))
        .color(crate::ui::theme::text_weak())
        .size(12.0),
    );
    ui.add_space(6.0);

    let minutes_response = crate::ui::components::text_edit_ui(
        ui,
        "meeting_minutes_draft",
        egui::TextEdit::multiline(&mut controller.minutes_draft)
            .desired_rows(18)
            .desired_width(f32::INFINITY),
    );
    if minutes_response.changed() {
        controller.minutes_dirty = true;
    }
    ui.add_space(8.0);

    if components::primary_button(ui, tr(language, "Save minutes")).clicked() {
        *action = UiAction::SaveMinutes;
    }
    ui.add_space(14.0);

    if let Some(bundle) = controller.bundle.as_mut() {
        components::section(ui, tr(language, "User markers"), |ui| {
            for marker in &mut bundle.markers {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(marker_label(marker.kind, language))
                            .strong()
                            .size(12.0),
                    );
                    let response = crate::ui::components::text_edit_ui(
                        ui,
                        ("marker_text", &marker.id),
                        egui::TextEdit::singleline(&mut marker.text).desired_width(420.0),
                    );
                    if response.lost_focus() && response.changed() {
                        *action = UiAction::SaveMarker(marker.clone());
                    }
                    let timestamp = bundle
                        .segments
                        .iter()
                        .find(|segment| segment.id == marker.segment_id)
                        .map(|segment| format_duration(segment.start_ms))
                        .unwrap_or_else(|| "Evidence".into());
                    if ui.small_button(timestamp).clicked() {
                        *action = UiAction::JumpToEvidence(marker.segment_id.clone());
                    }
                });
            }
        });
    }
}
