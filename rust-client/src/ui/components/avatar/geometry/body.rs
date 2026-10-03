//! Cross-section loft: a broad forehead, full cheeks and a forward-set, softly flattened chin.
use super::{Mesh, profile::Profile, signed_power};
use glam::{Mat4, Vec2, Vec3, Vec4, vec4};
use std::{f32::consts::TAU, sync::LazyLock};

// Measured sections: crown, temples, cheeks, jaw and chin. Change these
// landmarks to author a head; smooth tangents and rounded poles are derived.
static PROFILE: LazyLock<Profile<14>> = LazyLock::new(|| {
    Profile::new([
        vec4(0.0000, 0.1200, 0.1200, 0.9400),
        vec4(0.4742, 0.5975, -0.3385, 0.8000),
        vec4(0.6739, 0.7878, -0.5071, 0.6000),
        vec4(0.8188, 0.8947, -0.6179, 0.3000),
        vec4(0.8735, 0.9326, -0.6657, 0.0000),
        vec4(0.8801, 0.9553, -0.6735, -0.1500),
        vec4(0.8640, 0.9804, -0.6652, -0.3500),
        vec4(0.8205, 0.9861, -0.6357, -0.5000),
        vec4(0.7354, 0.9585, -0.5409, -0.6500),
        vec4(0.6380, 0.9136, -0.2528, -0.7500),
        vec4(0.5239, 0.8634, 0.1439, -0.8000),
        vec4(0.3689, 0.7852, 0.3806, -0.8400),
        vec4(0.2306, 0.7133, 0.4723, -0.8600),
        vec4(0.0000, 0.6000, 0.6000, -0.8715),
    ])
});

/// Independent cheek and chin fullness, from -1 (narrow) to 1 (round).
/// Features and clothing are fitted from the same surface, not separate presets.
#[derive(Clone, Copy, Default)]
pub struct HeadShape {
    pub cheeks: f32,
    pub chin: f32,
}

impl HeadShape {
    pub(crate) fn weights(self) -> [f32; 2] {
        [self.cheeks.clamp(-1.0, 1.0), self.chin.clamp(-1.0, 1.0)]
    }

    pub(crate) fn width_at(self, y: f32) -> f32 {
        let height = PROFILE.height_fraction(y);
        let [cheeks, chin] = self.weights();
        1.0 + cheeks * 0.16 * (-((height - 0.39) / 0.24).powi(2)).exp()
            + chin * 0.26 * (-((height - 0.07) / 0.13).powi(2)).exp()
    }

    fn deform(self, mut point: Vec3) -> Vec3 {
        point.x *= self.width_at(point.y);
        point
    }

    pub(crate) fn envelope_width(self) -> f32 {
        let width = PROFILE.sections().iter().map(|s| s.x).fold(0.0, f32::max);
        PROFILE
            .sections()
            .iter()
            .map(|s| s.x * self.width_at(s.w))
            .fold(0.0, f32::max)
            / width
    }

    pub(crate) fn frame(self, p: Vec2) -> Mat4 {
        let surface = |p| self.deform(front(p));
        let x = (surface(p + Vec2::X * 0.001) - surface(p - Vec2::X * 0.001)).normalize();
        let y = (surface(p + Vec2::Y * 0.001) - surface(p - Vec2::Y * 0.001)).normalize();
        let normal = x.cross(y).normalize();
        Mat4::from_cols(
            y.cross(normal).extend(0.0),
            y.extend(0.0),
            normal.extend(0.0),
            surface(p).extend(1.0),
        )
    }
}

fn roundness(y: f32) -> f32 {
    // The temples taper gently, while the lower cheeks fill out before the chin.
    2.135 - 0.190 * (-((y - 0.32) / 0.32).powi(2)).exp()
        + 0.250 * (-((y + 0.59) / 0.22).powi(2)).exp()
        - 0.538 * (-((y + 0.84) / 0.10).powi(2)).exp()
}

fn point(profile: Vec4, angle: f32, offset: f32) -> Vec3 {
    let power = 2.0 / roundness(profile.w);
    Vec3::new(
        (profile.x + offset) * signed_power(angle.cos(), power),
        profile.w,
        (profile.y + profile.z) * 0.5
            + ((profile.y - profile.z) * 0.5 + offset) * signed_power(angle.sin(), power),
    )
}

pub fn ring(y: f32, angle: f32, offset: f32) -> Vec3 {
    point(PROFILE.at_height(y), angle, offset)
}

fn front(p: Vec2) -> Vec3 {
    let profile = PROFILE.at_height(p.y);
    let roundness = roundness(p.y);
    let z = (1.0 - (p.x.abs() / profile.x.max(0.001)).powf(roundness))
        .max(0.0)
        .powf(1.0 / roundness);
    Vec3::new(
        p.x,
        p.y,
        (profile.y + profile.z) * 0.5 + (profile.y - profile.z) * 0.5 * z,
    )
}

pub fn mesh() -> Mesh {
    let build = |shape: HeadShape| {
        let mut mesh = Mesh::default();
        mesh.surface(80, 96, false, |u, v| {
            shape.deform(point(PROFILE.sample(v), u * TAU, 0.0))
        });
        mesh
    };
    build(HeadShape::default()).morph(
        build(HeadShape {
            cheeks: 1.0,
            chin: 0.0,
        }),
        build(HeadShape {
            cheeks: 0.0,
            chin: 1.0,
        }),
    )
}
