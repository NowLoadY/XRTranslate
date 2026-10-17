use super::{super::actions::UiAction, rows};
use crate::plugins::meeting::controller::MeetingController;
use eframe::egui;
use std::sync::Arc;

struct Row {
    id: egui::Id,
    text: Arc<egui::Galley>,
}

pub(super) fn render_transcript(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let live = super::active_capture_visible(controller);
    let can_seek = controller.active_meeting_id().is_none();
    let Some(bundle) = controller.bundle.as_ref() else {
        return;
    };
    let active_id = controller
        .playback
        .current_segment(&bundle.segments)
        .map(|segment| segment.id.clone());
    let evidence = bundle
        .segments
        .iter()
        .position(|segment| controller.evidence_target.as_deref() == Some(segment.id.as_str()));
    let scroll_id = ui.make_persistent_id(egui::IdSalt::new("meeting_full_transcript"));
    let geometry_id = scroll_id.with("geometry");
    let target = rows::scroll_target(ui, scroll_id, geometry_id, live, evidence);
    let viewport = ui.available_rect_before_wrap();
    crate::ui::automation::record_output(
        ui,
        "Meeting transcript",
        crate::ui::automation::ElementValue::Number(bundle.segments.len() as f64),
        viewport,
    );
    crate::ui::layout::show_variable_virtual_rows(
        ui,
        "meeting_full_transcript",
        8.0,
        target,
        Some(|row: &Row| row.id),
        |ui| {
            let prepared = bundle
                .segments
                .iter()
                .map(|segment| {
                    let mut job = egui::text::LayoutJob::default();
                    let mut append = |text: egui::RichText| {
                        text.append_to(
                            &mut job,
                            ui.style(),
                            egui::FontSelection::Default,
                            egui::Align::Center,
                        )
                    };
                    append(super::timestamp_text(
                        segment,
                        active_id.as_deref() == Some(segment.id.as_str()),
                    ));
                    append(egui::RichText::new("  "));
                    append(
                        egui::RichText::new(&segment.original_text)
                            .size(12.5)
                            .color(crate::ui::theme::text_normal()),
                    );
                    if let Some(text) = &segment.translated_text {
                        append(egui::RichText::new("  "));
                        append(
                            egui::RichText::new(text)
                                .strong()
                                .size(13.0)
                                .color(crate::ui::theme::text_strong()),
                        );
                    }
                    job.wrap.max_width = (ui.available_width() - 8.0).max(1.0);
                    let text = ui.painter().layout_job(job);
                    (
                        text.size().y + 8.0,
                        Row {
                            id: egui::Id::new((
                                "meeting_transcript",
                                &bundle.meeting.id,
                                &segment.id,
                            )),
                            text,
                        },
                    )
                })
                .collect::<Vec<_>>();
            rows::remember_geometry(ui, geometry_id, &prepared, 8.0, viewport.height());
            prepared
        },
        |ui, index, height, row| {
            let segment = &bundle.segments[index];
            let active = active_id.as_deref() == Some(segment.id.as_str());
            let rect = egui::Rect::from_min_size(
                ui.next_widget_position(),
                egui::vec2(ui.available_width(), height),
            );
            ui.push_id(row.id, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 8,
                        ..egui::Margin::ZERO
                    })
                    .show(ui, |ui| {
                        super::sentence_text(
                            row.text.clone(),
                            &segment.id,
                            can_seek && controller.playback.can_seek_segment(segment),
                            language,
                            action,
                            ui,
                        );
                    });
            });
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 2.0,
                egui::Stroke::new(1.0, crate::ui::theme::border()),
            );
            super::current_marker(ui, rect, active);
            rows::record(ui, &segment.id, rect);
        },
    );
    if evidence.is_some() {
        controller.evidence_target = None;
    }
}
