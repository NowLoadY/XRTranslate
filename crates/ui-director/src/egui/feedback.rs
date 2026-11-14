//! Map execution facts to live geometry. Painting is optional and has no layout side effects.
use crate::feedback::{Effect, Feedback};
use ::egui::{self, Color32, Id, Pos2, Rect, Stroke};

const LIFETIME: f64 = 4.0;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LocatedFeedback {
    pub feedback: Feedback,
    pub rect: Option<Rect>,
}

#[derive(Clone)]
struct State {
    items: Vec<LocatedFeedback>,
    until: f64,
    reveal: bool,
}

fn key() -> Id {
    Id::unique("director_feedback")
}

/// Show one bounded batch of actual operations; no additional tool/model messages.
pub fn show(items: impl IntoIterator<Item = Feedback>) {
    if let Some(ctx) = super::driver().context() {
        show_in(ctx, items);
    }
}

pub fn show_in(ctx: &egui::Context, items: impl IntoIterator<Item = Feedback>) {
    let items: Vec<_> = items
        .into_iter()
        .take(16)
        .map(|feedback| LocatedFeedback {
            feedback,
            rect: None,
        })
        .collect();
    if items.is_empty() {
        return;
    }
    let until = ctx.input(|input| input.time) + LIFETIME;
    ctx.data_mut(|data| {
        // A verification read often follows a write immediately. Give the
        // actual result its full display time instead of replacing it.
        if items.iter().all(|item| {
            matches!(
                item.feedback.effect,
                Effect::Read | Effect::Pending | Effect::Invoke
            )
        }) && data.get_temp::<State>(key()).is_some_and(|state| {
            state.until > until - LIFETIME
                && state
                    .items
                    .iter()
                    .any(|item| matches!(item.feedback.effect, Effect::Change { .. }))
        }) {
            return;
        }
        data.insert_temp(
            key(),
            State {
                items,
                until,
                reveal: true,
            },
        );
    });
    ctx.request_repaint();
}

/// Renderer-facing facts and current clipped rectangles. Hidden rows have no rectangle.
pub fn current(ctx: &egui::Context) -> Vec<LocatedFeedback> {
    ctx.data(|data| data.get_temp::<State>(key()))
        .filter(|state| state.until > ctx.input(|input| input.time))
        .map_or_else(Vec::new, |state| state.items)
}

pub fn expires(ctx: &egui::Context) -> f64 {
    ctx.data(|data| data.get_temp::<State>(key()))
        .map_or(0.0, |state| state.until)
}

/// Preserve timing and geometry supplied by an embedded surface.
pub fn import(ctx: &egui::Context, items: Vec<LocatedFeedback>, until: f64) {
    if !items.is_empty() && until > ctx.input(|input| input.time) {
        ctx.data_mut(|data| {
            data.insert_temp(
                key(),
                State {
                    items,
                    until,
                    reveal: false,
                },
            )
        });
    }
}

pub fn begin_frame(ctx: &egui::Context) {
    let now = ctx.input(|input| input.time);
    ctx.data_mut(|data| {
        if let Some(mut state) = data.get_temp::<State>(key()) {
            if now >= state.until {
                data.remove::<State>(key());
            } else {
                // Never draw at stale coordinates after navigation or scrolling.
                for item in &mut state.items {
                    item.rect = None;
                }
                data.insert_temp(key(), state);
            }
        }
    });
}

pub fn reveal_target(ctx: &egui::Context) -> Option<crate::feedback::Target> {
    ctx.data(|data| data.get_temp::<State>(key()))
        .filter(|state| state.reveal)
        .and_then(|state| state.items.first().map(|item| item.feedback.target))
}

/// Keep feedback visible without moving a view the user is watching or browsing.
pub fn dismiss_reveal(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        if let Some(mut state) = data.get_temp::<State>(key()) {
            state.reveal = false;
            data.insert_temp(key(), state);
        }
    });
}

/// Bind the same stable identity used by controls or documents to this frame's geometry.
pub fn record(ui: &egui::Ui, target: Id, rect: Rect) {
    let scroll = ui.ctx().data_mut(|data| {
        let Some(mut state) = data.get_temp::<State>(key()) else {
            return false;
        };
        let mut scroll = false;
        for (index, item) in state.items.iter_mut().enumerate() {
            if item.feedback.target != target.value() {
                continue;
            }
            let visible = rect.intersect(ui.clip_rect());
            item.rect = visible.is_positive().then_some(visible);
            if index == 0 {
                scroll = state.reveal && !ui.clip_rect().contains(rect.center());
                if !scroll {
                    state.reveal = false;
                }
            }
        }
        data.insert_temp(key(), state);
        scroll
    });
    if scroll {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
    }
}

/// Wordless overlay. Hosts supply an opaque accent and optional companion anchor.
/// Reads flow toward the anchor; writes flow toward the target. Facts remain in `current`.
pub fn paint(painter: &egui::Painter, anchor: Option<(Pos2, f32)>, accent: Color32) {
    let ctx = painter.ctx();
    let Some(state) = ctx.data(|data| data.get_temp::<State>(key())) else {
        return;
    };
    let remaining = state.until - ctx.input(|input| input.time);
    if remaining <= 0.0 {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(16));
    let age = (LIFETIME - remaining).max(0.0) as f32;
    // Enter and leave by changing geometry, never by washing out the colors.
    let reveal = ease(age / 0.16) * ease(remaining as f32 / 0.24);
    paint_items(painter, &state.items, anchor, accent, age, reveal, true);
}

/// Host-authored guidance uses the same wordless renderer and live controls,
/// without inventing execution facts or replacing Agent/CLI feedback.
pub fn paint_guidance(
    painter: &egui::Painter,
    controls: &[super::registry::ElementDescriptor],
    anchor: Option<(Pos2, f32)>,
    accent: Color32,
    age: f32,
    animated: bool,
) {
    let items: Vec<_> = controls
        .iter()
        .filter_map(|control| {
            let rect = control.visible_rect()?.intersect(painter.clip_rect());
            rect.is_positive().then(|| LocatedFeedback {
                feedback: control.feedback(Effect::Read),
                rect: Some(rect),
            })
        })
        .take(4)
        .collect();
    if !items.is_empty() {
        if animated {
            painter
                .ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        paint_items(
            painter,
            &items,
            anchor,
            accent,
            age,
            ease(age / 0.16),
            animated,
        );
    }
}

fn paint_items(
    painter: &egui::Painter,
    items: &[LocatedFeedback],
    anchor: Option<(Pos2, f32)>,
    accent: Color32,
    age: f32,
    reveal: f32,
    animated: bool,
) {
    let ctx = painter.ctx();
    let accent = accent.to_opaque();
    let mut painted = std::collections::BTreeSet::new();
    for item in items {
        if let Some(rect) = item.rect
            && painted.insert(item.feedback.target)
        {
            let (color, width, lift) = match item.feedback.effect {
                Effect::Read => (accent, 1.2, 0.0),
                Effect::Pending => (accent, 1.3 + 0.12 * (age * 4.0).sin(), 0.0),
                Effect::Failed(_) => {
                    let recoil = (age * 18.0).sin().abs() * (-age * 5.0).exp();
                    (
                        ctx.global_style().visuals.error_fg_color.to_opaque(),
                        1.6,
                        recoil * 1.5,
                    )
                }
                _ => (accent, 1.5, 0.0),
            };
            painter.rect_stroke(
                rect.expand(2.0 + lift * reveal),
                (rect.width().min(rect.height()) * 0.16).clamp(4.0, 20.0),
                Stroke::new(width * reveal, color),
                egui::StrokeKind::Outside,
            );
        }
    }
    let Some(item) = items.iter().find(|item| item.rect.is_some()) else {
        return;
    };
    let Some((head, radius)) = anchor else {
        return;
    };
    let end = item.rect.unwrap().clamp(head);
    let direction = end - head;
    if direction.length() <= radius + 12.0 {
        return;
    }
    let start = head + direction.normalized() * radius;
    painter.extend(egui::Shape::dashed_line(
        &[start, end],
        Stroke::new(reveal, accent),
        3.5,
        6.0,
    ));
    if animated {
        pulses(
            painter,
            [start, end],
            &item.feedback.effect,
            age,
            reveal,
            accent,
        );
    }
}

// Quintic easing has zero velocity and acceleration at both ends of each pass.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (10.0 + t * (-15.0 + 6.0 * t))
}

fn pulses(
    painter: &egui::Painter,
    mut path: [Pos2; 2],
    effect: &Effect,
    age: f32,
    reveal: f32,
    accent: Color32,
) {
    let reading = matches!(effect, Effect::Read);
    let (period, length, width): (f32, f32, f32) = match effect {
        Effect::Failed(_) => return,
        Effect::Read => (1.85, 7.0, 1.3),
        Effect::Pending => (2.4, 6.0, 1.3),
        _ => (1.45, 10.0, 1.7),
    };
    if reading {
        path.swap(0, 1);
    }
    let distance = path[0].distance(path[1]);
    let direction = (path[1] - path[0]) / distance;
    let count = (distance / 160.0).ceil().clamp(1.0, 3.0) as usize;
    // Even phases and constant speed keep pulses separated along one quiet path.
    // Only their size eases at the endpoints, so they never bunch up there.
    for index in 0..count {
        let phase = (age / period + index as f32 / count as f32).fract();
        let envelope = ease(phase / 0.12) * ease((1.0 - phase) / 0.12) * reveal;
        let center = phase * distance;
        let length = length.min(distance * 0.2) * envelope;
        let points = [
            path[0] + direction * (center - length * 0.5).max(0.0),
            path[0] + direction * (center + length * 0.5).min(distance),
        ];
        if reading {
            // A solid hairline keeps white readable on light surfaces.
            painter.line_segment(
                points,
                Stroke::new((width + 1.0) * envelope, Color32::from_rgb(166, 185, 214)),
            );
        }
        painter.line_segment(
            points,
            Stroke::new(
                width * envelope,
                if reading { Color32::WHITE } else { accent },
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tutorial_guidance_preserves_execution_feedback_and_clips_targets() {
        let ctx = egui::Context::default();
        let mut control: super::super::registry::ElementDescriptor =
            serde_json::from_value(serde_json::json!({
                "index": 0, "id_hex": "0000000000000007", "label": "Preview", "kind": "button",
                "value": null, "enabled": true, "rect": [20.0, 30.0, 100.0, 80.0],
                "clip_rect": [0.0, 0.0, 90.0, 60.0]
            }))
            .unwrap();
        assert_eq!(
            control.visible_rect(),
            Some(Rect::from_min_max(
                egui::pos2(20.0, 30.0),
                egui::pos2(90.0, 60.0)
            ))
        );
        let change = Feedback::changed(8, "Document", "before", "after").unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_in(&ctx, [change.clone()]);
            paint_guidance(
                ui.painter(),
                &[control.clone()],
                None,
                Color32::BLUE,
                1.0,
                false,
            );
            assert_eq!(current(&ctx)[0].feedback, change);
        });
        output.textures_delta.clear();
        control.rect = [200.0, 200.0, 100.0, 80.0];
        assert!(
            control.visible_rect().is_none(),
            "a scrolled-out target has no highlight"
        );
        control.clip_rect = None;
        assert!(
            control.visible_rect().is_some(),
            "older plugin snapshots remain readable"
        );
    }

    #[test]
    fn verification_does_not_hide_a_write_and_navigation_can_stay_put() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(1.0),
                ..Default::default()
            },
            |_| {},
        );
        output.textures_delta.clear();
        let change = Feedback::changed(7, "00:01", "old", "new").unwrap();
        show_in(&ctx, [change.clone()]);
        let until = expires(&ctx);
        for effect in [Effect::Read, Effect::Pending, Effect::Invoke] {
            show_in(&ctx, [Feedback::new(8, "", effect)]);
            assert_eq!(current(&ctx)[0].feedback, change);
            assert_eq!(expires(&ctx), until);
        }
        dismiss_reveal(&ctx);
        assert_eq!(reveal_target(&ctx), None);
        assert_eq!(current(&ctx)[0].feedback, change);
        let next = Feedback::changed(9, "00:02", "first", "second").unwrap();
        show_in(&ctx, [next.clone()]);
        assert_eq!(current(&ctx)[0].feedback, next);
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(6.0),
                ..Default::default()
            },
            |_| {},
        );
        output.textures_delta.clear();
        let read = Feedback::new(8, "", Effect::Read);
        show_in(&ctx, [read.clone()]);
        assert_eq!(current(&ctx)[0].feedback, read);
    }
}
