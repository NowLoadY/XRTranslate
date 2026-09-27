use super::{
    Avatar, Material, Part,
    model::Geometry,
    wardrobe::{Attachment, Socket},
};
use eframe::egui::Color32;
use std::sync::LazyLock;
pub(super) mod clothing;

pub struct Classic;
impl Classic {
    pub fn model() -> &'static Avatar {
        static MODEL: LazyLock<Avatar> = LazyLock::new(|| {
            let clothes = |socket, geometries: [Geometry; 2], color| {
                Attachment::new(
                    socket,
                    geometries
                        .into_iter()
                        .map(|geometry| Part {
                            material: Material::FABRIC,
                            ..Part::new(geometry, color)
                        })
                        .collect(),
                )
            };
            Avatar {
                attachments: vec![
                    clothes(
                        Socket::Hat,
                        [Geometry::Cap, Geometry::Visor],
                        Color32::from_rgb(155, 166, 198),
                    ),
                    clothes(
                        Socket::Scarf,
                        [Geometry::Scarf, Geometry::Tail],
                        Color32::from_rgb(177, 193, 214),
                    ),
                ],
                ..Avatar::default()
            }
        });
        &MODEL
    }
}
