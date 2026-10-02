use super::{
    actions::UiAction,
    presentation::{format_timestamp, meeting_language_label, page_header, source_label},
};
use crate::plugins::meeting::{
    controller::{MeetingController, can_continue, meeting_status_label},
    i18n::tr,
    store::MeetingStatus,
};
use crate::ui::components;
use eframe::egui;

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
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&meeting.name)
                                    .size(16.0)
                                    .strong()
                                    .color(crate::ui::theme::text_strong()),
                            )
                            .wrap(),
                        );
                        ui.horizontal_wrapped(|ui| {
                            components::status_badge(
                                ui,
                                tr(language, meeting_status_label(meeting.status)),
                                meeting.status == MeetingStatus::Live,
                                meeting.status == MeetingStatus::Failed,
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {}",
                                    source_label(meeting, language),
                                    format_timestamp(meeting.last_activity_at_ms)
                                ))
                                .size(11.5)
                                .color(crate::ui::theme::text_weak()),
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "{} → {}",
                                meeting_language_label(&meeting.source_language, language),
                                meeting_language_label(&meeting.target_language, language)
                            ))
                            .size(12.0)
                            .color(crate::ui::theme::text_weak()),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if components::primary_button(ui, tr(language, "Open")).clicked() {
                                action = UiAction::Open(meeting.id.clone());
                            }
                            ui.menu_button("⋯", |ui| {
                                if can_continue(meeting)
                                    && ui.button(tr(language, "Continue recording")).clicked()
                                {
                                    action = UiAction::Continue(meeting.id.clone());
                                    ui.close();
                                }
                                if ui.button(tr(language, "Export Markdown")).clicked() {
                                    action = UiAction::ExportMeeting(meeting.id.clone());
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        !controller.is_recording(&meeting.id),
                                        egui::Button::new(tr(language, "Delete")),
                                    )
                                    .clicked()
                                {
                                    action = UiAction::AskDelete(meeting.id.clone());
                                    ui.close();
                                }
                            })
                            .response
                            .on_hover_text(tr(language, "More"));
                        });
                    });
                    ui.add_space(10.0);
                });
            }
        });
    if let Some(meeting) = controller
        .pending_delete
        .as_ref()
        .and_then(|id| controller.meetings.iter().find(|meeting| &meeting.id == id))
    {
        let message = format!(
            "{}\n\n{}",
            meeting.name,
            tr(
                language,
                "Delete this meeting and all of its local records?"
            ),
        );
        if let Some(confirmed) = crate::ui::modal::confirm(
            ui.ctx(),
            ui.make_persistent_id("meeting_delete_confirmation"),
            language,
            tr(language, "Delete permanently"),
            &message,
            tr(language, "Delete permanently"),
            !controller.is_recording(&meeting.id),
        ) {
            action = if confirmed {
                UiAction::Delete(meeting.id.clone())
            } else {
                UiAction::CancelDelete
            };
        }
    }
    action
}
