mod highlights;
mod minutes;
mod organization;
mod playback;
mod rows;
mod timeline;
mod transcript;

use super::{
    actions::UiAction,
    presentation::{meeting_language_label, page_header, status_indicator},
};
use crate::plugins::meeting::{
    controller::{MeetingController, MeetingPane, MeetingRoute, can_continue},
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
    let capture = controller
        .active_capture
        .lock()
        .ok()
        .and_then(|capture| capture.clone());
    let active = capture
        .as_ref()
        .filter(|capture| capture.meeting_id == meeting.id);
    let processing_audio = active.is_some_and(|capture| capture.imported_audio);
    let finishing =
        active.is_some() && !processing_audio && meeting.status == MeetingStatus::Processing;
    let mut action = UiAction::None;
    page_header(ui, &meeting.name, language, |ui| {
        if meeting.status == MeetingStatus::Live && active.is_some() && !processing_audio {
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
            if ui
                .add_enabled_ui(latest.is_some() && !marked, |ui| {
                    components::icon_button(
                        ui,
                        tr(
                            language,
                            if marked {
                                "★ Highlight"
                            } else {
                                "Mark latest highlight"
                            },
                        ),
                        egui::include_image!("../../../../../resources/icons/star.svg"),
                    )
                })
                .inner
                .clicked()
                && let Some(segment) = latest
            {
                action = UiAction::Highlight(segment.id.clone());
            }
        }
        if active.is_some() {
            if ui
                .add_enabled_ui(!finishing, |ui| {
                    components::round_icon_button(
                        ui,
                        tr(
                            language,
                            if processing_audio {
                                "Stop processing"
                            } else {
                                "Finish recording"
                            },
                        ),
                        egui::include_image!("../../../../../resources/icons/stop.svg"),
                        36.0,
                    )
                })
                .inner
                .clicked()
            {
                action = UiAction::End;
            }
        }
        if meeting.status == MeetingStatus::Live && active.is_some() && !processing_audio {
            if components::icon_button(
                ui,
                tr(language, "Pause"),
                egui::include_image!("../../../../../resources/icons/pause.svg"),
            )
            .clicked()
            {
                action = UiAction::Pause;
            }
        } else if !processing_audio
            && can_continue(&meeting)
            && ui
                .add_enabled_ui(capture.is_none() || active.is_some(), |ui| {
                    components::round_icon_button(
                        ui,
                        tr(language, "Continue recording"),
                        egui::include_image!("../../../../../resources/icons/record.svg"),
                        36.0,
                    )
                })
                .inner
                .clicked()
        {
            action = UiAction::Continue(meeting.id.clone());
        }
        if components::back_button(ui, tr(language, "Meetings")).clicked() {
            action = UiAction::Back;
        }
    });

    ui.horizontal_wrapped(|ui| {
        status_indicator(ui, language, meeting.status, waiting_for_microphone);
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
        components::icon_menu(
            ui,
            tr(language, "More"),
            egui::include_image!("../../../../../resources/icons/more.svg"),
            |ui| {
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
            },
        );
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
    ui.push_id(&meeting.id, |ui| {
        if controller.pane != MeetingPane::Minutes
            && (active_capture_visible(controller)
                || matches!(
                    meeting.status,
                    MeetingStatus::Live | MeetingStatus::Processing
                )
                || !controller.previews.entries().is_empty())
        {
            render_previews(&controller.previews, ui);
        }
        match controller.pane {
            MeetingPane::Timeline => {
                timeline::render_timeline(controller, language, &mut action, ui)
            }
            MeetingPane::Minutes => minutes::render_minutes(controller, language, &mut action, ui),
            MeetingPane::Transcript => {
                transcript::render_transcript(controller, language, &mut action, ui)
            }
        }
    });
    action
}

fn active_capture_visible(controller: &MeetingController) -> bool {
    controller.bundle.as_ref().is_some_and(|bundle| {
        controller.active_meeting_id().as_deref() == Some(bundle.meeting.id.as_str())
    })
}

fn render_previews(previews: &crate::session_coordinator::LivePreviews, ui: &mut egui::Ui) {
    let height = (ui.available_height() * 0.25).clamp(0.0, 120.0);
    let preview = egui::ScrollArea::vertical()
        .id_salt("meeting_live_preview")
        .max_height(height)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for preview in previews.entries() {
                components::live_preview(ui, preview);
            }
        });
    crate::ui::automation::record_output(
        ui,
        "Meeting preview",
        crate::ui::automation::ElementValue::Number(previews.entries().len() as f64),
        preview.inner_rect,
    );
}

fn timestamp(
    segment: &crate::plugins::meeting::store::Segment,
    can_seek: bool,
    active: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let text = timestamp_text(segment, active);
    if can_seek {
        select_sentence(ui.link(text), &segment.id, true, language, action);
    } else {
        ui.label(text);
    }
}

fn timestamp_text(
    segment: &crate::plugins::meeting::store::Segment,
    active: bool,
) -> egui::RichText {
    egui::RichText::new(super::presentation::format_duration(segment.start_ms))
        .monospace()
        .size(11.0)
        .color(if active {
            crate::ui::theme::primary()
        } else {
            crate::ui::theme::text_weak()
        })
}

fn sentence_text(
    text: impl Into<egui::WidgetText>,
    segment_id: &str,
    can_seek: bool,
    can_merge: bool,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let label = egui::Label::new(text).wrap().selectable(!can_merge);
    let response = ui.add(if can_merge {
        label.sense(egui::Sense::click())
    } else {
        label
    });
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
