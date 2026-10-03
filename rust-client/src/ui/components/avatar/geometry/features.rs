//! Continuous rounded strokes with matching topology for GPU expression morphs.
use super::Mesh;
use glam::{Vec2, Vec3};
use std::f32::consts::{FRAC_PI_2, TAU};

fn stroke(
    path: impl Fn(f32) -> Vec2,
    radius: impl Fn(f32) -> f32,
    depth: f32,
    scale: Vec2,
) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.surface(48, 24, true, |u, v| {
        // Eight rows per rounded end; the middle follows the whole curve without joints.
        let t = ((v * 48.0 - 8.0) / 32.0).clamp(0.0, 1.0);
        let radius = radius(t);
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
        let point = (path(t)
            + tangent * offset * radius * if t < 0.5 { -1.0 } else { 1.0 }
            + side * (angle.cos() * radius * round))
            * scale;
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
    // All poses run from left to right, so a blink closes vertically without
    // twisting a vertical stroke sideways halfway through the morph.
    let neutral = stroke(
        |t| Vec2::new((t - 0.5) * 0.06, 0.0),
        |t| 0.07 * (1.0 - 0.05 * (t * 2.0 - 1.0).powi(2)),
        0.020,
        Vec2::new(0.75, 2.83),
    );
    let happy = stroke(
        |t| {
            let x = t * 2.0 - 1.0;
            Vec2::new(x * 0.115, (1.0 - x * x) * 0.085 - 0.025)
        },
        |_| 0.054,
        0.020,
        Vec2::ONE,
    );
    let closed = stroke(
        |t| Vec2::new((t - 0.5) * 0.16, 0.0),
        |_| 0.025,
        0.012,
        Vec2::ONE,
    );
    neutral.morph(happy, closed)
}

pub(super) fn mouth() -> Mesh {
    let shape = |width: f32, bend: f32, thickness: f32| {
        stroke(
            |t| {
                let x = t * 2.0 - 1.0;
                Vec2::new(x * width, -bend * (1.0 - x * x))
            },
            |_| thickness,
            0.012,
            Vec2::ONE,
        )
    };
    // A closed line, a closed smile, and a rounded speaking mouth share topology.
    // A small negative smile weight gives concern a frown without another mesh.
    shape(0.095, 0.0, 0.017).morph(shape(0.12, 0.045, 0.020), shape(0.022, 0.015, 0.080))
}
