//! Find quiet space in the painted page without adding hooks to its controls.
use super::placement;
use eframe::egui::{self, Pos2, Rect, TextureId};
use xrtranslate_plugin::surface::ContentBounds;

const SCAN_INTERVAL: f64 = 0.5;
const DESTINATION_DWELL: f64 = 1.2;
const NO_SPACE_GRACE: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spot {
    pub center: Pos2,
    pub radius: f32,
}

#[derive(Clone, Default)]
pub(super) struct Parking {
    spot: Option<Spot>,
    pending: Option<(Spot, f64)>,
    unavailable_since: Option<f64>,
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
        self.unavailable_since = None;
        self.next_check = 0.0;
    }

    pub fn locate(
        &mut self,
        ctx: &egui::Context,
        bounds: Rect,
        radius: f32,
        now: f64,
        background: Option<TextureId>,
        content: Option<&ContentBounds>,
    ) -> Option<Spot> {
        let bounds = bounds.intersect(ctx.viewport_rect());
        if now < self.next_check && self.bounds == Some(bounds) {
            return self.spot;
        }
        self.next_check = now + SCAN_INTERVAL;
        self.bounds = Some(bounds);
        self.occupied.clear();
        if let Some(content) = content {
            // Shared GPU surfaces are page containers, not full-page obstacles.
            self.occupied
                .extend(content.occupied.iter().filter_map(|rect| {
                    let rect = rect.intersect(bounds);
                    (rect.is_finite() && rect.is_positive()).then_some(rect)
                }));
            return self.settle(bounds, radius, content.heading, now);
        }
        let mut content = ContentBounds::default();
        let layers = ctx.memory(|memory| {
            memory
                .layer_ids()
                // Include page content and floating toolbars, excluding the
                // foreground companion itself, popups and tooltips.
                .filter(|candidate| {
                    matches!(
                        candidate.order,
                        egui::Order::Background | egui::Order::Middle
                    )
                })
                .collect::<Vec<_>>()
        });
        let layers = layers
            .into_iter()
            .map(|layer| {
                (
                    layer,
                    ctx.layer_transform_to_global(layer).unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        ctx.graphics(|graphics| {
            for (layer, transform) in layers {
                if let Some(paint) = graphics.get(layer) {
                    for entry in paint.all_entries() {
                        content.collect(
                            &entry.shape,
                            transform.mul_rect(entry.clip_rect).intersect(bounds),
                            bounds,
                            transform,
                            background,
                        );
                    }
                }
            }
        });
        self.occupied = content.occupied;
        self.settle(bounds, radius, content.heading, now)
    }

    fn settle(
        &mut self,
        bounds: Rect,
        radius: f32,
        heading: Option<Rect>,
        now: f64,
    ) -> Option<Spot> {
        // Keep a settled spot with a little less clearance than a new one needs.
        // Small layout changes should not send the companion back and forth.
        if self
            .spot
            .is_some_and(|spot| is_clear(spot, bounds, &self.occupied, 2.0))
        {
            self.pending = None;
            self.unavailable_since = None;
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
        // Once a destination is clear, keep its exact position while waiting.
        // Re-running the nearest-gap search each scan would follow moving text
        // and card edges, even when the original destination is still usable.
        let candidate = self
            .pending
            .map(|(spot, _)| spot)
            .filter(|spot| {
                spot.radius <= radius.max(12.0) && is_clear(*spot, bounds, &self.occupied, 6.0)
            })
            .or_else(|| {
                // Search the whole page at each size before shrinking. Obstacle
                // edges expose gaps that a fixed sampling grid can miss.
                for size in [radius, radius * 0.75, radius * 0.55, 12.0] {
                    let size = size.max(12.0);
                    if let Some(center) = find_space(bounds, &self.occupied, preferred, size) {
                        return Some(Spot {
                            center,
                            radius: size,
                        });
                    }
                    if size == 12.0 {
                        break;
                    }
                }
                None
            });
        if let Some(candidate) = candidate {
            self.unavailable_since = None;
            // Loading and scrolling can expose temporary gaps. A destination
            // must stay usable for a full dwell before the avatar moves there.
            match self.pending {
                Some((pending, since)) if pending == candidate => {
                    if now - since >= DESTINATION_DWELL {
                        self.spot = Some(candidate);
                        self.pending = None;
                    }
                }
                _ => self.pending = Some((candidate, now)),
            }
        } else {
            self.pending = None;
            // A single crowded frame should not start a disappear/reappear
            // cycle; sustained lack of space still releases the parked spot.
            let since = *self.unavailable_since.get_or_insert(now);
            if now - since >= NO_SPACE_GRACE {
                self.spot = None;
            }
        }
        self.spot
    }
}

fn is_clear(spot: Spot, bounds: Rect, occupied: &[Rect], clearance: f32) -> bool {
    let body = placement::footprint(spot.center, spot.radius).expand(clearance);
    bounds.contains_rect(body) && !occupied.iter().any(|rect| rect.intersects(body))
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
