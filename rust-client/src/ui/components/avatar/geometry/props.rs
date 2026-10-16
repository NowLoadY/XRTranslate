//! Unit solids for the small spatial controls; dimensions live in their instances.
use super::{Mesh, signed_power};
use glam::Vec3;
use std::f32::consts::{PI, TAU};

pub(super) fn solid(box_shape: bool) -> Mesh {
    let mut mesh = Mesh::default();
    let exponent = if box_shape { 0.18 } else { 1.0 };
    // Low exponents amplify trigonometric roundoff into visible seam gaps.
    let power = |value: f32| signed_power(if value.abs() < 1e-6 { 0.0 } else { value }, exponent);
    mesh.surface(24, 40, false, |u, v| {
        let longitude = u * TAU;
        let latitude = v * PI;
        let radial = power(latitude.sin());
        Vec3::new(
            radial * power(longitude.cos()),
            power(latitude.cos()),
            radial * power(longitude.sin()),
        )
    });
    mesh
}
