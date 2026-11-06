#[cfg(test)]
mod tests;
mod view;

use crate::{
    media::controller::{MediaController, SubtitleField},
    ui::{animation::AnimationSystem, theme},
};
use eframe::egui;
use view::View;

pub(super) enum Action {
    Select(usize),
    Trim {
        cue: String,
        edge: SubtitleField,
        time: i64,
        finished: bool,
    },
    MoveSpeech {
        clip: String,
        start: i64,
        position: i64,
        finished: bool,
    },
    AuditionSpeech(String),
    DeleteSpeech(String),
    CancelTrim,
}

impl Action {
    pub(super) fn apply(self, controller: &mut MediaController) {
        match self {
            Self::Trim {
                cue,
                edge,
                time,
                finished,
            } => controller.trim_subtitle(&cue, edge, time, finished),
            Self::MoveSpeech {
                clip,
                start,
                position,
                finished,
            } => controller.move_speech(&clip, start, position, finished),
            Self::AuditionSpeech(clip) => controller.audition_speech(&clip),
            Self::DeleteSpeech(clip) => controller.delete_speech(&clip),
            Self::CancelTrim => controller.cancel_subtitle_trim(),
            Self::Select(_) => {}
        }
    }
}

#[derive(Clone)]
enum DragKind {
    Subtitle { cue: String, edge: SubtitleField },
    Speech { clip: String, grab: f64 },
}

#[derive(Clone)]
struct Drag {
    kind: DragKind,
    time: f64,
    pointer_x: f32,
    offset: f32,
}

pub(super) fn overview(
    controller: &MediaController,
    subtitles: &crate::media::subtitles::SubtitleTimeline,
    editable: bool,
    ui: &mut egui::Ui,
) -> Option<Action> {
    let cues = subtitles.cues();
    let task = controller
        .active_task_id
        .as_deref()
        .and_then(|id| controller.store.get(id));
    let speech = task.and_then(|task| task.speech_track.as_ref());
    let clips = speech.map_or(&[][..], |track| track.clips.as_slice());
    if cues.is_empty() && clips.is_empty() {
        return None;
    }
    let duration = controller
        .get_duration_ms()
        .max(task.map_or(0, |task| task.duration_ms))
        .max(
            cues.len()
                .checked_sub(1)
                .map_or(0, |last| subtitles.effective_end_at(last)),
        )
        .max(clips.iter().map(|clip| clip.end_ms()).max().unwrap_or(0))
        .max(1);
    let (labels, _) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            ui.text_style_height(&egui::TextStyle::Body),
        ),
        egui::Sense::hover(),
    );
    let (full_rect, response) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            if clips.is_empty() { 24.0 } else { 56.0 },
        ),
        egui::Sense::click_and_drag(),
    );
    let rect = egui::Rect::from_min_size(full_rect.min, egui::vec2(full_rect.width(), 24.0));
    let speech_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(0.0, 28.0),
        egui::vec2(rect.width(), 28.0),
    );
    let position = controller.get_time_ms();
    let id = ui.make_persistent_id(("subtitle_window", &controller.active_task_id));
    let previous = ui.data(|data| data.get_temp::<View>(id));
    let drag_id = id.with("drag");
    let mut drag = ui.data(|data| data.get_temp::<Drag>(drag_id));
    if controller
        .subtitle_edit
        .as_ref()
        .is_none_or(|edit| !edit.dragging_time)
        && controller.speech_edit.is_none()
    {
        drag = None;
    }
    let mut started = false;
    if editable
        && response.is_pointer_button_down_on()
        && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary))
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
        && let Some(view) = previous
        && rect.contains(origin)
    {
        let mut closest: Option<(f32, Drag)> = None;
        for index in view.visible_range(cues) {
            for (edge, time, inset) in [
                (SubtitleField::Start, cues[index].start_ms, 0.0),
                (SubtitleField::End, subtitles.effective_end_at(index), -2.0),
            ] {
                if !(view.start..=view.end).contains(&(time as f64)) {
                    continue;
                }
                let x = rect.left() + view.fraction(time as f64) as f32 * rect.width();
                let distance = (origin.x - x - inset).abs();
                if distance <= 6.0 && closest.as_ref().is_none_or(|(best, ..)| distance < *best) {
                    closest = Some((
                        distance,
                        Drag {
                            kind: DragKind::Subtitle {
                                cue: cues[index].id.clone(),
                                edge,
                            },
                            time: time as f64,
                            pointer_x: origin.x,
                            offset: origin.x - x,
                        },
                    ));
                }
            }
        }
        if let Some((_, grabbed)) = closest {
            drag = Some(grabbed);
            started = true;
        }
    }
    if editable
        && response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
        && speech_rect.contains(origin)
        && !clips.is_empty()
        && let Some(view) = previous
    {
        let time = view.time(((origin.x - rect.left()) / rect.width()) as f64);
        if let Some(clip) = clips
            .iter()
            .rev()
            .find(|clip| (clip.start_ms as f64..clip.end_ms() as f64).contains(&time))
        {
            drag = Some(Drag {
                kind: DragKind::Speech {
                    clip: clip.id.clone(),
                    grab: time - clip.start_ms as f64,
                },
                time,
                pointer_x: origin.x,
                offset: 0.0,
            });
            started = true;
        }
    }
    let mut action = None;
    let mut finished = false;
    if let Some(drag) = &mut drag {
        let cancel = !editable
            || !ui.input(|input| input.focused)
            || ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancel {
            action = Some(Action::CancelTrim);
            finished = true;
        } else {
            let bounds = match &drag.kind {
                DragKind::Subtitle { cue, edge } => {
                    cues.iter().position(|item| item.id == *cue).map(|index| {
                        let bounds = controller.subtitle_trim_bounds(index, *edge);
                        *bounds.start() as f64..=*bounds.end() as f64
                    })
                }
                DragKind::Speech { clip, grab } => {
                    clips.iter().find(|item| item.id == *clip).map(|clip| {
                        let bounds = controller.speech_move_bounds(clip);
                        *bounds.start() as f64 + grab..=*bounds.end() as f64 + grab
                    })
                }
            };
            let Some(bounds) = bounds else {
                return Some(Action::CancelTrim);
            };
            let before = drag.time.round() as i64;
            if let (Some(pointer), Some(view)) = (response.interact_pointer_pos(), previous) {
                let fraction = |x| ((x - drag.offset - rect.left()) / rect.width()) as f64;
                drag.time += view.time(fraction(pointer.x)) - view.time(fraction(drag.pointer_x));
                // Holding beyond an edge continues through the same magnified window.
                let outside =
                    (pointer.x - rect.right()).max(0.0) + (pointer.x - rect.left()).min(0.0);
                drag.time += (view.end - view.start)
                    * (outside / rect.width()) as f64
                    * ui.input(|input| input.stable_dt.min(0.05)) as f64
                    * 2.0;
                drag.pointer_x = pointer.x;
            }
            drag.time = drag
                .time
                .clamp(*bounds.start() as f64, *bounds.end() as f64);
            finished = response.drag_stopped_by(egui::PointerButton::Primary)
                || !ui.input(|input| input.pointer.primary_down());
            let time = drag.time.round() as i64;
            if started || time != before || finished {
                action = Some(match &drag.kind {
                    DragKind::Subtitle { cue, edge } => Action::Trim {
                        cue: cue.clone(),
                        edge: *edge,
                        time,
                        finished,
                    },
                    DragKind::Speech { clip, grab } => Action::MoveSpeech {
                        clip: clip.clone(),
                        start: (drag.time - grab).round() as i64,
                        position: time,
                        finished,
                    },
                });
            }
        }
    }
    let position = drag
        .as_ref()
        .map_or(position, |drag| drag.time.round() as i64);
    let settling_id = id.with("settling");
    let adjusting =
        drag.is_some() || ui.data(|data| data.get_temp::<bool>(settling_id).unwrap_or(false));
    let frozen = previous
        .filter(|_| !adjusting && (response.hovered() || response.is_pointer_button_down_on()));
    let mut view = frozen.unwrap_or_else(|| {
        let focus = drag.as_ref().map_or(position as f64, |drag| drag.time);
        let view = if controller.can_play() {
            View::following(cues, duration, focus, rect.width())
        } else {
            let mut view = View::full(duration);
            view.focus = focus;
            view
        };
        view.magnified(if drag.is_some() { 4.0 } else { 1.0 })
    });
    let mut settling = false;
    if controller.can_play() || adjusting {
        // Animate the final window as well as the focus: adding cues or crossing
        // repeated timestamps must not abruptly change the displayed time range.
        for (name, value) in [
            ("start", &mut view.start),
            ("end", &mut view.end),
            ("focus", &mut view.focus),
        ] {
            let target = *value as f32 / 1000.0;
            let animated = AnimationSystem::animate_value(
                ui.ctx(),
                id.with(name),
                target,
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
            settling |= (animated - target).abs() > 0.001;
        }
    }
    if let Some(drag) = &drag {
        view = view.anchored(
            drag.time,
            ((drag.pointer_x - drag.offset - rect.left()) / rect.width()) as f64,
            duration,
        );
        ui.ctx().request_repaint();
    }
    if finished {
        // Continue the same window animation from the last anchored geometry.
        for (name, value) in [
            ("start", view.start),
            ("end", view.end),
            ("focus", view.focus),
        ] {
            AnimationSystem::animate_value(ui.ctx(), id.with(name), value as f32 / 1000.0, 0.0);
        }
    }
    ui.data_mut(|data| data.insert_temp(settling_id, drag.is_some() || (adjusting && settling)));
    ui.data_mut(|data| data.insert_temp(id, view));
    ui.data_mut(|data| {
        if finished {
            data.remove::<Drag>(drag_id);
        } else if let Some(drag) = &drag {
            data.insert_temp(drag_id, drag.clone());
        } else {
            data.remove::<Drag>(drag_id);
        }
    });
    for (time, position, align) in [
        (view.start, labels.left_center(), egui::Align2::LEFT_CENTER),
        (view.end, labels.right_center(), egui::Align2::RIGHT_CENTER),
    ] {
        ui.painter().text(
            position,
            align,
            if drag.is_some() {
                super::format_cue_time(time.round() as i64)
            } else {
                super::format_time_ms(time.round() as i64)
            },
            egui::TextStyle::Body.resolve(ui.style()),
            theme::text_weak(),
        );
    }
    ui_director::egui::record_output(
        ui,
        "Subtitle window",
        ui_director::egui::ElementValue::Text(format!(
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
        .then(|| subtitles.active_cue_at(position))
        .flatten();
    let range = view.visible_range(cues);
    let mut edge_hovered = false;
    let mut index = range.start;
    while index < range.end {
        let cue = &cues[index];
        let dragged = drag.as_ref().and_then(|drag| match &drag.kind {
            DragKind::Subtitle { cue: id, edge } if *id == cue.id => {
                Some((*edge, drag.time.round() as i64))
            }
            _ => None,
        });
        let start_ms = dragged
            .filter(|(edge, _)| *edge == SubtitleField::Start)
            .map_or(cue.start_ms, |(_, time)| time);
        let end_ms = dragged
            .filter(|(edge, _)| *edge == SubtitleField::End)
            .map_or(subtitles.effective_end_at(index), |(_, time)| time);
        if end_ms as f64 <= view.start {
            index += 1;
            continue;
        }
        let start = x(start_ms);
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
        let active = dragged.is_some()
            || current.is_some_and(|current| {
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
        if editable {
            for (edge, time, handle_x) in [
                (SubtitleField::Start, start_ms, start + 2.0),
                (SubtitleField::End, end_ms, span.right() - 2.0),
            ] {
                if !(view.start..=view.end).contains(&(time as f64)) {
                    continue;
                }
                let edge_x = if edge == SubtitleField::Start {
                    start
                } else {
                    end - 2.0
                };
                let hovered = response
                    .hover_pos()
                    .is_some_and(|point| rect.contains(point) && (point.x - edge_x).abs() <= 6.0);
                edge_hovered |= hovered;
                if hovered || dragged.is_some_and(|(drag_edge, _)| drag_edge == edge) {
                    ui.painter().line_segment(
                        [
                            egui::pos2(handle_x, span.top() + 3.0),
                            egui::pos2(handle_x, span.bottom() - 3.0),
                        ],
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                    );
                }
            }
        }
        index = next;
    }
    let mut speech_hovered = None;
    if !clips.is_empty() {
        ui.painter()
            .rect_filled(speech_rect, 6, theme::surface_subtle());
        ui_director::egui::record_output(
            ui,
            "Speech timeline",
            ui_director::egui::ElementValue::None,
            speech_rect,
        );
        let moving = drag.as_ref().and_then(|drag| match &drag.kind {
            DragKind::Speech { clip, .. } => Some(clip),
            _ => None,
        });
        for clip in clips
            .iter()
            .filter(|clip| Some(&clip.id) != moving)
            .chain(clips.iter().filter(|clip| Some(&clip.id) == moving))
        {
            let moved = drag.as_ref().and_then(|drag| match &drag.kind {
                DragKind::Speech { clip: id, grab } if *id == clip.id => {
                    Some((drag.time - grab).round() as i64)
                }
                _ => None,
            });
            let start = moved.unwrap_or(clip.start_ms);
            let end = start + clip.duration_ms;
            if end as f64 <= view.start || start as f64 >= view.end {
                continue;
            }
            let span = egui::Rect::from_min_max(
                egui::pos2(x(start), speech_rect.top() + 2.0),
                egui::pos2(
                    (x(end) - 2.0).max(x(start) + 1.0),
                    speech_rect.bottom() - 2.0,
                ),
            );
            let hovered = response
                .hover_pos()
                .is_some_and(|point| span.contains(point));
            if hovered {
                speech_hovered = Some(clip);
            }
            let active = moved.is_some()
                || (task.is_some_and(|task| task.play_speech_track)
                    && (start..end).contains(&position));
            ui.painter().rect_filled(
                span,
                3,
                theme::primary().gamma_multiply(if active || hovered { 1.0 } else { 0.55 }),
            );
            let painter = ui.painter().with_clip_rect(span.shrink(2.0));
            if clip.peaks.is_empty() {
                painter.line_segment(
                    [span.left_center(), span.right_center()],
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                );
            } else if span.width() > 4.0 {
                for (index, peak) in clip.peaks.iter().enumerate() {
                    let at = start
                        + (clip.duration_ms as f64 * (index as f64 + 0.5) / clip.peaks.len() as f64)
                            as i64;
                    let height = 1.0 + f32::from(*peak) / 255.0 * 8.0;
                    painter.line_segment(
                        [
                            egui::pos2(x(at), span.center().y - height),
                            egui::pos2(x(at), span.center().y + height),
                        ],
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                    );
                }
            }
            if let Some(time) = ui_director::egui::record_slider(
                ui,
                ui.scope_id().with(("speech-clip", &clip.id)),
                &format!("Speech clip · {}", clip.id),
                start as f64,
                editable,
                span,
            ) {
                action = Some(Action::MoveSpeech {
                    clip: clip.id.clone(),
                    start: time.round() as i64,
                    position: time.round() as i64,
                    finished: true,
                });
            }
        }
    }
    if controller.can_play() && (view.start..=view.end).contains(&(position as f64)) {
        let position = x(position);
        ui.painter().line_segment(
            [
                egui::pos2(position, rect.top()),
                egui::pos2(position, full_rect.bottom()),
            ],
            egui::Stroke::new(2.0, theme::text_strong()),
        );
    }
    if drag.is_some() {
        let cursor = if matches!(drag.as_ref().unwrap().kind, DragKind::Speech { .. }) {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::ResizeHorizontal
        };
        response.on_hover_cursor(cursor);
        ui.ctx().set_cursor_icon(cursor);
        return action;
    }
    if action.is_some() {
        return action;
    }
    if let Some(clip) = speech_hovered {
        let clicked = response.clicked();
        let click_id = id.with("speech_click");
        let delete = editable
            && response.double_clicked()
            && ui.data(|data| data.get_temp::<String>(click_id)).as_deref() == Some(&clip.id);
        if clicked {
            ui.data_mut(|data| data.insert_temp(click_id, clip.id.clone()));
        }
        if delete {
            ui.data_mut(|data| data.remove::<String>(click_id));
            return Some(Action::DeleteSpeech(clip.id.clone()));
        }
        response
            .on_hover_cursor(if editable {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::PointingHand
            })
            .on_hover_ui(|ui| {
                ui.set_max_width(300.0);
                ui.weak(format!(
                    "{} – {}",
                    super::format_cue_time(clip.start_ms),
                    super::format_cue_time(clip.end_ms())
                ));
                ui.label(&clip.text);
            });
        return clicked.then(|| Action::AuditionSpeech(clip.id.clone()));
    }
    let pointer = response.hover_pos()?;
    if !rect.contains(pointer) || cues.is_empty() {
        return None;
    }
    let time = view
        .time(((pointer.x - rect.left()) / rect.width()) as f64)
        .round() as i64;
    let index = cues
        .partition_point(|cue| cue.start_ms <= time)
        .saturating_sub(1);
    let clicked = response.clicked();
    response
        .on_hover_cursor(if edge_hovered {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::PointingHand
        })
        .on_hover_ui(|ui| {
            ui.set_max_width(300.0);
            ui.weak(format!(
                "{} – {}",
                super::format_cue_time(cues[index].start_ms),
                super::format_cue_time(subtitles.effective_end_at(index)),
            ));
            ui.label(&cues[index].original_text);
        });
    clicked.then_some(Action::Select(index))
}
