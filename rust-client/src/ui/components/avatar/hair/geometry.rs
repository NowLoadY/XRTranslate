//! Fitted scalp and swept locks; every surface is generated from editable curves.
use super::super::geometry::{Mesh, SurfaceSamples, bezier, curve, cushion, signed_power};
use super::strand::Strand;
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
// Lower strand landmarks fitted from reference sections, not mesh vertices.
// Height, center X/Z, direction, half-width/depth, curvature.
const SECTIONS: [[[f32; 7]; 4]; 6] = [
    [
        [-0.3150, 0.0000, -0.8659, 0.0000, 0.3966, 0.2016, -0.0156],
        [-0.4762, 0.0000, -0.8792, 0.0000, 0.3633, 0.1816, -0.0305],
        [-0.6778, 0.0000, -0.8366, 0.0000, 0.2755, 0.1487, -0.0450],
        [-0.8390, 0.0000, -0.7655, 0.0000, 0.0757, 0.0693, -0.0119],
    ],
    [
        [-0.3150, 0.5329, -0.7571, 0.8046, 0.2241, 0.1706, -0.0251],
        [-0.4744, 0.5219, -0.7648, 0.7392, 0.2291, 0.1593, -0.0321],
        [-0.6736, 0.4662, -0.7364, 0.6056, 0.2140, 0.1395, -0.0330],
        [-0.8330, 0.3339, -0.6450, -0.3242, 0.1234, 0.0712, 0.0003],
    ],
    [
        [-0.3150, 0.8967, -0.6311, 0.8225, 0.2439, 0.1657, 0.0111],
        [-0.4747, 0.8874, -0.6291, 0.8937, 0.2288, 0.1579, 0.0087],
        [-0.6743, 0.8108, -0.5620, 1.1633, 0.1889, 0.1542, -0.0118],
        [-0.8340, 0.6806, -0.4209, 1.9052, 0.0610, 0.0521, 0.0194],
    ],
    [
        [-0.3150, 0.8826, -0.2320, 1.0905, 0.2449, 0.1295, -0.0515],
        [-0.4706, 0.8963, -0.2434, 1.1489, 0.2140, 0.1203, -0.0424],
        [-0.6651, 0.8552, -0.2280, 1.3186, 0.1712, 0.1049, -0.0175],
        [-0.8207, 0.7079, -0.0975, 1.7456, 0.0754, 0.0429, 0.0091],
    ],
    [
        [-0.4650, 0.9132, 0.1665, 1.6929, 0.2466, 0.1587, -0.0200],
        [-0.5868, 0.9064, 0.1826, 1.6816, 0.2460, 0.1444, -0.0180],
        [-0.7392, 0.8578, 0.2699, 1.8501, 0.2475, 0.1064, -0.0096],
        [-0.8610, 0.7264, 0.3871, 2.0816, 0.1447, 0.0448, -0.0094],
    ],
    [
        [-0.3150, 1.1335, -0.2080, 1.4797, 0.1944, 0.0961, -0.0155],
        [-0.4116, 1.1298, -0.2289, 1.4199, 0.1614, 0.0901, -0.0121],
        [-0.5324, 1.0977, -0.2450, 1.2391, 0.1032, 0.0721, -0.0151],
        [-0.6291, 1.0475, -0.2491, 0.5541, 0.0368, 0.0290, -0.0006],
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

fn animated(build: impl Fn(Vec2) -> Mesh) -> Mesh {
    build(Vec2::ZERO).morph(build(Vec2::X), build(Vec2::Y))
}

pub(in super::super) fn mesh() -> Mesh {
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
    let samples = SurfaceSamples::new(128, 96, |u, v| scalp(u, v, clearance));
    let crown = |u, v| {
        let (u, v) = samples.at(u, v);
        scalp(u, v, clearance)
    };
    mesh.shaded_surface(96, 128, true, crown, |u, v| {
        1.0 - 0.22 * parting(crown(u, v))
    });
    for (index, ([start, bend_angle, turn], [width, depth, taper], lift, tip)) in
        locks.into_iter().enumerate()
    {
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
        let base_point = |u: f32, t: f32, side: f32| {
            if t >= 1.0 {
                return tip * vec3(-side, 1.0, 1.0);
            }
            let profile = curve(&PROFILE, t * reach);
            let end = (PI * t).sin().max(0.0);
            let span = width * end.powf(taper);
            let volume = depth * end.sqrt();
            let cross = u * TAU;
            let across = signed_power(cross.cos(), 0.82);
            let theta = PI + side * bezier(direction, t) + across * span;
            let offset = (0.035 * end + lift * end * t) * root(t * reach) + tip_offset * smooth(t)
                - volume * (1.0 - signed_power(cross.sin(), 0.80)) * 0.5;
            ring(profile, theta, offset)
        };
        let roots = std::array::from_fn(|i| {
            let height = PROFILE[0][0].y * (i as f32 * PI / 6.0).cos();
            let t = profile_at(height).0 / reach;
            std::array::from_fn(|j| base_point(j as f32 * 0.25, t, -1.0))
        });
        let strand = Strand::new(SECTIONS[index], tip, roots);
        for side in [-1.0, 1.0] {
            if direction[0] == 0.0 && side < 0.0 {
                continue;
            }
            let point = |u: f32, t: f32| {
                if t >= 1.0 {
                    tip * vec3(-side, 1.0, 1.0)
                } else {
                    strand.point(u, curve(&PROFILE, t * reach).y, side)
                }
            };
            let samples =
                SurfaceSamples::new(48, 64, |u, t| bend(point(u, t), t, 0.85, Vec2::ZERO));
            mesh.append(animated(|sway| {
                let mut part = Mesh::default();
                part.shaded_surface(
                    64,
                    48,
                    false,
                    |u, v| {
                        let (u, t) = samples.at(u, v);
                        bend(point(u, t), t, 0.85, sway)
                    },
                    |u, v| {
                        let (u, t) = samples.at(u, v);
                        1.0 - 0.18 * (1.0 - (u * TAU).sin().max(0.0)).powi(2) * root(t)
                    },
                );
                part
            }));
        }
    }
    let mut inner = [FRAME[3], FRAME[2]];
    inner.iter_mut().for_each(|segment| segment.reverse());
    for side in [-1.0, 1.0] {
        let point = |u: f32, t: f32| {
            let angle = u * TAU;
            let across = (1.0 + signed_power(angle.cos(), 0.75)) * 0.5;
            let p = curve(&FRAME[..2], t).lerp(curve(&inner, t), across);
            let depth = signed_power(angle.sin(), 0.65) * 0.045 * (PI * t).sin().max(0.0).sqrt();
            let mut point = front(p, (depth * side + 0.025) * root(t) - 0.03 * (1.0 - root(t)));
            point.x *= side;
            point.z -= 0.15 * (PI * t).sin().max(0.0).sqrt() * root(t);
            point
        };
        let samples = SurfaceSamples::new(32, 64, |u, t| bend(point(u, t), t, 0.45, Vec2::ZERO));
        mesh.append(animated(|sway| {
            let mut part = Mesh::default();
            part.surface(64, 32, true, |u, v| {
                let (u, t) = samples.at(u, v);
                bend(point(u, t), t, 0.45, sway)
            });
            part
        }));
    }
    for (offset, width, length) in [
        (-0.67_f32, 0.10, 1.02),
        (-0.48, 0.18, 0.998),
        (0.0, 0.29, 0.994),
        (0.48, 0.18, 0.998),
        (0.67, 0.10, 1.02),
    ] {
        mesh.append(animated(|sway| {
            cushion(&FRINGE, vec2(0.0, 0.55), 0.032, |p, thickness| {
                let t = p.y;
                let x = offset * (1.0 - (1.0 - t).powi(3)) + p.x * width;
                let notch = if offset == 0.0 {
                    0.026 * (-((p.x.abs() - 0.53) / 0.05).powi(2)).exp() * t.powi(12)
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
                    depth * root(t),
                );
                bend(point, t, 0.13, sway)
            })
        }));
    }
    mesh
}
