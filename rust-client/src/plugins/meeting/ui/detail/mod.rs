mod minutes;
mod organization;
mod timeline;
mod transcript;

use super::{
    actions::UiAction,
    presentation::{meeting_language_label, page_header},
};
use crate::plugins::meeting::{
    controller::{
        MeetingController, MeetingPane, MeetingRoute, can_continue, meeting_status_label,
    },
    i18n::tr,
    store::MeetingStatus,
};
use crate::ui::components;
use eframe::egui;

pub(super) fn render_detail(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    waiting_for_microphone: bool,
    ui: &mut egui::Ui,
) -> UiAction {
    let Some(bundle) = controller.bundle.as_ref() else {
        controller.route = MeetingRoute::Library;
        return UiAction::None;
    };
    let meeting = bundle.meeting.clone();
    let mut action = UiAction::None;
    page_header(ui, &meeting.name, language, |ui| {
        if meeting.status == MeetingStatus::Live && controller.is_recording(&meeting.id) {
            if components::danger_button(ui, tr(language, "Finish recording")).clicked() {
                action = UiAction::End;
            }
            if components::animated_button(ui, tr(language, "Pause")).clicked() {
                action = UiAction::Pause;
            }
        } else if can_continue(&meeting)
            && components::primary_button(ui, tr(language, "Continue recording")).clicked()
        {
            action = UiAction::Continue(meeting.id.clone());
        }
        if components::animated_button(ui, tr(language, "Meetings")).clicked() {
            action = UiAction::Back;
        }
    });

    ui.horizontal_wrapped(|ui| {
        components::status_badge(
            ui,
            if waiting_for_microphone && meeting.status == MeetingStatus::Live {
                crate::i18n::tr(language, "Waiting for microphone")
            } else {
                tr(language, meeting_status_label(meeting.status))
            },
            meeting.status == MeetingStatus::Live && !waiting_for_microphone,
            meeting.status == MeetingStatus::Failed,
        );
        ui.label(
            egui::RichText::new(format!(
                "{} → {}",
                meeting_language_label(&meeting.source_language, language),
                meeting_language_label(&meeting.target_language, language)
            ))
            .color(crate::ui::theme::text_weak())
            .size(12.0),
        );
    });
    if let Some(error) = &controller.error {
        components::error_notice(ui, language, error);
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(
                controller.pane != MeetingPane::Minutes,
                tr(language, "Transcript"),
            )
            .clicked()
        {
            controller.pane = MeetingPane::Timeline;
        }
        ui.selectable_value(
            &mut controller.pane,
            MeetingPane::Minutes,
            tr(language, "Organize"),
        );
        ui.menu_button("⋯", |ui| {
            if ui.button(tr(language, "Export Markdown")).clicked() {
                action = UiAction::Export;
                ui.close();
            }
            if ui.button(tr(language, "Full transcript")).clicked() {
                controller.pane = MeetingPane::Transcript;
                ui.close();
            }
            if meeting.can_reprocess
                && ui
                    .add_enabled(
                        controller.active_meeting_id().is_none(),
                        egui::Button::new(tr(language, "Reprocess audio")),
                    )
                    .clicked()
            {
                action = UiAction::Reprocess;
                ui.close();
            }
        })
        .response
        .on_hover_text(tr(language, "More"));
    });
    ui.add_space(8.0);

    match controller.pane {
        MeetingPane::Timeline => timeline::render_timeline(controller, language, &mut action, ui),
        MeetingPane::Minutes => minutes::render_minutes(controller, language, &mut action, ui),
        MeetingPane::Transcript => transcript::render_transcript(controller, language, ui),
    }
    action
}
