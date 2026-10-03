//! Local 3D sockets: accessories are composed here, then inherit the avatar's pose.
use super::model::{Model, Part};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    Hat,
    Scarf,
    #[expect(dead_code, reason = "Reserved clothing socket for future accessories")]
    Tie,
}

impl Socket {
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
