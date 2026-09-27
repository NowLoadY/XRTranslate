//! Quiet monochrome companions sharing one face rig.
use super::{Avatar, Nose};
use eframe::egui::Vec2;

pub fn companion(name: &str) -> Avatar {
    let variant = name
        .bytes()
        .fold(0usize, |sum, byte| sum.wrapping_add(byte as usize))
        % 3;
    let mut avatar = Avatar {
        aspect: [0.86, 1.0, 1.16][variant],
        ..Avatar::default()
    };
    if variant == 2 {
        avatar.face.nose = Some(Nose {
            position: Vec2::new(0.0, 0.12),
            size: 0.04,
        });
    }
    avatar
}
