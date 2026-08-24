//! Reusable, domain-neutral graph canvas geometry and painting.
//!
//! Graph consumers retain ownership of their node/link models and mutations. This module owns
//! only the viewport behavior shared by graph editors: transforms, fitting, navigation, and the
//! primitive visuals used for grids, wires, and selection boxes.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Pos2, Rect, Stroke, UiBuilder, Vec2,
};

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 1.6;

#[derive(Clone, Debug)]
pub(crate) struct GraphCanvasState {
    pub pan: Vec2,
    pub zoom: f32,
    pub fit_pending: bool,
    pub canvas_size: Vec2,
    wire_base_zoom: Option<f32>,
}

impl Default for GraphCanvasState {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            fit_pending: true,
            canvas_size: Vec2::new(960.0, 540.0),
            wire_base_zoom: None,
        }
    }
}

impl GraphCanvasState {
    pub fn graph_rect(&self, canvas: Rect, position: [f32; 2], size: Vec2) -> Rect {
        Rect::from_min_size(
            Pos2::new(
                canvas.left() + self.pan.x + position[0] * self.zoom,
                canvas.top() + self.pan.y + position[1] * self.zoom,
            ),
            size * self.zoom,
        )
    }

    pub fn graph_position(&self, canvas: Rect, pointer: Pos2) -> [f32; 2] {
        let position = (pointer - canvas.min - self.pan) / self.zoom;
        [position.x, position.y]
    }

    pub fn fit_to_bounds(&mut self, bounds: Rect, available: Vec2, minimum_size: Vec2) {
        let graph_size = bounds.size().max(minimum_size);
        let viewport = (available - Vec2::splat(48.0)).max(Vec2::splat(1.0));
        self.zoom = (viewport.x / graph_size.x)
            .min(viewport.y / graph_size.y)
            .clamp(MIN_ZOOM, 1.0);
        self.pan = Vec2::new(
            (available.x - graph_size.x * self.zoom) * 0.5 - bounds.min.x * self.zoom,
            (available.y - graph_size.y * self.zoom) * 0.5 - bounds.min.y * self.zoom,
        );
    }

    pub fn zoom_at_pointer(&mut self, canvas: Rect, pointer: Pos2, scroll: f32) {
        let old_zoom = self.zoom;
        let factor = (scroll * 0.0015).exp();
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - old_zoom).abs() <= f32::EPSILON {
            return;
        }
        let pointer_in_canvas = pointer - canvas.min;
        let graph_position = (pointer_in_canvas - self.pan) / old_zoom;
        self.zoom = new_zoom;
        self.pan = pointer_in_canvas - graph_position * new_zoom;
    }

    /// Handles edge auto-pan and temporary zoom-out while a connection is being dragged.
    pub fn update_wire_dragging_navigation(
        &mut self,
        canvas: Rect,
        ui: &egui::Ui,
        is_pulling_wire: bool,
    ) {
        let dt = ui.input(|i| i.predicted_dt).clamp(1.0 / 120.0, 0.1);

        if is_pulling_wire {
            let Some(pointer) = ui
                .ctx()
                .pointer_hover_pos()
                .or_else(|| ui.ctx().pointer_latest_pos())
            else {
                return;
            };
            let edge_margin = 72.0;
            let base_pan_speed = 560.0;
            let mut pan_delta = Vec2::ZERO;
            let mut max_edge_intensity: f32 = 0.0;

            if pointer.x < canvas.left() + edge_margin {
                let intensity =
                    ((canvas.left() + edge_margin - pointer.x) / edge_margin).clamp(0.0, 3.0);
                pan_delta.x += base_pan_speed * intensity * dt;
                max_edge_intensity = max_edge_intensity.max(intensity);
            } else if pointer.x > canvas.right() - edge_margin {
                let intensity =
                    ((pointer.x - (canvas.right() - edge_margin)) / edge_margin).clamp(0.0, 3.0);
                pan_delta.x -= base_pan_speed * intensity * dt;
                max_edge_intensity = max_edge_intensity.max(intensity);
            }
            if pointer.y < canvas.top() + edge_margin {
                let intensity =
                    ((canvas.top() + edge_margin - pointer.y) / edge_margin).clamp(0.0, 3.0);
                pan_delta.y += base_pan_speed * intensity * dt;
                max_edge_intensity = max_edge_intensity.max(intensity);
            } else if pointer.y > canvas.bottom() - edge_margin {
                let intensity =
                    ((pointer.y - (canvas.bottom() - edge_margin)) / edge_margin).clamp(0.0, 3.0);
                pan_delta.y -= base_pan_speed * intensity * dt;
                max_edge_intensity = max_edge_intensity.max(intensity);
            }

            if pan_delta != Vec2::ZERO {
                self.pan += pan_delta;
                ui.ctx().request_repaint();
            }

            let base_zoom = *self.wire_base_zoom.get_or_insert(self.zoom);
            let zoom_out_ratio = (max_edge_intensity / 1.5).clamp(0.0, 1.0);
            let target_zoom = (base_zoom * (1.0 - zoom_out_ratio * 0.22)).clamp(0.20, MAX_ZOOM);
            let zoom_change_rate = if target_zoom < self.zoom { 7.0 } else { 5.0 };
            let zoom_step = 1.0 - (-dt * zoom_change_rate).exp();
            let new_zoom = self.zoom + (target_zoom - self.zoom) * zoom_step;

            if (new_zoom - self.zoom).abs() > 0.0005 {
                self.zoom_around_center(canvas, new_zoom);
                ui.ctx().request_repaint();
            }
        } else if let Some(base_zoom) = self.wire_base_zoom {
            let zoom_step = 1.0 - (-dt * 10.0).exp();
            let new_zoom = self.zoom + (base_zoom - self.zoom) * zoom_step;
            if (new_zoom - self.zoom).abs() > 0.001 {
                self.zoom_around_center(canvas, new_zoom);
                ui.ctx().request_repaint();
            } else {
                self.zoom = base_zoom;
                self.wire_base_zoom = None;
            }
        }
    }

    /// Immediately restores the zoom captured when edge-navigation began.
    pub fn cancel_wire_navigation(&mut self) {
        if let Some(base_zoom) = self.wire_base_zoom.take() {
            let canvas = Rect::from_min_size(Pos2::ZERO, self.canvas_size);
            self.zoom_around_center(canvas, base_zoom);
        }
    }

    #[cfg(test)]
    pub(super) fn wire_base_zoom(&self) -> Option<f32> {
        self.wire_base_zoom
    }

    fn zoom_around_center(&mut self, canvas: Rect, zoom: f32) {
        let center = canvas.size() * 0.5;
        let graph_position = (center - self.pan) / self.zoom;
        self.zoom = zoom;
        self.pan = center - graph_position * zoom;
    }
}

pub(crate) fn canvas_viewport(parent: &mut egui::Ui, canvas: Rect) -> egui::Ui {
    let mut viewport = parent.new_child(
        UiBuilder::new()
            .max_rect(canvas)
            .layout(Layout::top_down(Align::Min)),
    );
    viewport.set_clip_rect(parent.clip_rect().intersect(canvas));
    viewport
}

pub(crate) fn paint_grid(ui: &egui::Ui, canvas: Rect, state: &GraphCanvasState, color: Color32) {
    let painter = ui.painter();
    let grid = (32.0 * state.zoom).max(8.0);
    let mut x = canvas.left() + state.pan.x.rem_euclid(grid);
    while x <= canvas.right() {
        painter.line_segment(
            [Pos2::new(x, canvas.top()), Pos2::new(x, canvas.bottom())],
            Stroke::new(1.0, color),
        );
        x += grid;
    }
    let mut y = canvas.top() + state.pan.y.rem_euclid(grid);
    while y <= canvas.bottom() {
        painter.line_segment(
            [Pos2::new(canvas.left(), y), Pos2::new(canvas.right(), y)],
            Stroke::new(1.0, color),
        );
        y += grid;
    }
}

pub(crate) fn bezier_points(from: Pos2, to: Pos2) -> [Pos2; 4] {
    let dx = ((to.x - from.x).abs() * 0.5).max(48.0);
    [
        from,
        Pos2::new(from.x + dx, from.y),
        Pos2::new(to.x - dx, to.y),
        to,
    ]
}

pub(crate) fn paint_wire(ui: &egui::Ui, points: [Pos2; 4], stroke: Stroke) {
    ui.painter().add(egui::Shape::CubicBezier(
        egui::epaint::CubicBezierShape::from_points_stroke(
            points,
            false,
            Color32::TRANSPARENT,
            stroke,
        ),
    ));
}

/// Paints a connected-but-inactive route without changing the graph's hit area.
/// Domain editors decide what "inactive" means; the canvas only owns the visual.
pub(crate) fn paint_dashed_wire(ui: &egui::Ui, points: [Pos2; 4], stroke: Stroke) {
    const STEPS: usize = 48;
    const DRAW_STEPS: usize = 3;
    const PERIOD: usize = 5;
    for step in 0..STEPS {
        if step % PERIOD >= DRAW_STEPS {
            continue;
        }
        let start = cubic_point(points, step as f32 / STEPS as f32);
        let end = cubic_point(points, (step + 1) as f32 / STEPS as f32);
        ui.painter().line_segment([start, end], stroke);
    }
}

pub(crate) fn distance_to_curve(pointer: Pos2, points: [Pos2; 4]) -> f32 {
    let mut min_distance = f32::INFINITY;
    let mut previous = points[0];
    for step in 1..=32 {
        let next = cubic_point(points, step as f32 / 32.0);
        min_distance = min_distance.min(distance_to_segment(pointer, previous, next));
        previous = next;
    }
    min_distance
}

pub(crate) fn rect_between(first: Pos2, second: Pos2) -> Rect {
    Rect::from_min_max(
        Pos2::new(first.x.min(second.x), first.y.min(second.y)),
        Pos2::new(first.x.max(second.x), first.y.max(second.y)),
    )
}

pub(crate) fn paint_selection_box(ui: &egui::Ui, selection: Rect, accent: Color32) {
    ui.painter().rect_filled(
        selection,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 24),
    );
    ui.painter().rect_stroke(
        selection,
        CornerRadius::ZERO,
        Stroke::new(1.0, accent),
        egui::epaint::StrokeKind::Inside,
    );
}

pub(crate) fn cubic_point(points: [Pos2; 4], t: f32) -> Pos2 {
    let a = points[0].lerp(points[1], t);
    let b = points[1].lerp(points[2], t);
    let c = points[2].lerp(points[3], t);
    a.lerp(b, t).lerp(b.lerp(c, t), t)
}

fn distance_to_segment(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_sq();
    if length_squared <= f32::EPSILON {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_bounds_inside_canvas() {
        let available = Vec2::new(1000.0, 600.0);
        let canvas = Rect::from_min_size(Pos2::ZERO, available);
        let bounds = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(920.0, 680.0));
        let mut state = GraphCanvasState::default();
        state.fit_to_bounds(bounds, available, Vec2::new(220.0, 84.0));

        let transformed = state.graph_rect(canvas, [100.0, 100.0], bounds.size());
        assert!(canvas.contains(transformed.min));
        assert!(canvas.contains(transformed.max));
    }

    #[test]
    fn curve_distance_tracks_wire_hit_area() {
        let points = bezier_points(Pos2::new(0.0, 100.0), Pos2::new(200.0, 100.0));
        assert!(distance_to_curve(Pos2::new(100.0, 100.0), points) < 1.0);
        assert!((distance_to_curve(Pos2::new(100.0, 90.0), points) - 10.0).abs() < 1.5);
        assert!(distance_to_curve(Pos2::new(100.0, 200.0), points) > 50.0);
    }

    #[test]
    fn cancelling_wire_navigation_restores_the_captured_zoom() {
        let mut state = GraphCanvasState::default();
        state.canvas_size = Vec2::new(800.0, 500.0);
        state.zoom = 0.72;
        state.wire_base_zoom = Some(1.0);
        state.cancel_wire_navigation();
        assert_eq!(state.zoom, 1.0);
        assert!(state.wire_base_zoom.is_none());
    }
}
