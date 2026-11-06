//! Shared accessory catalog and persisted choices; renderers receive the same appearance.
use super::super::{Avatar, Classic, model::Model, render};
use super::{Attachment, Socket};
use eframe::egui::{Color32, Id, Painter, Pos2, Rect, Vec2};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    hat: Option<String>,
    scarf: Option<String>,
}

impl Appearance {
    pub fn selected(&self, socket: Socket) -> Option<&'static Accessory> {
        let id = match socket {
            Socket::Hat => &self.hat,
            Socket::Scarf => &self.scarf,
            Socket::Tie => return None,
        };
        // An unavailable item must not invalidate the rest of a settings file.
        Accessory::ALL
            .iter()
            .find(|item| item.socket == socket && Some(item.id) == id.as_deref())
    }

    pub fn select(&mut self, socket: Socket, item: Option<&Accessory>) {
        let choice = match socket {
            Socket::Hat => &mut self.hat,
            Socket::Scarf => &mut self.scarf,
            Socket::Tie => return,
        };
        *choice = item
            .filter(|item| item.socket == socket)
            .map(|item| item.id.to_owned());
    }

    pub fn model(&self) -> Avatar {
        let mut avatar = Classic::model().clone();
        avatar.attachments = Socket::AVAILABLE
            .into_iter()
            .filter_map(|socket| self.selected(socket))
            .map(Accessory::attachment)
            .collect();
        avatar
    }
}

pub struct Accessory {
    pub id: &'static str,
    pub name: &'static str,
    pub socket: Socket,
    color: Color32,
}

impl Accessory {
    pub const ALL: &'static [Self] = &[
        Self::new("cap-blue", "Mist blue", Socket::Hat, [170, 185, 212]),
        Self::new("cap-cream", "Cream", Socket::Hat, [231, 215, 184]),
        Self::new("cap-rose", "Rose", Socket::Hat, [212, 173, 186]),
        Self::new("scarf-blue", "Mist blue", Socket::Scarf, [143, 167, 198]),
        Self::new("scarf-cream", "Cream", Socket::Scarf, [220, 203, 177]),
        Self::new("scarf-rose", "Rose", Socket::Scarf, [205, 159, 177]),
    ];

    const fn new(id: &'static str, name: &'static str, socket: Socket, color: [u8; 3]) -> Self {
        Self {
            id,
            name,
            socket,
            color: Color32::from_rgb(color[0], color[1], color[2]),
        }
    }

    fn attachment(&self) -> Attachment {
        match self.socket {
            Socket::Hat => Classic::cap(self.color),
            Socket::Scarf => Classic::scarf(self.color),
            Socket::Tie => unreachable!("No tie is registered in the catalog"),
        }
    }

    /// Render the actual accessory mesh on its own, using the shared GPU passes.
    pub fn paint(&self, painter: &Painter, id: Id, center: Pos2, radius: f32, yaw: f32) {
        let mut model = Model::default();
        self.attachment().append(Vec3::ONE, &mut model);
        let pivot = match self.socket {
            Socket::Hat => Vec3::new(0.0, 1.0, 0.15),
            _ => Vec3::new(0.0, -0.95, 0.25),
        };
        let transform = Mat4::from_rotation_x(0.18)
            * Mat4::from_rotation_y(yaw)
            * Mat4::from_translation(-pivot);
        for part in &mut model.parts {
            part.transform = transform * part.transform;
        }
        render::paint(
            painter,
            id,
            Rect::from_center_size(center, Vec2::splat(radius * 4.4)),
            model,
        );
    }
}
