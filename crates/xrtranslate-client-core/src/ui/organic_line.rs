use eframe::egui::{self, Pos2, Rect, Stroke};

/// Paints a hand-drawn stroke with organic micro-perturbation between two points.
pub fn paint_hand_drawn_line(
    painter: &egui::Painter,
    id: egui::Id,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
) {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = dx.hypot(dy);
    if length < 2.0 {
        return;
    }
    let id_value = id.value();
    let seed = ((id_value ^ (id_value >> 32)) & 0xffff) as f32 / 257.0;

    let step_size = 3.5;
    let steps = ((length / step_size).ceil() as usize).max(3);
    let mut points = Vec::with_capacity(steps + 1);

    let nx = -dy / length;
    let ny = dx / length;

    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let base_x = start.x + dx * t;
        let base_y = start.y + dy * t;

        // Endpoints fade to exactly zero displacement to preserve layout alignment
        let edge_fade = (t * std::f32::consts::PI).sin();
        // A fixed visual-noise coefficient, not an angle; preserve existing strokes.
        #[allow(clippy::approx_constant)]
        let phase1 = (t * std::f32::consts::TAU * 1.5 + seed * 3.14).sin();
        let phase2 = (t * std::f32::consts::TAU * 3.7 + seed * 1.73).cos() * 0.35;
        let offset = (phase1 + phase2) * 0.70 * edge_fade;

        points.push(egui::pos2(base_x + nx * offset, base_y + ny * offset));
    }

    painter.add(egui::Shape::line(points, stroke));
}

/// Paints an organic hand-drawn bottom line across the bottom edge of a given rectangle.
pub fn paint_hand_drawn_bottom_line(
    painter: &egui::Painter,
    id: egui::Id,
    rect: Rect,
    stroke: Stroke,
) {
    let start = egui::pos2(rect.left() + 1.0, rect.bottom() - 1.5);
    let end = egui::pos2(rect.right() - 1.0, rect.bottom() - 1.5);
    paint_hand_drawn_line(painter, id, start, end, stroke);
}

/// Paints an organic hand-drawn bounding rectangle.
pub fn paint_hand_drawn_rect(painter: &egui::Painter, id: egui::Id, rect: Rect, stroke: Stroke) {
    let tl = rect.left_top();
    let tr = rect.right_top();
    let br = rect.right_bottom();
    let bl = rect.left_bottom();

    paint_hand_drawn_line(painter, id.with("t"), tl, tr, stroke);
    paint_hand_drawn_line(painter, id.with("r"), tr, br, stroke);
    paint_hand_drawn_line(painter, id.with("b"), br, bl, stroke);
    paint_hand_drawn_line(painter, id.with("l"), bl, tl, stroke);
}

/// Paints a hand-drawn checkmark with organic sketch stroke.
pub fn paint_hand_drawn_checkmark(
    painter: &egui::Painter,
    id: egui::Id,
    rect: Rect,
    stroke: Stroke,
) {
    let start = egui::pos2(
        rect.left() + rect.width() * 0.22,
        rect.top() + rect.height() * 0.52,
    );
    let mid = egui::pos2(
        rect.left() + rect.width() * 0.44,
        rect.bottom() - rect.height() * 0.22,
    );
    let end = egui::pos2(
        rect.right() - rect.width() * 0.18,
        rect.top() + rect.height() * 0.22,
    );

    paint_hand_drawn_line(painter, id.with("check_down"), start, mid, stroke);
    paint_hand_drawn_line(painter, id.with("check_up"), mid, end, stroke);
}

/// Paints a hand-drawn circle with subtle sketch perturbation.
pub fn paint_hand_drawn_circle(
    painter: &egui::Painter,
    id: egui::Id,
    center: Pos2,
    radius: f32,
    stroke: Stroke,
) {
    if radius < 1.0 {
        return;
    }
    let id_value = id.value();
    let seed = ((id_value ^ (id_value >> 32)) & 0xffff) as f32 / 257.0;

    let steps = 16;
    let mut points = Vec::with_capacity(steps + 1);

    for i in 0..=steps {
        let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
        let phase = (angle * 3.0 + seed * 2.1).sin() * 0.55;
        let r = radius + phase;
        points.push(egui::pos2(
            center.x + r * angle.cos(),
            center.y + r * angle.sin(),
        ));
    }

    painter.add(egui::Shape::closed_line(points, stroke));
}
