//! Independent features attach to a tangent frame on the fitted body surface.
use super::{
    HeadShape, Pose,
    model::{Geometry, Material, Model, Part},
};
use eframe::egui::{Color32, Vec2};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy)]
pub struct Eye {
    pub position: Vec2,
    pub thickness: f32,
    pub height: f32,
}
#[derive(Clone, Copy)]
pub struct Mouth {
    pub position: Vec2,
    pub width: f32,
    pub color: Color32,
}
#[derive(Clone, Copy)]
pub struct Nose {
    pub position: Vec2,
    pub size: f32,
}

fn feature(
    kind: Geometry,
    point: Vec2,
    scale: Vec3,
    body: Vec3,
    head: HeadShape,
    color: Color32,
) -> Part {
    Part {
        transform: Mat4::from_scale(body)
            * head.frame(glam::Vec2::new(point.x, -point.y))
            * Mat4::from_scale(scale),
        material: Material {
            shade_contrast: 0.12,
            softness: 0.40,
            outline_width: 0.0,
            ..Material::CLAY
        },
        ..Part::new(kind, color)
    }
}

impl Eye {
    fn append(self, body: Vec3, head: HeadShape, pose: Pose, color: Color32, model: &mut Model) {
        let side = self.position.x.signum();
        let thought = pose.thinking * if side < 0.0 { 1.0 } else { 0.25 };
        let mut part = feature(
            Geometry::Eye,
            self.position + Vec2::new(0.0, side * pose.curiosity * 0.014 - pose.concern * 0.015),
            Vec3::new(
                self.thickness / 0.146,
                self.height / 0.396 * (1.0 + pose.curiosity * 0.12 - thought * 0.15),
                1.0,
            ),
            body,
            head,
            color,
        );
        let blink = pose.blink.max(thought * 0.35);
        part.morph = [pose.joy * (1.0 - blink), blink];
        part.transform *=
            Mat4::from_rotation_z(side * (pose.concern * 0.14 - pose.curiosity * 0.04));
        model.add(part);
    }
}
impl Mouth {
    fn append(self, body: Vec3, head: HeadShape, pose: Pose, ink: Color32, model: &mut Model) {
        let open = pose.speech.clamp(0.0, 1.0);
        let mut part = feature(
            Geometry::Mouth,
            self.position + Vec2::new(pose.thinking * 0.025, 0.0),
            Vec3::new(self.width / 0.226 * (1.0 - pose.curiosity * 0.2), 1.0, 1.0),
            body,
            head,
            ink.lerp_to_gamma(self.color, open.sqrt()),
        );
        part.material = Material {
            shade_contrast: 0.12,
            outline_width: 0.0,
            softness: 0.40,
            ..Material::CLAY
        };
        part.morph = [
            (0.25 + pose.joy * 0.75 - pose.concern * 0.95 - pose.thinking * 0.2) * (1.0 - open),
            open,
        ];
        model.add(part);
    }
}
impl Nose {
    fn append(self, body: Vec3, head: HeadShape, color: Color32, model: &mut Model) {
        model.add(feature(
            Geometry::Body,
            self.position,
            Vec3::splat(self.size),
            body,
            head,
            color,
        ));
    }
}

#[derive(Clone, Copy)]
pub struct Face {
    pub eyes: [Eye; 2],
    pub mouth: Option<Mouth>,
    pub nose: Option<Nose>,
    pub ink: Color32,
}
impl Default for Face {
    fn default() -> Self {
        Self {
            eyes: [-0.424, 0.424].map(|x| Eye {
                position: Vec2::new(x, 0.257),
                thickness: 0.149,
                height: 0.348,
            }),
            mouth: Some(Mouth {
                position: Vec2::new(0.0, 0.493),
                width: 0.172,
                color: Color32::from_rgb(236, 148, 147),
            }),
            nose: None,
            ink: Color32::from_rgb(71, 59, 53),
        }
    }
}
impl Face {
    pub(super) fn append(self, body: Vec3, head: HeadShape, pose: Pose, model: &mut Model) {
        for eye in self.eyes {
            eye.append(body, head, pose, self.ink, model);
        }
        if let Some(nose) = self.nose {
            nose.append(body, head, self.ink, model);
        }
        if let Some(mouth) = self.mouth {
            mouth.append(body, head, pose, self.ink, model);
        }
    }
}
