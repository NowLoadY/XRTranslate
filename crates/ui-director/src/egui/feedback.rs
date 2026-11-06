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
        data.insert_temp(
            key(),
            State {
                items,
                until,
                reveal: true,
            },
        )
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
    let accent = accent.to_opaque();
    let mut painted = std::collections::BTreeSet::new();
    for item in &state.items {
        if let Some(rect) = item.rect
            && painted.insert(item.feedback.target)
        {
            let (color, width, lift) = match item.feedback.effect {
                Effect::Read => (accent, 1.6, 0.0),
                Effect::Pending => (accent, 1.7 + 0.25 * (age * 4.0).sin(), 0.0),
                Effect::Failed(_) => {
                    let recoil = (age * 18.0).sin().abs() * (-age * 5.0).exp();
                    (
                        ctx.global_style().visuals.error_fg_color.to_opaque(),
                        2.0,
                        recoil * 2.0,
                    )
                }
                _ => {
                    // A small response at the receiving edge, as the blue pulse arrives.
                    let hit =
                        (std::f32::consts::PI * ((age / 1.15).fract() - 0.8).max(0.0) * 5.0).sin();
                    (accent, 2.0 + hit * 0.5, hit)
                }
            };
            painter.rect_stroke(
                rect.expand(2.0 + lift * reveal),
                (rect.width().min(rect.height()) * 0.16).clamp(4.0, 20.0),
                Stroke::new(width * reveal, color),
                egui::StrokeKind::Outside,
            );
        }
    }
    let Some(item) = state.items.iter().find(|item| item.rect.is_some()) else {
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
        Stroke::new(1.7 * reveal, accent),
        5.0,
        5.0,
    ));
    pulses(
        painter,
        [start, end],
        &item.feedback.effect,
        age,
        reveal,
        accent,
    );
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
    let (period, lanes, stretch, width) = match effect {
        Effect::Failed(_) => return,
        Effect::Read => (1.85, 2, 12.0, 1.6),
        Effect::Pending => (2.4, 2, 8.0, 1.7),
        _ => (1.15, 4, 28.0, 2.4),
    };
    if reading {
        path.swap(0, 1);
    }
    let distance = path[0].distance(path[1]);
    let direction = (path[1] - path[0]) / distance;
    let normal = egui::vec2(-direction.y, direction.x);
    let phase = (age / period).fract();
    let speed = 16.0 * phase.powi(2) * (1.0 - phase).powi(2);
    let envelope = ease(phase / 0.12) * ease((1.0 - phase) / 0.12) * reveal;
    let center = ease(phase) * distance;
    // Stretch the current bars with speed; never draw translucent trailing copies.
    for (offset, size) in [(-6.0, 1.0), (6.0, 0.75), (-11.0, 0.55), (11.0, 0.4)]
        .into_iter()
        .take(lanes)
    {
        let length = (4.0 + stretch * speed) * size * envelope;
        let points = [
            path[0] + direction * (center - length * 0.5).max(0.0) + normal * offset,
            path[0] + direction * (center + length * 0.5).min(distance) + normal * offset,
        ];
        if reading {
            // A solid hairline keeps white readable on light surfaces.
            painter.line_segment(
                points,
                Stroke::new((width + 1.2) * envelope, Color32::from_rgb(166, 185, 214)),
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
