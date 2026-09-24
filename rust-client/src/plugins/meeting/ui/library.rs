use super::{
    actions::UiAction,
    presentation::{format_timestamp, meeting_language_label, page_header, source_label},
};
use crate::plugins::meeting::{
    controller::{MeetingController, can_continue, meeting_status_label},
    i18n::tr,
    store::{MeetingSourceKind, MeetingStatus},
};
use crate::ui::components;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Stroke};

pub(super) fn render_library(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> UiAction {
    let mut action = UiAction::None;
    page_header(ui, "Meeting notes", language, |ui| {
        if components::primary_button(ui, tr(language, "New meeting")).clicked() {
            action = UiAction::NewLive;
        }
        if components::animated_button(ui, tr(language, "Import audio")).clicked() {
            action = UiAction::NewImport;
        }
    });

    if let Some(error) = &controller.error {
        components::error_notice(ui, language, error);
        ui.add_space(10.0);
    }

    components::search_bar(ui, &mut controller.search, tr(language, "Search meetings"));

    ui.add_space(12.0);

    if controller.meetings.is_empty() {
        components::card(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(tr(language, "No meetings yet"))
                        .size(16.0)
                        .strong()
                        .color(crate::ui::theme::text_strong()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(tr(
                        language,
                        "Create a live record or import an audio file. Records are stored locally.",
                    ))
                    .size(12.0)
                    .color(crate::ui::theme::text_weak()),
                );
                ui.add_space(16.0);
            });
        });
        return action;
    }

    let query = controller.search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("meeting_library_scroll")
        .show(ui, |ui| {
            for meeting in controller
                .meetings
                .iter()
                .filter(|meeting| query.is_empty() || meeting.name.to_lowercase().contains(&query))
            {
                ui.push_id(("meeting-card", &meeting.id), |ui| {
                    components::card(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let (badge_bg, badge_fg, badge_text) = match meeting.source_kind {
                                    MeetingSourceKind::LiveCapture => (
                                        Color32::from_rgb(236, 253, 245),
                                        Color32::from_rgb(5, 150, 105),
                                        "LIVE",
                                    ),
                                    MeetingSourceKind::ImportedAudio => (
                                        Color32::from_rgb(238, 242, 255),
                                        Color32::from_rgb(79, 70, 229),
                                        "FILE",
                                    ),
                                };

                                Frame::new()
                                    .fill(badge_bg)
                                    .corner_radius(CornerRadius::same(6))
                                    .inner_margin(Margin::symmetric(6, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(badge_text)
                                                .color(badge_fg)
                                                .strong()
                                                .size(11.0),
                                        );
                                    });

                                ui.add_space(8.0);

                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&meeting.name)
                                            .size(15.5)
                                            .strong()
                                            .color(crate::ui::theme::text_strong()),
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {} → {} · {}",
                                            source_label(meeting, language),
                                            meeting_language_label(&meeting.source_language, language),
                                            meeting_language_label(&meeting.target_language, language),
                                            format_timestamp(meeting.last_activity_at_ms)
                                        ))
                                        .color(crate::ui::theme::text_weak())
                                        .size(11.5),
                                    );
                                });

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let status_text = meeting_status_label(meeting.status);
                                        components::status_badge(
                                            ui,
                                            status_text,
                                            meeting.status == MeetingStatus::Live,
                                            meeting.status == MeetingStatus::Failed,
                                        );
                                    },
                                );
                            });

                            ui.add_space(10.0);

                            ui.horizontal_wrapped(|ui| {
                                if components::primary_button(ui, tr(language, "Open")).clicked() {
                                    action = UiAction::Open(meeting.id.clone());
                                }
                                if can_continue(meeting)
                                    && components::animated_button(
                                        ui,
                                        tr(language, "Continue recording"),
                                    )
                                    .clicked()
                                {
                                    action = UiAction::Continue(meeting.id.clone());
                                }
                                if components::animated_button(
                                    ui,
                                    tr(language, "Export Markdown"),
                                )
                                .clicked()
                                {
                                    action = UiAction::ExportMeeting(meeting.id.clone());
                                }
                                let active = controller.is_recording(&meeting.id);
                                let delete = ui
                                    .add_enabled_ui(!active, |ui| {
                                        components::danger_button(ui, tr(language, "Delete"))
                                    })
                                    .inner;
                                if active {
                                    delete.on_hover_text(tr(
                                        language,
                                        "Finish the active meeting before deleting it",
                                    ));
                                } else if delete.clicked() {
                                    action = UiAction::AskDelete(meeting.id.clone());
                                }
                            });

                            if controller.pending_delete.as_deref() == Some(&meeting.id) {
                                ui.add_space(8.0);
                                Frame::new()
                                    .fill(Color32::from_rgb(254, 242, 242))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(254, 202, 202)))
                                    .corner_radius(CornerRadius::same(10))
                                    .inner_margin(Margin::same(12))
                                    .show(ui, |ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(tr(
                                                    language,
                                                    "Delete this meeting and all of its local records?",
                                                ))
                                                .color(Color32::from_rgb(185, 28, 28))
                                                .strong()
                                                .size(12.5),
                                            );
                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                if components::danger_button(
                                                    ui,
                                                    tr(language, "Delete permanently"),
                                                )
                                                .clicked()
                                                {
                                                    action = UiAction::Delete(meeting.id.clone());
                                                }
                                                if components::animated_button(
                                                    ui,
                                                    tr(language, "Cancel"),
                                                )
                                                .clicked()
                                                {
                                                    action = UiAction::CancelDelete;
                                                }
                                            });
                                        });
                                    });
                            }
                        });
                    });
                    ui.add_space(10.0);
                });
            }
        });
    action
}
