//! Independent features attach to a tangent frame on the fitted body surface.
use super::{
    Pose,
    geometry::body,
    model::{Geometry, Material, Model, Part},
};
use eframe::egui::{Color32, Vec2};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy)]
pub struct Eye {
    pub position: Vec2,
    pub thickness: f32,
}
#[derive(Clone, Copy)]
pub struct Mouth {
    pub position: Vec2,
    pub width: f32,
}
#[derive(Clone, Copy)]
pub struct Nose {
    pub position: Vec2,
    pub size: f32,
}

fn feature(kind: Geometry, point: Vec2, scale: Vec3, body: Vec3, color: Color32) -> Part {
    Part {
        transform: Mat4::from_scale(body)
            * body::frame(glam::Vec2::new(point.x, -point.y))
            * Mat4::from_scale(scale),
        material: Material {
            shade_contrast: 0.0,
            outline_width: 0.0,
        },
        ..Part::new(kind, color)
    }
}

impl Eye {
    fn append(self, body: Vec3, pose: Pose, color: Color32, model: &mut Model) {
        let mut part = feature(
            Geometry::Eye,
            self.position,
            Vec3::new(self.thickness / 0.146, 1.0 + pose.curiosity * 0.15, 1.0),
            body,
            color,
        );
        part.morph = [pose.joy * (1.0 - pose.blink), pose.blink];
        model.add(part);
    }
}
impl Mouth {
    fn append(self, body: Vec3, pose: Pose, color: Color32, model: &mut Model) {
        let mut part = feature(
            Geometry::Mouth,
            self.position,
            Vec3::new(self.width / 0.226, 1.0, 1.0),
            body,
            color,
        );
        let open = pose.curiosity.max(pose.speech.clamp(0.0, 1.0));
        part.morph = [pose.joy * (1.0 - open), open];
        model.add(part);
    }
}
impl Nose {
    fn append(self, body: Vec3, color: Color32, model: &mut Model) {
        model.add(feature(
            Geometry::Body,
            self.position,
            Vec3::splat(self.size),
            body,
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
            eyes: [-0.34, 0.34].map(|x| Eye {
                position: Vec2::new(x, -0.20),
                thickness: 0.158,
            }),
            mouth: Some(Mouth {
                position: Vec2::new(0.0, 0.02),
                width: 0.226,
            }),
            nose: None,
            ink: Color32::from_rgb(59, 60, 64),
        }
    }
}
impl Face {
    pub(super) fn append(self, body: Vec3, pose: Pose, model: &mut Model) {
        for eye in self.eyes {
            eye.append(body, pose, self.ink, model);
        }
        if let Some(nose) = self.nose {
            nose.append(body, self.ink, model);
        }
        if let Some(mouth) = self.mouth {
            mouth.append(body, pose, self.ink, model);
        }
    }
}
