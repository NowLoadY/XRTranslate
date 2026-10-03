//! A rounded ribbon loft: landmarks control curl, width and thickness; tangents are inferred.
use super::super::geometry::{Spline, signed_power};
use glam::{Vec2, Vec3, vec2, vec3};
use std::f32::consts::{PI, TAU};

pub(super) struct Strand {
    curve: Spline<6, 9>,
    top: f32,
    bottom: f32,
}

impl Strand {
    // Each section specifies height, center X/Z, direction, half-width/depth and curvature.
    pub fn new(sections: [[f32; 7]; 4], tip: Vec3, roots: [[Vec3; 4]; 4]) -> Self {
        let mut values = [[0.0; 6]; 9];
        let mut heights = [0.0; 9];
        for (i, root) in roots.into_iter().enumerate() {
            let points = root.map(|p| vec2(p.x, p.z));
            let center = (points[0] + points[2]) * 0.5;
            let across = (points[0] - points[2]) * 0.5;
            let axis = across.normalize_or(Vec2::X);
            let normal = vec2(-axis.y, axis.x);
            values[i] = [
                center.x,
                center.y,
                axis.y.atan2(axis.x),
                across.length(),
                (points[1] - points[3]).dot(normal) * 0.5,
                ((points[1] + points[3]) * 0.5 - center).dot(normal),
            ];
            if i > 1 {
                values[i][2] += ((values[i - 1][2] - values[i][2]) / TAU).round() * TAU;
            }
            heights[i] = root[0].y;
        }
        values[0][2] = values[1][2];
        for (i, [y, x, z, mut angle, width, depth, mut bend]) in sections.into_iter().enumerate() {
            // Opposite axes describe the same section; choose continuous frames.
            let turns = ((values[i + 3][2] - angle) / PI).round();
            angle += turns * PI;
            if turns as i32 % 2 != 0 {
                bend = -bend;
            }
            values[i + 4] = [x, z, angle, width, depth, bend];
            heights[i + 4] = y;
        }
        values[8] = [tip.x, tip.z, values[7][2], 0.0, 0.0, 0.0];
        heights[8] = tip.y;
        let top = heights[0];
        // Cosine height coordinates round a closing tip without a separately authored cap.
        let knots = heights.map(|y| Self::parameter(y, top, tip.y));
        Self {
            curve: Spline::new(knots, values).flat_ends(&[0, 1, 2, 5]),
            top,
            bottom: tip.y,
        }
    }

    fn parameter(y: f32, top: f32, bottom: f32) -> f32 {
        (2.0 * (y - bottom) / (top - bottom) - 1.0)
            .clamp(-1.0, 1.0)
            .acos()
            / PI
    }

    pub fn point(&self, u: f32, y: f32, side: f32) -> Vec3 {
        let [x, z, angle, width, depth, bend] =
            self.curve.sample(Self::parameter(y, self.top, self.bottom));
        let axis = Vec2::from_angle(angle);
        let across = signed_power((u * TAU).cos(), 0.82) * -side;
        let offset = signed_power((u * TAU).sin(), 0.80) * depth + bend * (1.0 - across * across);
        let p = vec2(x, z) + axis * (across * width) + vec2(-axis.y, axis.x) * offset;
        vec3(p.x * -side, y, p.y)
    }
}
