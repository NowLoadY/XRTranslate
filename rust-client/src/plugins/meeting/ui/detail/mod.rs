mod highlights;
mod minutes;
mod organization;
mod playback;
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
            let latest = controller
                .bundle
                .as_ref()
                .and_then(|bundle| bundle.segments.last());
            let marked = latest.is_some_and(|segment| {
                controller.bundle.as_ref().is_some_and(|bundle| {
                    bundle
                        .markers
                        .iter()
                        .any(|marker| marker.segment_id == segment.id)
                })
            });
            if components::primary_button_enabled(
                ui,
                tr(
                    language,
                    if marked {
                        "★ Highlight"
                    } else {
                        "Mark latest highlight"
                    },
                ),
                latest.is_some() && !marked,
            )
            .clicked()
                && let Some(segment) = latest
            {
                action = UiAction::Highlight(segment.id.clone());
            }
        }
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
    playback::render(controller, language, ui);
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

    if controller.pane != MeetingPane::Minutes
        && ui.rect_contains_pointer(ui.available_rect_before_wrap())
        && ui.input(|input| {
            input.smooth_scroll_delta.y.abs() > 0.1 || input.pointer.is_decidedly_dragging()
        })
    {
        controller.playback.follow = false;
    }
    let active = controller
        .bundle
        .as_ref()
        .and_then(|bundle| controller.playback.current_segment(&bundle.segments))
        .map(|segment| segment.id.clone());
    if controller.playback.follow
        && controller.evidence_target.is_none()
        && controller.search.trim().is_empty()
        && !controller.highlights_only
        && controller.pane != MeetingPane::Minutes
        && active != controller.playback.last_followed
    {
        controller.evidence_target = active.clone();
        controller.playback.last_followed = active;
    }
    match controller.pane {
        MeetingPane::Timeline => timeline::render_timeline(controller, language, &mut action, ui),
        MeetingPane::Minutes => minutes::render_minutes(controller, language, &mut action, ui),
        MeetingPane::Transcript => {
            transcript::render_transcript(controller, language, &mut action, ui)
        }
    }
    action
}

fn preview_matches(
    preview: &crate::session_coordinator::TranslationPreview,
    segment: &crate::plugins::meeting::store::Segment,
) -> bool {
    preview.segment_index != 0
        && preview.source == segment.original_text
        && segment
            .external_key
            .ends_with(&format!(":{}:{}", preview.turn_id, preview.segment_index))
}

fn segment_preview<'a>(
    previews: &'a crate::session_coordinator::LivePreviews,
    segment: &crate::plugins::meeting::store::Segment,
) -> Option<&'a crate::session_coordinator::TranslationPreview> {
    previews
        .entries()
        .iter()
        .find(|preview| preview_matches(preview, segment))
}

fn render_unstored_previews(
    ui: &mut egui::Ui,
    previews: &crate::session_coordinator::LivePreviews,
    segments: &[crate::plugins::meeting::store::Segment],
) {
    for preview in previews.entries() {
        if !segments
            .iter()
            .any(|segment| preview_matches(preview, segment))
        {
            crate::ui::components::live_preview(ui, preview);
        }
    }
}

fn timestamp(
    segment: &crate::plugins::meeting::store::Segment,
    can_seek: bool,
    active: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let text = egui::RichText::new(super::presentation::format_duration(segment.start_ms))
        .monospace()
        .size(11.0)
        .color(if active {
            crate::ui::theme::primary()
        } else {
            crate::ui::theme::text_weak()
        });
    if can_seek {
        select_sentence(ui.link(text), &segment.id, true, language, action);
    } else {
        ui.label(text);
    }
}

fn sentence_text(
    text: egui::RichText,
    segment_id: &str,
    can_seek: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let response = ui.add(egui::Label::new(text).wrap().selectable(true));
    select_sentence(response, segment_id, can_seek, language, action);
}

fn select_sentence(
    response: egui::Response,
    segment_id: &str,
    can_seek: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
) {
    if can_seek
        && response
            .on_hover_text(tr(language, "Locate in recording"))
            .clicked()
    {
        *action = UiAction::SelectSegment(segment_id.to_owned());
    }
}

fn current_marker(ui: &egui::Ui, rect: egui::Rect, active: bool) {
    if active {
        ui.painter().vline(
            rect.left() + 4.0,
            rect.top() + 4.0..=rect.bottom() - 4.0,
            egui::Stroke::new(2.0, crate::ui::theme::primary()),
        );
    }
}
