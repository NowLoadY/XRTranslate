//! Fitted short hair and its secondary motion, independent of clothing and expressions.
pub(super) mod geometry;
pub(super) mod motion;

use super::{
    Material, Part, Pose,
    model::{Geometry, Model},
};
use eframe::egui::Color32;
use glam::{Mat4, Vec3};

#[derive(Clone, Copy)]
pub struct Hair {
    pub color: Color32,
}

impl Default for Hair {
    fn default() -> Self {
        Self {
            color: Color32::from_rgb(248, 228, 186),
        }
    }
}

impl Hair {
    pub(super) fn append(self, body: Vec3, pose: Pose, model: &mut Model) {
        model.add(Part {
            transform: Mat4::from_scale(body),
            morph: pose.hair,
            material: Material {
                shade_contrast: 0.26,
                outline_width: 0.004,
                softness: 0.38,
                warmth: 1.0,
                ..Material::CLAY
            },
            ..Part::new(Geometry::Hair, self.color)
        });
    }
}
