//! Selecting a voice reference never trims or moves the original audio.
use super::*;
use crate::media::i18n::tr;

pub(super) fn show(
    controller: &MediaController,
    view: View,
    rect: egui::Rect,
    editable: bool,
    language: crate::i18n::UiLanguage,
    ui: &mut egui::Ui,
) -> Option<Action> {
    let task = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get(id))?;
    let cache = &controller.vocal_waveform;
    let duration = cache
        .waveform
        .as_ref()
        .map_or(task.duration_ms, |wave| wave.duration_ms);
    let id = ui.make_persistent_id(("vocal-reference", &task.id));
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());
    let anchor_id = id.with("anchor");
    let mut anchor = ui.data(|data| data.get_temp::<i64>(anchor_id));
    let time = |x: f32| {
        (view.time(((x - rect.left()) / rect.width()) as f64).round() as i64)
            .clamp(0, duration.max(0))
    };
    if editable
        && response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        anchor = Some(time(origin.x));
        ui.data_mut(|data| data.insert_temp(anchor_id, anchor.unwrap()));
    }
    let mut action = None;
    let mut selected = task.speech_reference_range;
    if let Some(start) = anchor {
        if !editable
            || !ui.input(|input| input.focused)
            || ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            ui.data_mut(|data| data.remove::<i64>(anchor_id));
        } else if let Some(pointer) = response.interact_pointer_pos() {
            let end =
                time(pointer.x).clamp(start.saturating_sub(20_000), start.saturating_add(20_000));
            selected = Some([start.min(end), start.max(end)]);
            if response.drag_stopped_by(egui::PointerButton::Primary)
                || !ui.input(|input| input.pointer.primary_down())
            {
                ui.data_mut(|data| data.remove::<i64>(anchor_id));
                action = Some(Action::VocalReference(selected));
            }
        }
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 6, theme::surface_subtle());
    let x = |time: i64| rect.left() + view.fraction(time as f64) as f32 * rect.width();
    if let Some([start, end]) =
        selected.filter(|[start, end]| *end as f64 > view.start && (*start as f64) < view.end)
    {
        let span = egui::Rect::from_min_max(
            egui::pos2(x(start), rect.top()),
            egui::pos2(x(end), rect.bottom()),
        );
        let color = if end - start >= 5000 {
            theme::primary()
        } else {
            theme::danger()
        };
        painter.rect_filled(span, 4, color.gamma_multiply(0.16));
        if span.width() > 1.0 {
            painter.rect_stroke(
                span.shrink(0.5),
                4,
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
    }
    if let Some(waveform) = &cache.waveform {
        // Aggregate into screen columns, including every bin even when zoomed out.
        let columns = (rect.width() / 2.0).ceil().max(1.0) as usize;
        for column in 0..columns {
            let left = column as f64 / columns as f64;
            let right = (column + 1) as f64 / columns as f64;
            let peak = waveform
                .peak(view.time(left), view.time(right))
                .clamp(0.0, 1.0)
                .sqrt();
            let height = (peak * 10.0).max(0.4);
            let x = rect.left() + (left as f32 + 0.5 / columns as f32) * rect.width();
            painter.line_segment(
                [
                    egui::pos2(x, rect.center().y - height),
                    egui::pos2(x, rect.center().y + height),
                ],
                egui::Stroke::new(1.0, theme::text_weak()),
            );
        }
    } else if cache.loading() {
        egui::Spinner::new().size(16.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0)),
        );
    }
    let position = controller.get_time_ms();
    if (view.start..=view.end).contains(&(position as f64)) {
        painter.line_segment(
            [
                egui::pos2(x(position), rect.top()),
                egui::pos2(x(position), rect.bottom()),
            ],
            egui::Stroke::new(2.0, theme::text_strong()),
        );
    }
    ui_director::egui::record_output(
        ui,
        "Original vocal timeline",
        ui_director::egui::ElementValue::None,
        rect,
    );
    if response.clicked()
        && controller.can_play()
        && let Some(pointer) = response.hover_pos()
    {
        let at = time(pointer.x);
        let [start, end] = selected
            .filter(|[start, end]| (*start..*end).contains(&at))
            .unwrap_or([at, (at + 10_000).min(duration)]);
        if end > start {
            action = Some(Action::AuditionVocals(start, end));
        }
    }
    response
        .on_hover_cursor(if editable {
            egui::CursorIcon::Crosshair
        } else {
            egui::CursorIcon::PointingHand
        })
        .on_hover_ui(|ui| {
            ui.set_max_width(320.0);
            ui.label(tr(language, "Vocals"));
            if let Some([start, end]) = selected {
                ui.weak(format!(
                    "{} – {}",
                    super::super::format_cue_time(start),
                    super::super::format_cue_time(end)
                ));
            }
            ui.label(tr(
                language,
                "Click to listen. Drag to select a 5–20 second voice reference.",
            ));
            if let Some(error) = &cache.error {
                ui.weak(error);
            }
        });
    action
}
