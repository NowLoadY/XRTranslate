//! Find quiet space in the painted page without adding hooks to its controls.
use super::placement;
use eframe::egui::{self, LayerId, Pos2, Rect, Shape};

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
        layer: LayerId,
        bounds: Rect,
        radius: f32,
        now: f64,
    ) -> Option<Spot> {
        let bounds = bounds.intersect(ctx.viewport_rect());
        if now < self.next_check && self.bounds == Some(bounds) {
            return self.spot;
        }
        self.next_check = now + SCAN_INTERVAL;
        self.bounds = Some(bounds);
        self.occupied.clear();
        let mut heading = None;
        let layers = ctx.memory(|memory| {
            memory
                .layer_ids()
                .filter(|candidate| candidate.order == layer.order)
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
                        collect(
                            &entry.shape,
                            transform.mul_rect(entry.clip_rect).intersect(bounds),
                            bounds,
                            transform,
                            &mut self.occupied,
                            &mut heading,
                        );
                    }
                }
            }
        });
        self.settle(bounds, radius, heading, now)
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

fn collect(
    shape: &Shape,
    clip: Rect,
    page: Rect,
    transform: egui::emath::TSTransform,
    occupied: &mut Vec<Rect>,
    heading: &mut Option<Rect>,
) {
    if let Shape::Vec(shapes) = shape {
        for shape in shapes {
            collect(shape, clip, page, transform, occupied, heading);
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
    let rect = transform.mul_rect(rect).intersect(clip);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))
    }

    #[test]
    fn pending_destination_stays_fixed_when_preferred_position_changes() {
        let mut parking = Parking::default();
        assert!(parking.settle(bounds(), 20.0, None, 0.0).is_none());
        let pending = parking.pending.unwrap().0;
        let heading = Rect::from_min_size(egui::pos2(300.0, 70.0), egui::vec2(100.0, 30.0));

        assert!(parking.settle(bounds(), 20.0, Some(heading), 0.7).is_none());
        assert_eq!(parking.pending.unwrap().0, pending);
        assert_eq!(
            parking.settle(bounds(), 20.0, Some(heading), 1.3),
            Some(pending)
        );
    }

    #[test]
    fn blocked_destination_must_complete_a_new_dwell() {
        let mut parking = Parking::default();
        parking.settle(bounds(), 20.0, None, 0.0);
        let first = parking.pending.unwrap().0;
        parking
            .occupied
            .push(placement::footprint(first.center, first.radius));

        assert!(parking.settle(bounds(), 20.0, None, 0.7).is_none());
        let replacement = parking.pending.unwrap().0;
        assert_ne!(replacement, first);
        assert!(parking.settle(bounds(), 20.0, None, 1.3).is_none());
        assert_eq!(parking.settle(bounds(), 20.0, None, 2.0), Some(replacement));
        assert!(is_clear(replacement, bounds(), &parking.occupied, 6.0));
    }

    #[test]
    fn transient_crowding_does_not_hide_a_settled_companion() {
        let mut parking = Parking::default();
        parking.dropped(egui::pos2(200.0, 150.0), 20.0);
        let settled = parking.spot;
        parking.occupied.push(bounds());

        assert_eq!(parking.settle(bounds(), 20.0, None, 0.0), settled);
        assert_eq!(parking.settle(bounds(), 20.0, None, 0.6), settled);
        parking.occupied.clear();
        assert_eq!(parking.settle(bounds(), 20.0, None, 0.8), settled);
        assert!(parking.unavailable_since.is_none());

        parking.occupied.push(bounds());
        assert_eq!(parking.settle(bounds(), 20.0, None, 1.0), settled);
        assert_eq!(parking.settle(bounds(), 20.0, None, 1.6), settled);
        assert!(parking.settle(bounds(), 20.0, None, 2.1).is_none());
        parking.occupied.clear();
        assert!(parking.settle(bounds(), 20.0, None, 2.2).is_none());
        assert!(parking.settle(bounds(), 20.0, None, 3.5).is_some());
    }

    #[test]
    fn clear_spot_keeps_its_position_and_size_as_more_space_opens() {
        let mut parking = Parking::default();
        parking.dropped(egui::pos2(200.0, 150.0), 12.0);
        let settled = parking.spot;
        parking.occupied.push(Rect::from_min_size(
            egui::pos2(300.0, 150.0),
            egui::vec2(40.0, 30.0),
        ));

        assert_eq!(parking.settle(bounds(), 24.0, None, 0.0), settled);
        parking.occupied.clear();
        assert_eq!(parking.settle(bounds(), 24.0, None, 5.0), settled);
    }
}
