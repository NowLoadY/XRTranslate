mod minutes;
mod timeline;
mod transcript;

use minutes::render_minutes;
use timeline::render_timeline;
use transcript::render_transcript;

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
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

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
            if components::danger_button(ui, tr(language, "End meeting")).clicked() {
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
                meeting_status_label(meeting.status)
            },
            meeting.status == MeetingStatus::Live && !waiting_for_microphone,
            meeting.status == MeetingStatus::Failed,
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!(
                "{} → {}",
                meeting_language_label(&meeting.source_language, language),
                meeting_language_label(&meeting.target_language, language)
            ))
            .color(crate::ui::theme::text_weak())
            .size(12.0),
        );
        if meeting.can_reprocess {
            ui.add_space(8.0);
            Frame::new()
                .fill(Color32::from_rgb(240, 253, 244))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(tr(language, "Audio available for reprocessing"))
                            .color(Color32::from_rgb(22, 101, 52))
                            .size(11.0),
                    );
                });
        }
    });

    ui.add_space(10.0);

    // Segmented Tab Switcher Bar
    Frame::new()
        .fill(Color32::from_rgb(241, 245, 249))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let tabs = [
                    (MeetingPane::Timeline, "Timeline"),
                    (MeetingPane::Minutes, "Minutes"),
                    (MeetingPane::Transcript, "Full transcript"),
                ];
                for (pane, key) in tabs {
                    let selected = controller.pane == pane;
                    let bg = if selected {
                        Color32::WHITE
                    } else {
                        Color32::TRANSPARENT
                    };
                    let stroke = if selected {
                        Stroke::new(1.0, Color32::from_rgb(226, 232, 240))
                    } else {
                        Stroke::NONE
                    };
                    let fg = if selected {
                        Color32::from_rgb(37, 99, 235)
                    } else {
                        crate::ui::theme::text_weak()
                    };

                    let mut text = egui::RichText::new(tr(language, key)).color(fg).size(12.5);
                    if selected {
                        text = text.strong();
                    }

                    if ui
                        .add(
                            egui::Button::new(text)
                                .fill(bg)
                                .stroke(stroke)
                                .corner_radius(CornerRadius::same(7))
                                .wrap_mode(egui::TextWrapMode::Extend),
                        )
                        .clicked()
                    {
                        controller.pane = pane;
                    }
                }

                if components::animated_button(ui, tr(language, "Export Markdown")).clicked() {
                    action = UiAction::Export;
                }
                if meeting.can_reprocess
                    && components::animated_button(ui, tr(language, "Reprocess audio")).clicked()
                {
                    action = UiAction::Reprocess;
                }
            });
        });

    ui.add_space(12.0);

    match controller.pane {
        MeetingPane::Timeline => render_timeline(controller, language, &mut action, ui),
        MeetingPane::Minutes => render_minutes(controller, language, &mut action, ui),
        MeetingPane::Transcript => render_transcript(controller, language, ui),
    }
    action
}
