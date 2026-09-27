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
    next_check: f64,
    bounds: Option<Rect>,
}

impl Parking {
    pub fn dropped(&mut self, center: Pos2, radius: f32) {
        self.spot = Some(Spot { center, radius });
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
        if now < self.next_check && self.bounds == Some(bounds) {
            return self.spot;
        }
        self.next_check = now + 0.35;
        self.bounds = Some(bounds);
        let mut occupied = Vec::new();
        let mut heading = None;
        ctx.graphics(|graphics| {
            if let Some(paint) = graphics.get(layer) {
                for entry in paint.all_entries() {
                    collect(
                        &entry.shape,
                        entry.clip_rect.intersect(bounds),
                        bounds,
                        &mut occupied,
                        &mut heading,
                    );
                }
            }
        });
        let clear = |spot: Spot| {
            let body = placement::footprint(spot.center, spot.radius).expand(5.0);
            bounds.contains_rect(body) && !occupied.iter().any(|rect| rect.intersects(body))
        };
        if self.spot.is_some_and(clear) {
            return self.spot;
        }
        let heading = heading.unwrap_or(Rect::from_min_size(
            bounds.min + egui::vec2(24.0, 20.0),
            egui::vec2(120.0, 28.0),
        ));
        // Prefer the title's right-hand whitespace, shrinking only as needed.
        for size in [radius, radius * 0.75, radius * 0.55, 12.0] {
            let mut x = heading.right() + size * 1.3 + 18.0;
            while x + size * 1.3 + 6.0 < bounds.right() {
                let center = placement::constrain(
                    egui::pos2(x, heading.center().y + size * 0.3),
                    size,
                    bounds,
                );
                let spot = Spot {
                    center,
                    radius: size,
                };
                if clear(spot)
                    && placement::footprint(center, size).bottom() <= heading.bottom() + 24.0
                {
                    self.spot = Some(spot);
                    return self.spot;
                }
                x += 12.0;
            }
        }
        // Dense toolbars may use the entire header. Search other clear space,
        // keeping the small silhouette away from text, images and controls.
        let size = (radius * 0.75).max(12.0);
        let mut y = bounds.top() + size * 1.7 + 8.0;
        while y + size * 1.1 + 6.0 < bounds.bottom() {
            let mut x = bounds.right() - size * 1.3 - 8.0;
            while x - size * 1.3 - 6.0 > bounds.left() {
                let spot = Spot {
                    center: egui::pos2(x, y),
                    radius: size,
                };
                if clear(spot) {
                    self.spot = Some(spot);
                    return self.spot;
                }
                x -= size * 2.8;
            }
            y += size * 2.8;
        }
        self.spot = None;
        None
    }
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
    let rect = shape.visual_bounding_rect().intersect(clip);
    if !rect.is_positive() || !rect.is_finite() {
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
