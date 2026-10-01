//! Find quiet space in the painted page without adding hooks to its controls.
use super::placement;
use eframe::egui::{self, LayerId, Pos2, Rect, Shape};

#[derive(Clone, Copy)]
pub(super) struct Spot {
    pub center: Pos2,
    pub radius: f32,
}

#[derive(Clone, Default)]
pub(super) struct Parking {
    spot: Option<Spot>,
    pending: Option<(Spot, f64)>,
    next_check: f64,
    bounds: Option<Rect>,
    occupied: Vec<Rect>,
}

impl Parking {
    pub fn occupied(&self) -> &[Rect] {
        &self.occupied
    }

    pub fn dropped(&mut self, center: Pos2, radius: f32) {
        self.spot = Some(Spot { center, radius });
        self.pending = None;
        self.next_check = 0.0;
    }

    pub fn locate(
        &mut self,
        ctx: &egui::Context,
        layer: LayerId,
        bounds: Rect,
        radius: f32,
        now: f64,
    ) -> Option<Spot> {
        let bounds = bounds.intersect(ctx.viewport_rect());
        if now < self.next_check && self.bounds == Some(bounds) {
            return self.spot;
        }
        self.next_check = now + 0.35;
        self.bounds = Some(bounds);
        self.occupied.clear();
        let mut heading = None;
        ctx.graphics(|graphics| {
            if let Some(paint) = graphics.get(layer) {
                for entry in paint.all_entries() {
                    collect(
                        &entry.shape,
                        entry.clip_rect.intersect(bounds),
                        bounds,
                        &mut self.occupied,
                        &mut heading,
                    );
                }
            }
        });
        // Keep a settled spot with a little less clearance than a new one needs.
        // Small layout changes should not send the companion back and forth.
        if self.spot.is_some_and(|spot| {
            let body = placement::footprint(spot.center, spot.radius).expand(2.0);
            bounds.contains_rect(body) && !self.occupied.iter().any(|rect| rect.intersects(body))
        }) {
            self.pending = None;
            return self.spot;
        }
        let heading = heading.unwrap_or(Rect::from_min_size(
            bounds.min + egui::vec2(24.0, 20.0),
            egui::vec2(120.0, 28.0),
        ));
        let preferred = self.spot.map_or(
            egui::pos2(heading.right() + radius * 1.3 + 18.0, heading.center().y),
            |spot| spot.center,
        );
        // Search the whole page at each size before shrinking. Obstacle edges
        // expose narrow gaps that a fixed sampling grid can miss entirely.
        for size in [radius, radius * 0.75, radius * 0.55, 12.0] {
            let size = size.max(12.0);
            if let Some(center) = find_space(bounds, &self.occupied, preferred, size) {
                let candidate = Spot {
                    center,
                    radius: size,
                };
                // Page transitions and late controls can expose temporary gaps.
                // Commit only after the same destination survives another scan.
                match self.pending {
                    Some((pending, since))
                        if pending.center.distance(center) < 6.0
                            && (pending.radius - size).abs() < 0.5 =>
                    {
                        if now - since >= 0.6 {
                            self.spot = Some(candidate);
                            self.pending = None;
                        }
                    }
                    _ => self.pending = Some((candidate, now)),
                }
                return self.spot;
            }
            if size == 12.0 {
                break;
            }
        }
        self.spot = None;
        self.pending = None;
        None
    }
}

fn find_space(bounds: Rect, occupied: &[Rect], preferred: Pos2, radius: f32) -> Option<Pos2> {
    let body = placement::footprint(Pos2::ZERO, radius).expand(6.0);
    let allowed = Rect::from_min_max(
        bounds.min - body.min.to_vec2(),
        bounds.max - body.max.to_vec2(),
    );
    if !allowed.is_positive() {
        return None;
    }
    let preferred = allowed.clamp(preferred);
    // Expand obstacles into forbidden center positions, then sweep their edges.
    let mut blocked: Vec<_> = occupied
        .iter()
        .map(|rect| {
            Rect::from_min_max(rect.min - body.max.to_vec2(), rect.max - body.min.to_vec2())
        })
        .filter(|rect| rect.intersects(allowed))
        .collect();
    blocked.sort_unstable_by(|a, b| a.left().total_cmp(&b.left()));
    let mut rows = vec![preferred.y, allowed.top(), allowed.bottom()];
    for rect in &blocked {
        rows.extend([rect.top() - 0.5, rect.bottom() + 0.5]);
    }
    rows.retain(|y| *y >= allowed.top() && *y <= allowed.bottom());
    rows.sort_unstable_by(f32::total_cmp);
    rows.dedup();
    let mut best: Option<Pos2> = None;
    for y in rows {
        let mut left = allowed.left();
        let mut consider = |left: f32, right: f32| {
            if right >= left {
                let point = egui::pos2(preferred.x.clamp(left, right), y);
                if best
                    .is_none_or(|best| point.distance_sq(preferred) < best.distance_sq(preferred))
                {
                    best = Some(point);
                }
            }
        };
        // Each horizontal interval is a complete free span, not just one sample.
        for rect in blocked
            .iter()
            .filter(|rect| y >= rect.top() && y <= rect.bottom())
        {
            consider(left, (rect.left() - 0.5).min(allowed.right()));
            left = left.max(rect.right() + 0.5);
            if left > allowed.right() {
                break;
            }
        }
        consider(left, allowed.right());
    }
    best
}

fn collect(
    shape: &Shape,
    clip: Rect,
    page: Rect,
    occupied: &mut Vec<Rect>,
    heading: &mut Option<Rect>,
) {
    if let Shape::Vec(shapes) = shape {
        for shape in shapes {
            collect(shape, clip, page, occupied, heading);
        }
        return;
    }
    // Page/section backdrops are surfaces we can rest on, not foreground content.
    if matches!(shape, Shape::Rect(rect) if rect.rect.area() > page.area() * 0.15) {
        return;
    }
    // Empty text can have infinite visual bounds. Clipping it first would turn
    // an invisible label into an obstacle covering the entire page.
    let rect = shape.visual_bounding_rect();
    if !rect.is_positive() || !rect.is_finite() {
        return;
    }
    let rect = rect.intersect(clip);
    if !rect.is_positive() {
        return;
    }
    if let Shape::Text(text) = shape
        && rect.top() < page.top() + 100.0
        && text
            .galley
            .job
            .sections
            .iter()
            .any(|section| section.format.font_id.size >= 18.0)
        && heading.is_none_or(|current| rect.top() < current.top())
    {
        *heading = Some(rect);
    }
    occupied.push(rect);
}
