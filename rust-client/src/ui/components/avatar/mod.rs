//! Native 3D characters painted into the caller's layer, with shared geometry and facial animation.
mod classic;
mod companion;
mod face;
mod geometry;
mod model;
mod motion;
mod render;
mod speech;
pub mod wardrobe;

pub use classic::Classic;
pub use companion::companion;
pub use face::{Face, Nose};
pub use model::{Material, Part};
pub use motion::{Expression, Gaze, Pose};
#[cfg(debug_assertions)]
pub(crate) use render::export::render_views;
pub use render::install;
pub(crate) use render::stereo::StereoRenderer;

/// A presentation of the existing companion, never a second dialogue owner.
#[derive(Clone, Default)]
pub(crate) struct Presentation {
    pub pose: Pose,
    pub speech: Speech,
    pub clock: f64,
    pub visible: bool,
}
pub use speech::Speech;

use eframe::egui::{self, Color32, Id, Pos2, Rect, Vec2};
use glam::{EulerRot, Mat4, Quat, Vec3};
use model::{Geometry, Model};
use wardrobe::Attachment;

#[derive(Clone)]
pub struct Avatar {
    pub aspect: f32,
    pub color: Color32,
    pub face: Face,
    pub attachments: Vec<Attachment>,
}

impl Default for Avatar {
    fn default() -> Self {
        Self {
            aspect: 1.0,
            color: Color32::from_gray(246),
            face: Face::default(),
            attachments: Vec::new(),
        }
    }
}

impl Avatar {
    fn assemble(&self, pose: Pose) -> Model {
        let body = Vec3::new(self.aspect, 1.0, 0.92);
        let mut model = Model::default();
        model.add(Part {
            transform: Mat4::from_scale(body),
            ..Part::new(Geometry::Body, self.color)
        });
        self.face.append(body, pose, &mut model);
        for attachment in &self.attachments {
            attachment.append(body, &mut model);
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

    pub fn paint(&self, painter: &egui::Painter, id: Id, center: Pos2, radius: f32, pose: Pose) {
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
