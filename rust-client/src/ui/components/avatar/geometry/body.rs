//! The same fitted body profile drives the mesh, face placement and clothing fit.
use super::{Mesh, curve, signed_power};
use glam::{Mat4, Vec2, Vec3};
use std::f32::consts::TAU;

// Radius/height control points: soft crown, fuller cheeks, gently flattened lower pole.
const PROFILE: [[Vec2; 4]; 3] = [
    [
        Vec2::new(0.0, 1.04),
        Vec2::new(0.46, 1.04),
        Vec2::new(0.82, 0.80),
        Vec2::new(0.93, 0.46),
    ],
    [
        Vec2::new(0.93, 0.46),
        Vec2::new(1.02, 0.14),
        Vec2::new(1.04, -0.21),
        Vec2::new(0.94, -0.46),
    ],
    [
        Vec2::new(0.94, -0.46),
        Vec2::new(0.84, -0.78),
        Vec2::new(0.48, -1.02),
        Vec2::new(0.0, -1.02),
    ],
];
const ROUNDNESS: f32 = 2.15;

pub fn radius(y: f32) -> f32 {
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..18 {
        let middle = (low + high) * 0.5;
        if curve(&PROFILE, middle).y > y {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve(&PROFILE, (low + high) * 0.5).x
}

pub fn ring(radius: f32, y: f32, angle: f32) -> Vec3 {
    Vec3::new(
        radius * signed_power(angle.cos(), 2.0 / ROUNDNESS),
        y,
        radius * signed_power(angle.sin(), 2.0 / ROUNDNESS),
    )
}

pub fn front(point: Vec2) -> Vec3 {
    let radius = radius(point.y);
    let z = (radius.powf(ROUNDNESS) - point.x.abs().powf(ROUNDNESS))
        .max(0.0)
        .powf(1.0 / ROUNDNESS);
    Vec3::new(point.x, point.y, z)
}

pub fn frame(point: Vec2) -> Mat4 {
    let x = (front(point + Vec2::X * 0.001) - front(point - Vec2::X * 0.001)).normalize();
    let y = (front(point + Vec2::Y * 0.001) - front(point - Vec2::Y * 0.001)).normalize();
    let normal = x.cross(y).normalize();
    Mat4::from_cols(
        y.cross(normal).extend(0.0),
        y.extend(0.0),
        normal.extend(0.0),
        front(point).extend(1.0),
    )
}

pub fn mesh() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.surface(72, 96, false, |u, v| {
        let profile = curve(&PROFILE, v);
        ring(profile.x, profile.y, u * TAU)
    });
    mesh
}
