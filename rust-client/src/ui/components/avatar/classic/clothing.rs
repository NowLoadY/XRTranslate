//! Fitted courier cap and scarf. Coordinates are authored against the body, then made socket-local.
use super::super::{
    geometry::{Mesh, bezier, body, cushion, signed_power},
    model::Geometry,
    wardrobe::Socket,
};
use glam::{Vec2, Vec3};
use std::f32::consts::{PI, TAU};

// Each cap panel is a fitted shell patch between the curved overlap and the opening.
// The grid follows the seam itself, so a soft crease does not depend on dense triangle sampling.
fn cap_point(u: f32, v: f32, right: bool) -> Vec3 {
    let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let opening = |angle: f32| Vec2::new(1.04 * angle.cos() - 0.04, 0.91 * angle.sin() - 0.035);
    let start = 4.80;
    let end = 0.80;
    let seam = bezier(
        [
            opening(start),
            Vec2::new(0.22, -0.25),
            Vec2::new(0.04, 0.90),
            opening(end),
        ],
        v,
    );
    let edge = opening(start + (end + if right { TAU } else { 0.0 } - start) * v);
    let p = seam.lerp(edge, u);
    let radial = Vec2::new((p.x + 0.04) / 1.04, (p.y + 0.035) / 0.91);
    let radius = radial.length().min(1.0);
    let rim = 0.53 + 0.17 * radial.y.max(0.0);
    let top = 1.49 - 0.10 * radial.x;
    let dome = (1.0 - radius.powf(2.4)).max(0.0).sqrt();
    let envelope = (PI * v).sin().max(0.0).sqrt();
    let (crease, puff) = if right {
        (0.038, 0.040)
    } else {
        (0.115, 0.050)
    };
    let fold = envelope * (-crease * (-(u / 0.13).powi(2)).exp() + puff * (PI * u).sin());
    Vec3::new(p.x, rim + (top - rim) * dome + fold, p.y)
}

fn cap_inside(u: f32, v: f32, right: bool) -> Vec3 {
    let point = cap_point(u, v, right);
    let radius = Vec2::new((point.x + 0.04) / 1.04, (point.z + 0.035) / 0.91)
        .length()
        .min(1.0);
    let dome = (1.0 - radius * radius).sqrt();
    // The two panels share the same inner seam; the raised fold overlaps it without a slit.
    let overlap = if right {
        0.077 * (PI * v).sin().max(0.0).sqrt() * (-(u / 0.13).powi(2)).exp()
    } else {
        0.0
    };
    Vec3::new(
        (point.x + 0.04) * (1.0 - 0.07 * u) - 0.04,
        point.y + 0.025 - 0.105 * dome - overlap,
        (point.z + 0.035) * (1.0 - 0.07 * u) - 0.035,
    )
}

fn cap() -> Mesh {
    let mut mesh = Mesh::default();
    for right in [false, true] {
        mesh.surface(96, 64, right, |u, v| cap_point(u, v, right));
        mesh.surface(96, 64, !right, |u, v| cap_inside(u, v, right));
        for edge in [0.0_f32, 1.0] {
            mesh.surface(12, 96, right == (edge == 0.0), |u, v| {
                let rim = if edge == 1.0 {
                    -Vec3::Y * 0.055
                } else {
                    let tangent =
                        cap_point(edge, u + 0.001, right) - cap_point(edge, u - 0.001, right);
                    Vec3::new(-tangent.z, 0.0, tangent.x).normalize_or(Vec3::X)
                        * if right { 0.038 } else { -0.018 }
                        * (PI * u).sin().max(0.0).sqrt()
                };
                cap_point(edge, u, right)
                    .lerp(cap_inside(edge, u, right), (1.0 - (PI * v).cos()) * 0.5)
                    + rim * (PI * v).sin()
            });
        }
    }
    mesh
}

fn visor() -> Mesh {
    let outline = [
        [
            Vec2::new(-0.78, 0.50),
            Vec2::new(-0.60, 0.40),
            Vec2::new(0.41, 0.40),
            Vec2::new(0.63, 0.55),
        ],
        [
            Vec2::new(0.63, 0.55),
            Vec2::new(0.90, 0.88),
            Vec2::new(0.72, 1.20),
            Vec2::new(0.35, 1.23),
        ],
        [
            Vec2::new(0.35, 1.23),
            Vec2::new(-0.02, 1.27),
            Vec2::new(-0.56, 1.25),
            Vec2::new(-0.77, 1.09),
        ],
        [
            Vec2::new(-0.77, 1.09),
            Vec2::new(-0.94, 0.97),
            Vec2::new(-0.92, 0.70),
            Vec2::new(-0.78, 0.50),
        ],
    ];
    cushion(
        &outline,
        Vec2::new(-0.08, 0.83),
        0.043,
        |point, thickness| {
            let y = 0.70 + 0.11 * (1.0 - (point.x / 0.90).powi(2))
                - 0.095 * ((point.y - 0.52) / 0.80).powi(2);
            Vec3::new(point.x * 0.94, y + thickness, point.y)
        },
    )
}

fn scarf() -> Mesh {
    let mut mesh = Mesh::default();
    mesh.surface(40, 128, true, |u, v| {
        let angle = u * TAU;
        let cross = v * TAU;
        // Flattened cloth section, with the inside following the actual cheek profile.
        let y = -0.49 - 0.11 * angle.sin() + 0.185 * signed_power(cross.sin(), 0.5);
        let radius = body::radius(y) + 0.042 + 0.030 * signed_power(cross.cos(), 0.5);
        body::ring(radius, y, angle)
    });
    mesh
}

fn tail() -> Mesh {
    let outline = [
        [
            Vec2::new(-0.12, 0.0),
            Vec2::new(-0.06, -0.035),
            Vec2::new(0.07, -0.035),
            Vec2::new(0.12, 0.0),
        ],
        [
            Vec2::new(0.12, 0.0),
            Vec2::new(0.15, 0.20),
            Vec2::new(0.17, 0.62),
            Vec2::new(0.16, 0.83),
        ],
        [
            Vec2::new(0.16, 0.83),
            Vec2::new(0.15, 1.04),
            Vec2::new(-0.12, 1.07),
            Vec2::new(-0.145, 0.88),
        ],
        [
            Vec2::new(-0.145, 0.88),
            Vec2::new(-0.17, 0.64),
            Vec2::new(-0.13, 0.22),
            Vec2::new(-0.12, 0.0),
        ],
    ];
    cushion(&outline, Vec2::new(0.0, 0.48), 0.046, |point, thickness| {
        let t = point.y;
        let bend = bezier(
            [
                Vec2::new(-0.39, 0.87),
                Vec2::new(-0.54, 1.01),
                Vec2::new(-0.85, 0.97),
                Vec2::new(-0.97, 0.87),
            ],
            t,
        );
        Vec3::new(0.60 + 0.11 * t + point.x, bend.x, bend.y + thickness)
    })
}

pub fn mesh(geometry: Geometry) -> Mesh {
    let (mut mesh, socket) = match geometry {
        Geometry::Cap => (cap(), Socket::Hat),
        Geometry::Visor => (visor(), Socket::Hat),
        Geometry::Scarf => (scarf(), Socket::Scarf),
        Geometry::Tail => (tail(), Socket::Scarf),
        _ => unreachable!("not a classic accessory"),
    };
    for vertex in &mut mesh.vertices {
        vertex.position = (Vec3::from_array(vertex.position) - socket.origin()).to_array();
    }
    mesh
}
