//! Continuous rounded strokes with matching topology for GPU expression morphs.
use super::Mesh;
use glam::{Vec2, Vec3};
use std::f32::consts::{FRAC_PI_2, TAU};

fn stroke(path: impl Fn(f32) -> Vec2, radius: f32, depth: f32) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.surface(48, 24, true, |u, v| {
        // Eight rows per rounded end; the middle follows the whole curve without joints.
        let t = ((v * 48.0 - 8.0) / 32.0).clamp(0.0, 1.0);
        let tangent = (path((t + 0.001).min(1.0)) - path((t - 0.001).max(0.0))).normalize();
        let side = Vec2::new(tangent.y, -tangent.x);
        let cap = if v < 1.0 / 6.0 {
            v * 6.0
        } else if v > 5.0 / 6.0 {
            (1.0 - v) * 6.0
        } else {
            1.0
        };
        let (round, offset) = (cap * FRAC_PI_2).sin_cos();
        let angle = u * TAU;
        let point = path(t)
            + tangent * offset * radius * if t < 0.5 { -1.0 } else { 1.0 }
            + side * (angle.cos() * radius * round);
        // The back follows the local curvature of the face instead of floating above it.
        Vec3::new(
            point.x,
            point.y,
            depth * (0.65 + angle.sin() * round) - 0.48 * point.length_squared(),
        )
    });
    mesh
}

pub(super) fn eye() -> Mesh {
    let neutral = stroke(|t| Vec2::new(0.0, (t - 0.5) * 0.25), 0.073, 0.034);
    let happy = stroke(
        |t| {
            let x = t * 2.0 - 1.0;
            Vec2::new(x * 0.115, (1.0 - x * x) * 0.085 - 0.025)
        },
        0.054,
        0.030,
    );
    let closed = stroke(|t| Vec2::new((t - 0.5) * 0.16, 0.0), 0.025, 0.018);
    neutral.morph(happy, closed)
}

pub(super) fn mouth() -> Mesh {
    let smile = |width, height| {
        stroke(
            |t| {
                let x = t * 2.0 - 1.0;
                Vec2::new(x * width, -(1.0 - x * x) * height)
            },
            0.025,
            0.020,
        )
    };
    let curious = stroke(
        |t| {
            let angle = t * TAU;
            Vec2::new(angle.cos() * 0.038, angle.sin() * 0.046 - 0.025)
        },
        0.018,
        0.017,
    );
    smile(0.088, 0.058).morph(smile(0.115, 0.085), curious)
}
