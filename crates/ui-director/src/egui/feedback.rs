//! Map execution facts to live geometry. Painting is optional and has no layout side effects.
use crate::feedback::{Effect, Feedback};
use ::egui::{self, Id, Pos2, Rect};

#[derive(Clone, Debug)]
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
    let until = ctx.input(|input| input.time) + 4.0;
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

/// Default overlay. Hosts supply their accent, localization and optional companion anchor.
pub fn paint(
    painter: &egui::Painter,
    anchor: Option<(Pos2, f32)>,
    accent: egui::Color32,
    translate: impl Fn(&str) -> String,
) {
    let ctx = painter.ctx();
    let Some(state) = ctx.data(|data| data.get_temp::<State>(key())) else {
        return;
    };
    let remaining = state.until - ctx.input(|input| input.time);
    if remaining <= 0.0 {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
    let color = accent.gamma_multiply((remaining as f32 / 0.5).min(1.0) * 0.7);
    let first = state.items.iter().find(|item| item.rect.is_some());
    let mut painted = std::collections::BTreeSet::new();
    for item in &state.items {
        if let Some(rect) = item.rect
            && painted.insert(item.feedback.target)
        {
            painter.rect_stroke(
                rect.expand(2.0),
                5.0,
                egui::Stroke::new(1.5, color),
                egui::StrokeKind::Outside,
            );
        }
    }
    let Some(item) = first else {
        return;
    };
    let rect = item.rect.unwrap();
    if let Some((head, radius)) = anchor {
        let end = rect.clamp(head);
        let direction = end - head;
        if direction.length() > radius {
            painter.extend(egui::Shape::dashed_line(
                &[head + direction.normalized() * radius, end],
                egui::Stroke::new(1.5, color),
                5.0,
                4.0,
            ));
        }
    }
    let title = match &item.feedback.effect {
        Effect::Read => "Read",
        Effect::Invoke => "Activated",
        Effect::Pending => "Pending",
        Effect::Failed(_) => "Failed",
        Effect::Change { before, .. } if before.is_empty() => "Added",
        Effect::Change { after, .. } if after.is_empty() => "Deleted",
        Effect::Change { .. } => "Changed",
    };
    let mut text = translate(title);
    if !item.feedback.label.is_empty() {
        text.push_str(" · ");
        text.extend(translate(&item.feedback.label).chars().take(60));
    }
    if state.items.len() > 1 {
        text.push_str(&format!(" (+{})", state.items.len() - 1));
    }
    match &item.feedback.effect {
        Effect::Change { before, after } => {
            let prefix = before
                .chars()
                .zip(after.chars())
                .take_while(|(a, b)| a == b)
                .count();
            for (sign, value) in [("−", before), ("+", after)] {
                if value.is_empty() {
                    continue;
                }
                let start = prefix.saturating_sub(16);
                let excerpt: String = value.chars().skip(start).take(100).collect();
                text.push_str(&format!(
                    "\n{sign} {}{}{}",
                    if start > 0 { "…" } else { "" },
                    excerpt.replace('\n', " "),
                    if value.chars().count() > start + 100 {
                        "…"
                    } else {
                        ""
                    }
                ));
            }
        }
        Effect::Failed(error) => {
            text.push('\n');
            text.extend(error.chars().take(120));
        }
        _ => {}
    }
    let visuals = &ctx.global_style().visuals;
    let galley = painter.layout(
        text,
        egui::FontId::proportional(12.0),
        visuals.text_color(),
        340.0,
    );
    let size = galley.size() + egui::vec2(16.0, 10.0);
    let screen = ctx.content_rect().shrink(6.0);
    let y = if rect.top() - size.y - 6.0 >= screen.top() {
        rect.top() - size.y - 6.0
    } else {
        rect.bottom() + 6.0
    };
    let pos = egui::pos2(
        rect.left()
            .clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
        y.clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
    );
    let mut overlay = painter.clone();
    overlay.multiply_opacity((remaining as f32 / 0.5).min(1.0));
    overlay.rect_filled(Rect::from_min_size(pos, size), 5.0, visuals.panel_fill);
    overlay.rect_stroke(
        Rect::from_min_size(pos, size),
        5.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Outside,
    );
    overlay.galley(pos + egui::vec2(8.0, 5.0), galley, visuals.text_color());
}
