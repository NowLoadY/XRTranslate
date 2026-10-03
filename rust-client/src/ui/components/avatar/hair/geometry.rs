//! Fitted scalp and swept locks; every surface is generated from editable curves.
use super::super::geometry::{Mesh, SurfaceSamples, bezier, curve, cushion, signed_power};
use glam::{Vec2, Vec3, Vec4, vec2, vec3, vec4};
use std::f32::consts::{PI, TAU};

const RING_POWER: f32 = 0.95;

// Width, height, front depth and back depth; the bob and forehead vary independently.
const PROFILE: [[Vec4; 4]; 3] = [
    [
        vec4(0.0000, 1.0900, 0.0000, 0.0000),
        vec4(0.6132, 1.0900, 0.4953, 0.5099),
        vec4(0.9588, 0.8200, 0.9703, 0.8482),
        vec4(1.0162, 0.3900, 1.0083, 0.8922),
    ],
    [
        vec4(1.0162, 0.3900, 1.0083, 0.8922),
        vec4(1.0656, 0.0200, 1.0410, 0.9302),
        vec4(1.3720, -0.3600, 0.9389, 1.1431),
        vec4(1.0833, -0.6200, 0.8845, 1.0378),
    ],
    [
        vec4(1.0833, -0.6200, 0.8845, 1.0378),
        vec4(0.9168, -0.7700, 0.8531, 0.9771),
        vec4(0.9037, -0.8500, 0.6848, 0.8533),
        vec4(0.8000, -0.8750, 0.7000, 0.7200),
    ],
];
const FRINGE: [[Vec2; 4]; 6] = [
    [
        vec2(0.0, 0.0),
        vec2(0.80, 0.06),
        vec2(1.10, 0.42),
        vec2(1.0, 0.72),
    ],
    [
        vec2(1.0, 0.72),
        vec2(0.98, 0.96),
        vec2(0.86, 1.0),
        vec2(0.65, 1.0),
    ],
    [
        vec2(0.65, 1.0),
        vec2(0.45, 1.0),
        vec2(0.20, 1.0),
        vec2(0.0, 1.0),
    ],
    [
        vec2(0.0, 1.0),
        vec2(-0.20, 1.0),
        vec2(-0.45, 1.0),
        vec2(-0.65, 1.0),
    ],
    [
        vec2(-0.65, 1.0),
        vec2(-0.86, 1.0),
        vec2(-0.98, 0.96),
        vec2(-1.0, 0.72),
    ],
    [
        vec2(-1.0, 0.72),
        vec2(-1.10, 0.42),
        vec2(-0.80, 0.06),
        vec2(0.0, 0.0),
    ],
];
// Cheek locks curve around the opening and turn inward above the long bob.
const FRAME: [[Vec2; 4]; 4] = [
    [
        vec2(0.24, 1.02),
        vec2(0.85, 1.03),
        vec2(1.13, 0.43),
        vec2(0.98, -0.23),
    ],
    [
        vec2(0.98, -0.23),
        vec2(0.93, -0.48),
        vec2(0.78, -0.56),
        vec2(0.65, -0.49),
    ],
    [
        vec2(0.65, -0.49),
        vec2(0.77, -0.38),
        vec2(0.80, -0.12),
        vec2(0.77, 0.16),
    ],
    [
        vec2(0.77, 0.16),
        vec2(0.76, 0.62),
        vec2(0.54, 0.99),
        vec2(0.24, 1.02),
    ],
];

// The crown parting, fringe arc and rear V are shallow cuts in the same surface.
fn parting(point: Vec3) -> f32 {
    let front = point.z - 0.412 - 0.50 * point.x.abs().powf(1.7);
    let back = point.x.abs() - 0.48 * (-point.z - 0.023).max(0.0).powf(1.2);
    let crown_part = point.z - 0.284 + 0.07 * point.x * point.x;
    let rear = ((-point.z - 0.02) / 0.08).clamp(0.0, 1.0);
    let crown = ((point.y - 0.42) / 0.30).clamp(0.0, 1.0);
    let opening = 1.0 - (-(point.x / 0.13).powi(2)).exp();
    crown
        * crown
        * (3.0 - 2.0 * crown)
        * ((-((front / 0.024).powi(2))).exp() * opening
            + (-((back / 0.021).powi(2))).exp() * rear
            + (-((crown_part / 0.021).powi(2))).exp())
}

fn ring(profile: Vec4, angle: f32, offset: f32) -> Vec3 {
    let z = signed_power(angle.cos(), RING_POWER);
    Vec3::new(
        (profile.x + offset).max(0.0) * signed_power(angle.sin(), RING_POWER),
        profile.y,
        ((if z >= 0.0 { profile.z } else { profile.w }) + offset).max(0.0) * z,
    )
}

fn profile_at(y: f32) -> (f32, Vec4) {
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..22 {
        let t = (low + high) * 0.5;
        if curve(&PROFILE, t).y > y {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    (t, curve(&PROFILE, t))
}

fn front(p: Vec2, thickness: f32) -> Vec3 {
    let (_, profile) = profile_at(p.y);
    let exponent = 2.0 / RING_POWER;
    let z = (1.0 - (p.x.abs() / profile.x.max(0.001)).powf(exponent))
        .max(0.0)
        .powf(1.0 / exponent);
    Vec3::new(p.x, p.y, z * profile.z + thickness)
}

fn bend(mut point: Vec3, t: f32, strength: f32, sway: Vec2) -> Vec3 {
    // Roots sit inside the continuous crown, hiding the joins of separate locks.
    point.y -= 0.035 * (1.0 - root(t));
    point.y -= 0.018 * parting(point);
    let weight = t.clamp(0.0, 1.0).powi(3) * strength;
    point.x += sway.x * weight * 0.15;
    point.z += sway.y * weight * 0.12;
    point
}

fn root(t: f32) -> f32 {
    smooth((t - 0.15) / 0.27)
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn scalp(u: f32, v: f32, clearance: f32) -> Vec3 {
    let angle = u * TAU - PI;
    let side = smooth((angle.abs() - 0.72) / 0.38);
    // The cap stops under the fringe roots; it must not bridge the visible gaps.
    let t = v * (0.28 + 0.30 * side);
    let p = curve(&PROFILE, t);
    let mut point = ring(p + Vec4::Y * 0.006, angle, -clearance * root(t));
    point.y -= 0.018 * parting(point);
    // Close inside the head, so the scalp cannot leave a visible cut edge between locks.
    point.lerp(Vec3::Y * PROFILE[0][3].y, smooth((v - 0.75) / 0.25))
}

fn hairstyle(sway: Vec2) -> Mesh {
    // Root/bend/turn, width/depth/taper, lift and tip for each mirrored lock.
    let locks: [([f32; 3], [f32; 3], f32, Vec3); 6] = [
        (
            [0.0000, 0.0000, 0.0000],
            [0.3695, 0.3805, 0.6035],
            -0.0234,
            vec3(0.0, -0.853, -0.798),
        ),
        (
            [0.5750, 0.4649, 0.6071],
            [0.2358, 0.3520, 0.0947],
            0.0994,
            vec3(0.370, -0.847, -0.632),
        ),
        (
            [0.9370, 0.5502, 1.0520],
            [0.2395, 0.3433, 0.4027],
            0.2502,
            vec3(0.645, -0.848, -0.401),
        ),
        (
            [1.3560, 1.5427, 1.0075],
            [0.2652, 0.2583, 0.5699],
            0.0936,
            vec3(0.668, -0.835, -0.050),
        ),
        (
            [1.5930, 1.7090, 1.7194],
            [0.3222, 0.2925, 0.2726],
            -0.0954,
            vec3(0.715, -0.875, 0.410),
        ),
        (
            [1.8000, 1.5163, 1.3903],
            [0.2725, 0.2643, 0.7631],
            0.0303,
            vec3(1.05, -0.650, -0.269),
        ),
    ];
    let clearance = locks
        .iter()
        .map(|(_, section, _, _)| section[1] * 0.5)
        .fold(0.0, f32::max);
    let mut mesh = Mesh::default();
    let samples = SurfaceSamples::new(128, 48, |u, v| scalp(u, v, clearance));
    let crown = |u, v| scalp(u, samples.at(u, v), clearance);
    mesh.shaded_surface(48, 128, true, crown, |u, v| {
        1.0 - 0.22 * parting(crown(u, v))
    });
    for ([start, bend_angle, turn], [width, depth, taper], lift, tip) in locks {
        // A tip is a landmark, not an added translation. Solve its coordinates on
        // the loft so the whole end section follows the head's curvature naturally.
        let (reach, p) = profile_at(tip.y);
        let radius_z = if tip.z >= 0.0 { p.z } else { p.w };
        let (mut low, mut high) = (-p.x.min(radius_z) + 0.0001, tip.x.hypot(tip.z));
        for _ in 0..22 {
            let offset = (low + high) * 0.5;
            let distance = (tip.x.abs() / (p.x + offset)).powf(2.0 / RING_POWER)
                + (tip.z.abs() / (radius_z + offset)).powf(2.0 / RING_POWER);
            if distance > 1.0 {
                low = offset;
            } else {
                high = offset;
            }
        }
        let tip_offset = (low + high) * 0.5;
        let angle = signed_power(tip.x / (p.x + tip_offset), 1.0 / RING_POWER).atan2(signed_power(
            tip.z / (radius_z + tip_offset),
            1.0 / RING_POWER,
        ));
        let direction = [start, bend_angle, turn, PI - angle];
        // Round the last quarter of the local thickness. Interpolating squared
        // width matches the incoming tangent and gives the closed tip a finite radius.
        let bevel = depth.min(width * p.x) * 0.25;
        let (end_t, _) = profile_at(tip.y + bevel);
        let cap_t = end_t / reach;
        let area = width.powi(2) * (PI * cap_t).sin().powf(2.0 * taper);
        let dy =
            (curve(&PROFILE, end_t + 0.0005).y - curve(&PROFILE, end_t - 0.0005).y) * reach / 0.001;
        let slope = area * 2.0 * taper * PI / (PI * cap_t).tan() * bevel / dy;
        for side in [-1.0, 1.0] {
            if direction[0] == 0.0 && side < 0.0 {
                continue;
            }
            let point = |u: f32, t: f32| {
                if t >= 1.0 {
                    return tip * vec3(-side, 1.0, 1.0);
                }
                let profile = curve(&PROFILE, t * reach);
                let end = (PI * t).sin().max(0.0);
                let span = if profile.y < tip.y + bevel {
                    let h = ((profile.y - tip.y) / bevel).clamp(0.0, 1.0);
                    (area * h * (2.0 - h) + slope * h * h * (h - 1.0))
                        .max(0.0)
                        .sqrt()
                } else {
                    width * end.powf(taper)
                };
                let volume = depth * end.sqrt();
                let cross = u * TAU;
                let across = signed_power(cross.cos(), 0.82);
                let theta = PI + side * bezier(direction, t) + across * span;
                let offset = (0.035 * end + lift * end * t) * root(t * reach)
                    + tip_offset * smooth(t)
                    - volume * (1.0 - signed_power(cross.sin(), 0.80)) * 0.5;
                ring(profile, theta, offset)
            };
            let samples =
                SurfaceSamples::new(48, 64, |u, t| bend(point(u, t), t, 0.85, Vec2::ZERO));
            mesh.shaded_surface(
                64,
                48,
                false,
                |u, v| {
                    let t = samples.at(u, v);
                    bend(point(u, t), t, 0.85, sway)
                },
                |u, v| {
                    1.0 - 0.18 * (1.0 - (u * TAU).sin().max(0.0)).powi(2) * root(samples.at(u, v))
                },
            );
        }
    }
    let mut inner = [FRAME[3], FRAME[2]];
    inner.iter_mut().for_each(|segment| segment.reverse());
    for side in [-1.0, 1.0] {
        mesh.surface(64, 32, true, |u, t| {
            let angle = u * TAU;
            let across = (1.0 + signed_power(angle.cos(), 0.75)) * 0.5;
            let p = curve(&FRAME[..2], t).lerp(curve(&inner, t), across);
            let depth = signed_power(angle.sin(), 0.65) * 0.045 * (PI * t).sin().max(0.0).sqrt();
            let mut point = front(p, (depth * side + 0.025) * root(t) - 0.03 * (1.0 - root(t)));
            point.x *= side;
            point.z -= 0.15 * (PI * t).sin().max(0.0).sqrt() * root(t);
            bend(point, t, 0.45, sway)
        });
    }
    for (offset, width, length) in [
        (-0.67_f32, 0.10, 1.02),
        (-0.48, 0.15, 0.99),
        (0.0, 0.29, 0.99),
        (0.48, 0.15, 0.99),
        (0.67, 0.10, 1.02),
    ] {
        mesh.append(cushion(&FRINGE, vec2(0.0, 0.55), 0.032, |p, thickness| {
            let t = p.y;
            let x = offset * (1.0 - (1.0 - t).powi(3)) + p.x * width;
            let notch = if offset == 0.0 {
                0.045 * (-((p.x.abs() - 0.58) / 0.08).powi(2)).exp() * t.powi(12)
            } else {
                0.0
            };
            let bulge = 0.030 * (1.0 - p.x * p.x).max(0.0);
            let volume = 0.12 + 0.26 * (PI * t).sin().max(0.0).sqrt();
            let depth = 0.050 + bulge - volume * (1.0 - thickness / 0.032) * 0.5;
            let slope = if offset.abs() > 0.6 {
                -offset.signum() * 0.07 * p.x * t.powi(4)
            } else {
                0.0
            };
            let point = front(
                vec2(x, 1.09 - t * (length - 0.01) + notch + slope),
                depth * root(t) - 0.03 * (1.0 - root(t)),
            );
            bend(point, t, 0.13, sway)
        }));
    }
    mesh
}

pub(in super::super) fn mesh() -> Mesh {
    hairstyle(Vec2::ZERO).morph(hairstyle(Vec2::X), hairstyle(Vec2::Y))
}
