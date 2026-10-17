mod view;

use crate::{
    plugins::media::controller::MediaController,
    ui::{animation::AnimationSystem, theme},
};
use eframe::egui;
use view::View;

pub(super) fn overview(controller: &MediaController, ui: &mut egui::Ui) -> Option<usize> {
    let cues = controller.subtitles.cues();
    let last = cues.len().checked_sub(1)?;
    let duration = controller
        .get_duration_ms()
        .max(controller.subtitles.effective_end_at(last))
        .max(cues[last].start_ms)
        .max(1);
    let (labels, _) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            ui.text_style_height(&egui::TextStyle::Body),
        ),
        egui::Sense::hover(),
    );
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    let position = controller.get_time_ms();
    let id = ui.make_persistent_id(("subtitle_window", &controller.active_task_id));
    let previous = ui.data(|data| data.get_temp::<View>(id));
    let frozen = previous.filter(|_| response.hovered() || response.is_pointer_button_down_on());
    let mut view = if !controller.can_play() {
        View::full(duration)
    } else {
        frozen.unwrap_or_else(|| View::following(cues, duration, position as f64, rect.width()))
    };
    if controller.can_play() {
        // Animate the final window as well as the focus: adding cues or crossing
        // repeated timestamps must not abruptly change the displayed time range.
        for (name, value) in [
            ("start", &mut view.start),
            ("end", &mut view.end),
            ("focus", &mut view.focus),
        ] {
            let animated = AnimationSystem::animate_value(
                ui.ctx(),
                id.with(name),
                *value as f32 / 1000.0,
                if frozen.is_some() {
                    0.0
                } else {
                    theme::animation_timings(ui.ctx()).selection
                },
            );
            // Keep preview and repeated click geometry exact, even after seeking.
            if frozen.is_none() {
                *value = animated as f64 * 1000.0;
            }
        }
    }
    ui.data_mut(|data| data.insert_temp(id, view));
    for (time, position, align) in [
        (view.start, labels.left_center(), egui::Align2::LEFT_CENTER),
        (view.end, labels.right_center(), egui::Align2::RIGHT_CENTER),
    ] {
        ui.painter().text(
            position,
            align,
            super::format_time_ms(time.round() as i64),
            egui::TextStyle::Body.resolve(ui.style()),
            theme::text_weak(),
        );
    }
    crate::ui::automation::record_output(
        ui,
        "Subtitle window",
        crate::ui::automation::ElementValue::Text(format!(
            "{} – {}",
            super::format_cue_time(view.start.round() as i64),
            super::format_cue_time(view.end.round() as i64),
        )),
        rect,
    );
    ui.painter().rect_filled(rect, 6, theme::surface_subtle());
    let x = |time: i64| rect.left() + view.fraction(time as f64) as f32 * rect.width();
    let current = controller
        .can_play()
        .then(|| controller.subtitles.active_cue_at(position))
        .flatten();
    let range = view.visible_range(cues);
    let mut index = range.start;
    while index < range.end {
        let cue = &cues[index];
        let end_ms = controller.subtitles.effective_end_at(index);
        if end_ms as f64 <= view.start {
            index += 1;
            continue;
        }
        let start = x(cue.start_ms);
        let end = x(end_ms);
        // Subpixel cues share a mark. Keep the last one in the pixel because
        // it may extend further; this also skips coincident timestamps cheaply.
        let next = if end - start < 1.0 {
            let next_time = view.time(((start + 1.0 - rect.left()) / rect.width()) as f64);
            cues.partition_point(|cue| cue.start_ms as f64 <= next_time)
                .saturating_sub(1)
                .max(index + 1)
        } else {
            index + 1
        };
        let span = egui::Rect::from_min_max(
            egui::pos2(start, rect.top() + 4.0),
            egui::pos2(
                (end - 2.0).max(start + 1.0).min(rect.right()),
                rect.bottom() - 4.0,
            ),
        );
        let active = current.is_some_and(|current| {
            current.id == cue.id
                || (current.start_ms >= cue.start_ms
                    && cues
                        .get(next)
                        .is_none_or(|next| current.start_ms < next.start_ms))
        });
        ui.painter().rect_filled(
            span,
            2,
            theme::primary().gamma_multiply(if active { 1.0 } else { 0.4 }),
        );
        index = next;
    }
    if controller.can_play() && (view.start..=view.end).contains(&(position as f64)) {
        let position = x(position);
        ui.painter().line_segment(
            [
                egui::pos2(position, rect.top()),
                egui::pos2(position, rect.bottom()),
            ],
            egui::Stroke::new(2.0, theme::text_strong()),
        );
    }
    let pointer = response.hover_pos()?;
    let time = view
        .time(((pointer.x - rect.left()) / rect.width()) as f64)
        .round() as i64;
    let index = cues
        .partition_point(|cue| cue.start_ms <= time)
        .saturating_sub(1);
    let clicked = response.clicked();
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_ui(|ui| {
            ui.set_max_width(300.0);
            ui.weak(format!(
                "{} – {}",
                super::format_cue_time(cues[index].start_ms),
                super::format_cue_time(controller.subtitles.effective_end_at(index)),
            ));
            ui.label(&cues[index].original_text);
        });
    clicked.then_some(index)
}
