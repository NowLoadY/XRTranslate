//! Authored surfaces, tessellated once and shared by every avatar instance.
pub(super) mod body;
mod features;

use super::{classic::clothing, model::Geometry};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use std::{f32::consts::PI, ops::Range};

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub happy_position: [f32; 3],
    pub happy_normal: [f32; 3],
    pub other_position: [f32; 3],
    pub other_normal: [f32; 3],
}

#[derive(Default)]
pub(super) struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn append(&mut self, mesh: Self) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(mesh.vertices);
        self.indices
            .extend(mesh.indices.into_iter().map(|index| index + base));
    }

    /// Numerical derivatives preserve the normals of the authored surface, including its bevels.
    pub fn surface(
        &mut self,
        rows: u32,
        columns: u32,
        reverse: bool,
        point: impl Fn(f32, f32) -> Vec3,
    ) {
        let base = self.vertices.len() as u32;
        for row in 0..=rows {
            let v = row as f32 / rows as f32;
            for column in 0..=columns {
                let u = column as f32 / columns as f32;
                // Evaluate just inside collapsed pole rows to obtain a stable limiting normal.
                let nv = v.clamp(0.0001, 0.9999);
                let du = point(u + 0.0001, nv) - point(u - 0.0001, nv);
                let dv = point(u, nv + 0.00005) - point(u, nv - 0.00005);
                let normal = du.cross(dv).normalize_or(Vec3::Y) * if reverse { -1.0 } else { 1.0 };
                self.vertices.push(Vertex {
                    position: point(u, v).to_array(),
                    normal: normal.to_array(),
                    happy_position: [0.0; 3],
                    happy_normal: [0.0; 3],
                    other_position: [0.0; 3],
                    other_normal: [0.0; 3],
                });
            }
        }
        // Collapsed pole rows share one limiting normal. This also keeps expanded toon
        // contours from opening tiny spikes where all the surface meridians meet.
        for row in [0, rows] {
            let start = (base + row * (columns + 1)) as usize;
            let ring = &mut self.vertices[start..start + columns as usize + 1];
            let pole = Vec3::from_array(ring[0].position);
            if ring
                .iter()
                .all(|vertex| Vec3::from_array(vertex.position).distance_squared(pole) < 1e-10)
            {
                let normal = ring
                    .iter()
                    .map(|vertex| Vec3::from_array(vertex.normal))
                    .sum::<Vec3>()
                    .normalize_or(Vec3::Y)
                    .to_array();
                for vertex in ring {
                    vertex.normal = normal;
                }
            }
        }
        for row in 0..rows {
            for col in 0..columns {
                let a = base + row * (columns + 1) + col;
                let b = a + columns + 1;
                self.indices.extend(if reverse {
                    [a, b, a + 1, a + 1, b, b + 1]
                } else {
                    [a, a + 1, b, a + 1, b + 1, b]
                });
            }
        }
    }

    pub fn morph(mut self, happy: Self, other: Self) -> Self {
        for ((base, happy), other) in self
            .vertices
            .iter_mut()
            .zip(happy.vertices)
            .zip(other.vertices)
        {
            base.happy_position =
                (Vec3::from_array(happy.position) - Vec3::from_array(base.position)).to_array();
            base.happy_normal =
                (Vec3::from_array(happy.normal) - Vec3::from_array(base.normal)).to_array();
            base.other_position =
                (Vec3::from_array(other.position) - Vec3::from_array(base.position)).to_array();
            base.other_normal =
                (Vec3::from_array(other.normal) - Vec3::from_array(base.normal)).to_array();
        }
        self
    }
}

pub(super) fn bezier(points: [Vec2; 4], t: f32) -> Vec2 {
    let s = 1.0 - t;
    points[0] * s.powi(3)
        + points[1] * (3.0 * s * s * t)
        + points[2] * (3.0 * s * t * t)
        + points[3] * t.powi(3)
}

pub(super) fn curve(segments: &[[Vec2; 4]], t: f32) -> Vec2 {
    let step = t.clamp(0.0, 1.0) * segments.len() as f32;
    let index = (step as usize).min(segments.len() - 1);
    bezier(segments[index], step - index as f32)
}

pub(super) fn signed_power(value: f32, power: f32) -> f32 {
    value.signum() * value.abs().powf(power)
}

/// A closed, beveled cloth solid with a Bézier outline and a separately authored bend.
pub(super) fn cushion(
    outline: &[[Vec2; 4]],
    center: Vec2,
    thickness: f32,
    place: impl Fn(Vec2, f32) -> Vec3,
) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.surface(48, 96, false, |u, v| {
        let angle = v * PI;
        let radius = angle.sin();
        let point = center.lerp(curve(outline, u.rem_euclid(1.0)), radius);
        // Broad faces and a small rolled edge, rather than an ellipsoidal cross section.
        let edge = ((radius - 0.82) / 0.18).max(0.0);
        let height = (1.0 - edge * edge).max(0.0).sqrt() * angle.cos().signum() * thickness;
        place(point, height)
    });
    mesh
}

pub(super) fn atlas() -> (Vec<Vertex>, Vec<u32>, Vec<Range<u32>>) {
    let mut atlas = Mesh::default();
    let mut ranges = Vec::new();
    for geometry in Geometry::ALL {
        let mesh = match geometry {
            Geometry::Body => body::mesh(),
            Geometry::Eye => features::eye(),
            Geometry::Mouth => features::mouth(),
            _ => clothing::mesh(geometry),
        };
        let start = atlas.indices.len() as u32;
        atlas.append(mesh);
        ranges.push(start..atlas.indices.len() as u32);
    }
    (atlas.vertices, atlas.indices, ranges)
}
