use super::{super::actions::UiAction, rows};
use crate::plugins::meeting::{controller::MeetingController, store::Segment};
use crate::ui::virtual_list;
use eframe::egui;
use std::sync::Arc;

pub(super) fn render_transcript(
    controller: &mut MeetingController,
    language: crate::i18n::UiLanguage,
    action: &mut UiAction,
    ui: &mut egui::Ui,
) {
    let live = super::active_capture_visible(controller);
    let can_seek = controller.active_meeting_id().is_none();
    let can_merge = controller.can_merge_segments();
    let Some(bundle) = controller.bundle.as_ref() else {
        return;
    };
    let active_id = controller
        .playback
        .current_segment(&bundle.segments)
        .map(|segment| segment.id.clone());
    let evidence = controller.evidence_target.as_deref().and_then(|target| {
        bundle
            .segments
            .iter()
            .position(|segment| segment.id == target)
    });
    let revision = egui::Id::new((&bundle.meeting.id, controller.bundle_revision()));
    let measurements_id = egui::Id::new("meeting_full_transcript_measurements");
    let mut measurements = ui
        .data_mut(|data| data.remove_temp::<virtual_list::Measurements<usize>>(measurements_id))
        .unwrap_or_default();
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
    let merged = virtual_list::show_subtitle_rows(
        ui,
        "meeting_full_transcript",
        8.0,
        target,
        |row: &virtual_list::MeasuredRow<usize>| row.id,
        virtual_list::SubtitleMerge {
            enabled: can_merge,
            language,
            index: &|row: &virtual_list::MeasuredRow<usize>| Some(row.entry),
            text: &|index| bundle.segments[index].original_text.as_str(),
        },
        |ui| {
            let prepared = measurements.prepare(
                ui,
                revision,
                language,
                || {
                    bundle
                        .segments
                        .iter()
                        .enumerate()
                        .map(|(index, segment)| virtual_list::MeasuredRow {
                            id: egui::Id::new((
                                "meeting_transcript",
                                &bundle.meeting.id,
                                &segment.id,
                            )),
                            signature: egui::Id::new((
                                &segment.original_text,
                                &segment.translated_text,
                                segment.start_ms,
                            )),
                            entry: index,
                        })
                        .collect()
                },
                |row| {
                    segment_text(ui, &bundle.segments[row.entry], false)
                        .size()
                        .y
                        + 8.0
                },
            );
            rows::remember_geometry(ui, geometry_id, &prepared, 8.0, viewport.height());
            prepared
        },
        |ui, _, height, row| {
            let segment = &bundle.segments[row.entry];
            let active = active_id.as_deref() == Some(segment.id.as_str());
            let text = segment_text(ui, segment, active);
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
                            text,
                            &segment.id,
                            can_seek && controller.playback.can_seek_segment(segment),
                            can_merge,
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
            rows::record(ui, &segment.id, rect);
        },
    );
    if let Some((source, target)) = merged {
        *action = UiAction::MergeSegments(
            bundle.segments[source].id.clone(),
            bundle.segments[target].id.clone(),
        );
    }
    ui.data_mut(|data| data.insert_temp(measurements_id, measurements));
    if evidence.is_some() {
        controller.evidence_target = None;
    }
}

fn segment_text(ui: &egui::Ui, segment: &Segment, active: bool) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let mut append = |text: egui::RichText| {
        text.append_to(
            &mut job,
            ui.style(),
            egui::FontSelection::Default,
            egui::Align::Center,
        )
    };
    append(super::timestamp_text(segment, active));
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
    ui.painter().layout_job(job)
}
