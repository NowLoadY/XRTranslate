//! Native 3D characters painted into the caller's layer, with shared geometry and facial animation.
mod classic;
mod companion;
mod face;
mod geometry;
mod hair;
mod model;
mod motion;
mod render;
mod speech;
mod surface;
pub mod wardrobe;

pub use classic::Classic;
pub use companion::companion;
pub use face::{Face, Nose};
pub use geometry::body::HeadShape;
pub use hair::Hair;
pub use model::{Material, Part};
pub use motion::{Expression, Gaze, Pose};
#[cfg(debug_assertions)]
pub(crate) use render::export::render_views;
pub use render::install;
pub(crate) use render::stereo::StereoRenderer;

/// A presentation of the existing companion, never a second dialogue owner.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Presentation {
    pub pose: Pose,
    pub speech: Speech,
    pub clock: f64,
    pub visible: bool,
}
pub use speech::Speech;
pub(crate) use surface::Surface;

use eframe::egui::{self, Color32, Id, Pos2, Rect, Vec2};
use glam::{EulerRot, Mat4, Quat, Vec3};
use model::{Geometry, Model};
use wardrobe::Attachment;

#[derive(Clone)]
pub struct Avatar {
    pub aspect: f32,
    pub head: HeadShape,
    pub color: Color32,
    pub face: Face,
    pub hair: Option<Hair>,
    pub attachments: Vec<Attachment>,
}

impl Default for Avatar {
    fn default() -> Self {
        Self {
            aspect: 1.0,
            head: HeadShape::default(),
            color: Color32::from_gray(246),
            face: Face::default(),
            hair: Some(Hair::default()),
            attachments: Vec::new(),
        }
    }
}

impl Avatar {
    fn assemble(&self, pose: Pose) -> Model {
        let body = Vec3::new(self.aspect, 1.0, 1.0);
        let mut model = Model::default();
        model.add(Part {
            transform: Mat4::from_scale(body),
            morph: self.head.weights(),
            material: Material {
                shade_contrast: 0.16,
                outline_width: 0.006,
                softness: 0.40,
                blush: 0.75,
                ..Material::CLAY
            },
            ..Part::new(Geometry::Body, self.color)
        });
        self.face.append(body, self.head, pose, &mut model);
        let hair_width = self.head.envelope_width();
        if let Some(hair) = self.hair {
            hair.append(body * Vec3::new(hair_width, 1.0, 1.0), pose, &mut model);
        }
        for attachment in &self.attachments {
            let width = match attachment.socket {
                wardrobe::Socket::Hat => hair_width,
                socket => self.head.width_at(socket.origin().y),
            };
            attachment.append(body * Vec3::new(width, 1.0, 1.0), &mut model);
        }
        let rotation = Mat4::from_quat(Quat::from_euler(
            EulerRot::YXZ,
            pose.gaze.yaw,
            pose.gaze.pitch,
            -pose.roll,
        ));
        for part in &mut model.parts {
            part.transform = rotation * part.transform;
        }
        model
    }

    pub fn paint(
        &self,
        painter: &egui::Painter,
        id: Id,
        center: Pos2,
        radius: f32,
        mut pose: Pose,
    ) {
        if self.hair.is_some() {
            pose.hair = hair::motion::paint(painter.ctx(), id, center, radius, pose);
        }
        render::paint(
            painter,
            id,
            Rect::from_center_size(center, Vec2::splat(radius * 4.4)),
            self.assemble(pose),
        );
    }
}

pub fn visible(ui: &egui::Ui, center: Pos2, radius: f32) -> bool {
    ui.is_rect_visible(Rect::from_center_size(center, Vec2::splat(radius * 3.0)))
}
