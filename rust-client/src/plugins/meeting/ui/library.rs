use super::{
    actions::UiAction,
    presentation::{
        format_timestamp, meeting_language_label, page_header, source_label, status_indicator,
    },
};
use crate::plugins::meeting::{
    controller::{MeetingController, can_continue},
    i18n::tr,
};
use crate::ui::components;
use eframe::egui;

pub(super) fn render_library(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> UiAction {
    let mut action = UiAction::None;
    page_header(ui, "Meeting notes", language, |_| {});
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(24.0, 8.0);
            for (label, icon, next_action) in [
                (
                    "New meeting",
                    egui::include_image!("../../../../resources/icons/record.svg"),
                    UiAction::NewLive,
                ),
                (
                    "Import audio",
                    egui::include_image!("../../../../resources/icons/import.svg"),
                    UiAction::NewImport,
                ),
            ] {
                let label = tr(language, label);
                let diameter = 40.0_f32.max(ui.spacing().interact_size.y);
                let label_width = ui
                    .painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        egui::FontId::proportional(12.0),
                        crate::ui::theme::text_normal(),
                    )
                    .size()
                    .x;
                let entry = ui.allocate_ui_with_layout(
                    egui::vec2(diameter + 8.0 + label_width, diameter),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let clicked =
                            components::round_icon_button(ui, label, icon, diameter).clicked();
                        let caption = ui.add(
                            egui::Label::new(
                                egui::RichText::new(label)
                                    .size(12.0)
                                    .color(crate::ui::theme::text_normal()),
                            )
                            .sense(egui::Sense::click()),
                        );
                        clicked || caption.clicked()
                    },
                );
                if entry.inner {
                    action = next_action;
                }
            }
        });
    });
    ui.add_space(16.0);

    if let Some(error) = &controller.error {
        components::error_notice(ui, language, error);
        ui.add_space(10.0);
    }

    if controller.meetings.is_empty() {
        return action;
    }
    let limit = components::library_navigation(
        ui,
        language,
        &mut controller.browse_all,
        &mut controller.search,
        controller.meetings.len(),
        tr(language, "Search meetings"),
    );

    let query = controller.search.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("meeting_library_scroll")
        .show(ui, |ui| {
            for meeting in controller
                .meetings
                .iter()
                .filter(|meeting| query.is_empty() || meeting.name.to_lowercase().contains(&query))
                .take(limit)
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
                            status_indicator(ui, language, meeting.status, false);
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
                            if components::open_button(ui, tr(language, "Open")).clicked() {
                                action = UiAction::Open(meeting.id.clone());
                            }
                            components::icon_menu(
                                ui,
                                tr(language, "More"),
                                egui::include_image!("../../../../resources/icons/more.svg"),
                                |ui| {
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
                                },
                            );
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
