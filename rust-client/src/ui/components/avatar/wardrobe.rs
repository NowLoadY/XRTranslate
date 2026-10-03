//! Local 3D sockets: accessories are composed here, then inherit the avatar's pose.
mod catalog;
use super::model::{Model, Part};
pub use catalog::{Accessory, Appearance};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Socket {
    Hat,
    Scarf,
    #[expect(dead_code, reason = "Reserved clothing socket for future accessories")]
    Tie,
}

impl Socket {
    pub const AVAILABLE: [Self; 2] = [Self::Hat, Self::Scarf];

    /// Screen-facing fitting handles sit on the outside of their attachment area.
    pub(super) fn marker(self) -> Vec3 {
        match self {
            Self::Hat => Vec3::new(0.0, 0.60, 0.0),
            Self::Scarf => Vec3::new(0.0, -0.20, 1.05),
            Self::Tie => Vec3::ZERO,
        }
    }

    pub fn origin(self) -> Vec3 {
        match self {
            Self::Hat => Vec3::new(0.0, 0.65, 0.0),
            Self::Scarf => Vec3::new(0.0, -0.70, 0.0),
            Self::Tie => Vec3::new(0.0, -0.55, 0.93),
        }
    }

    pub fn transform(self, body: Vec3) -> Mat4 {
        Mat4::from_scale(body) * Mat4::from_translation(self.origin())
    }
}

#[derive(Clone)]
pub struct Attachment {
    pub socket: Socket,
    pub transform: Mat4,
    pub parts: Vec<Part>,
}

impl Attachment {
    pub(super) fn append(&self, body: Vec3, model: &mut Model) {
        let parent = self.socket.transform(body) * self.transform;
        model
            .parts
            .extend(self.parts.iter().cloned().map(|mut part| {
                part.transform = parent * part.transform;
                part
            }));
    }
}
