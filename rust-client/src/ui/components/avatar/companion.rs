//! Card companions share the classic character's appearance and face rig.
use super::{Avatar, Classic, Nose};
use eframe::egui::Vec2;

pub fn companion(name: &str) -> Avatar {
    let variant = name
        .bytes()
        .fold(0usize, |sum, byte| sum.wrapping_add(byte as usize))
        % 3;
    let mut avatar = Avatar {
        aspect: [0.86, 1.0, 1.16][variant],
        ..Classic::model().clone()
    };
    if variant == 2 {
        avatar.face.nose = Some(Nose {
            position: Vec2::new(0.0, 0.12),
            size: 0.04,
        });
    }
    avatar
}
