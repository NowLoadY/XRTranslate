use crate::{plugins::media::controller::MediaController, ui::theme};
use eframe::egui;

pub(super) fn overview(controller: &MediaController, ui: &mut egui::Ui) -> Option<usize> {
    let cues = controller.subtitles.cues();
    let last = cues.len().checked_sub(1)?;
    let duration = controller
        .get_duration_ms()
        .max(controller.subtitles.effective_end_at(last))
        .max(cues[last].start_ms)
        .max(1);
    ui.horizontal(|ui| {
        ui.weak(super::format_time_ms(0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(super::format_time_ms(duration));
        });
    });
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 6, theme::surface_subtle());
    let x = |time: i64| {
        rect.left() + (time as f64 / duration as f64).clamp(0.0, 1.0) as f32 * rect.width()
    };
    let current = controller
        .can_play()
        .then(|| controller.subtitles.active_cue_at(controller.get_time_ms()))
        .flatten();
    for (index, cue) in cues.iter().enumerate() {
        let start = x(cue.start_ms);
        let end = x(controller.subtitles.effective_end_at(index));
        let span = egui::Rect::from_min_max(
            egui::pos2(start, rect.top() + 4.0),
            egui::pos2(
                (end - 0.5).max(start + 1.0).min(rect.right()),
                rect.bottom() - 4.0,
            ),
        );
        let active = current.is_some_and(|current| current.id == cue.id);
        ui.painter().rect_filled(
            span,
            2,
            theme::primary().gamma_multiply(if active { 1.0 } else { 0.4 }),
        );
    }
    if controller.can_play() {
        let position = x(controller.get_time_ms());
        ui.painter().line_segment(
            [
                egui::pos2(position, rect.top()),
                egui::pos2(position, rect.bottom()),
            ],
            egui::Stroke::new(2.0, theme::text_strong()),
        );
    }
    let pointer = response.hover_pos()?;
    let time = ((pointer.x - rect.left()) / rect.width() * duration as f32) as i64;
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
