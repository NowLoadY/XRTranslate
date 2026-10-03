use super::{
    Avatar, Material, Part,
    model::Geometry,
    wardrobe::{Attachment, Socket},
};
use eframe::egui::Color32;
use glam::{Mat4, Vec3};
use std::sync::LazyLock;
pub(super) mod clothing;

pub struct Classic;
impl Classic {
    pub fn model() -> &'static Avatar {
        static MODEL: LazyLock<Avatar> = LazyLock::new(|| Avatar {
            color: Color32::from_rgb(252, 232, 209),
            ..Avatar::default()
        });
        &MODEL
    }

    pub fn cap(color: Color32) -> Attachment {
        Attachment {
            socket: Socket::Hat,
            transform: Mat4::from_translation(Vec3::Y * 0.11)
                * Mat4::from_scale(Vec3::new(1.06, 1.0, 1.16)),
            parts: [Geometry::Cap, Geometry::Visor]
                .map(|geometry| garment(geometry, color))
                .into(),
        }
    }

    pub fn scarf(color: Color32) -> Attachment {
        Attachment {
            socket: Socket::Scarf,
            transform: Mat4::IDENTITY,
            parts: [Geometry::Scarf, Geometry::Tail]
                .map(|geometry| garment(geometry, color))
                .into(),
        }
    }
}

fn garment(geometry: Geometry, color: Color32) -> Part {
    Part {
        material: Material {
            shade_contrast: 0.32,
            softness: 0.24,
            ..Material::CLAY
        },
        ..Part::new(geometry, color)
    }
}
