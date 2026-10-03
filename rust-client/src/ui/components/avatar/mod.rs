//! Native 3D characters painted into the caller's layer, with shared geometry and facial animation.
mod classic;
mod companion;
mod face;
mod geometry;
mod hair;
mod model;
mod motion;
mod presentation;
mod render;
mod speech;
#[cfg(any(windows, target_os = "linux"))]
mod surface;
pub mod wardrobe;

pub use classic::Classic;
pub use companion::companion;
pub use face::{Face, Nose};
pub use geometry::body::HeadShape;
pub use hair::Hair;
pub use model::{Material, Part};
pub(crate) use motion::Motion;
pub use motion::{Expression, Gaze, Pose};
pub use presentation::Presentation;
pub(crate) use presentation::PresentationSampler;
#[cfg(debug_assertions)]
pub(crate) use render::export::render_views;
pub use render::install;
pub(crate) use render::stereo::StereoRenderer;
pub use wardrobe::{Accessory, Appearance};

pub use speech::Speech;
#[cfg(any(windows, target_os = "linux"))]
pub(crate) use surface::Surface;

use eframe::egui::{self, Color32, Id, Pos2, Rect, Vec2};
use glam::{Mat4, Vec3};
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
    fn attachment_body(&self, socket: wardrobe::Socket) -> Vec3 {
        let width = match socket {
            wardrobe::Socket::Hat => self.head.envelope_width(),
            _ => self.head.width_at(socket.origin().y),
        };
        Vec3::new(self.aspect * width, 1.0, 1.0)
    }

    pub fn socket_position(&self, socket: wardrobe::Socket, pose: Pose) -> Vec3 {
        (pose.rotation() * socket.transform(self.attachment_body(socket)))
            .transform_point3(socket.marker())
    }

    fn assemble(&self, pose: Pose) -> Model {
        let body = Vec3::new(self.aspect, 1.0, 1.0);
        let mut model = Model::default();
        model.add(Part {
            transform: Mat4::from_scale(body),
            morph: self.head.weights(),
            material: Material {
                shade_contrast: 0.13,
                outline_width: 0.006,
                softness: 0.40,
                blush: 0.42 + 0.22 * pose.joy,
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
            attachment.append(self.attachment_body(attachment.socket), &mut model);
        }
        let rotation = pose.rotation();
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
