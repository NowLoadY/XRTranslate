//! Minimal wrist buttons: a thin ring and a translucent press surface.
use super::interaction::ButtonId;
use crate::ui::components::avatar::{Geometry, Material, Model, Part};
use eframe::egui::Color32;
use glam::{Mat4, Quat, Vec3};

pub(super) struct ButtonVisual {
    pub kind: ButtonId,
    pub pose: Mat4,
    pub half: Vec3,
    pub enabled: bool,
    pub hover: bool,
    pub depth: f32,
    pub feedback: f32,
}

const MIST: Color32 = Color32::from_rgb(170, 185, 212);
const PAPER: Color32 = Color32::from_rgb(252, 242, 225);
const INK: Color32 = Color32::from_rgb(92, 79, 72);

pub(super) fn render(buttons: &[ButtonVisual], editing: bool) -> Model {
    let mut model = Model::default();
    for button in buttons {
        button_model(&mut model, button, editing);
    }
    model
}

fn button_model(model: &mut Model, button: &ButtonVisual, editing: bool) {
    let editing = editing && matches!(button.kind, ButtonId::Revise);
    let pressed = button.depth > 0.001 || button.feedback > 0.0;
    let accent = if editing {
        Color32::from_rgb(94, 225, 210)
    } else {
        MIST
    };
    let rim = if editing || pressed {
        PAPER
    } else if button.enabled {
        accent
    } else {
        Color32::from_gray(140)
    };
    let radius = button.half.x;
    for index in 0..48 {
        let angle = |i: usize| i as f32 / 48.0 * std::f32::consts::TAU;
        let point = |a: f32| Vec3::new(a.cos() * radius, a.sin() * radius, 0.0);
        line(
            model,
            button.pose,
            point(angle(index)),
            point(angle(index + 1)),
            0.0011,
            rim,
        );
    }
    let alpha = if pressed {
        255
    } else if editing {
        220
    } else if button.hover && button.enabled {
        180
    } else {
        110
    };
    let rgb = if editing {
        accent
    } else if pressed {
        PAPER
    } else {
        Color32::from_rgb(192, 210, 228)
    };
    let color = Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), alpha);
    let depth = if button.enabled {
        button.depth.min(0.008)
    } else {
        0.0
    };
    let cap = button.pose * Mat4::from_translation(Vec3::Z * (0.010 - depth));
    let half = Vec3::new(radius * 0.85, radius * 0.85, 0.0015);
    solid(model, cap, half, color);
    let ink = if editing || pressed { INK } else { PAPER };
    let glyph = |model: &mut Model, a: [f32; 2], b: [f32; 2]| {
        let point = |t: f32| {
            let x = (a[0] + (b[0] - a[0]) * t) * half.x;
            let y = (a[1] + (b[1] - a[1]) * t) * half.y;
            Vec3::new(x, y, dome(half, x, y) + 0.0015)
        };
        for step in 0..4 {
            line(
                model,
                cap,
                point(step as f32 / 4.0),
                point((step + 1) as f32 / 4.0),
                0.002,
                ink,
            );
        }
    };
    match button.kind {
        ButtonId::Send => {
            glyph(model, [-0.43, 0.39], [0.43, 0.0]);
            glyph(model, [-0.43, -0.39], [0.43, 0.0]);
            glyph(model, [-0.12, 0.0], [0.43, 0.0]);
        }
        ButtonId::Revise if editing => {
            glyph(model, [-0.39, 0.0], [-0.11, -0.37]);
            glyph(model, [-0.11, -0.37], [0.40, 0.43]);
        }
        ButtonId::Revise => {
            // A simple pencil distinguishes editing from refresh/retry.
            glyph(model, [-0.27, -0.33], [0.27, 0.41]);
            glyph(model, [-0.15, -0.43], [0.39, 0.31]);
            glyph(model, [0.27, 0.41], [0.39, 0.31]);
            glyph(model, [-0.27, -0.33], [-0.28, -0.49]);
            glyph(model, [-0.28, -0.49], [-0.15, -0.43]);
        }
    }
}

/// Height on an ellipsoid: raised ink follows the cap instead of floating above
/// its curved edges.
fn dome(half: Vec3, x: f32, y: f32) -> f32 {
    half.z
        * (1.0 - (x / half.x).powi(2) - (y / half.y).powi(2))
            .max(0.0)
            .sqrt()
}

fn solid(model: &mut Model, pose: Mat4, half: Vec3, color: Color32) {
    model.add(Part {
        transform: pose * Mat4::from_scale(half),
        material: Material {
            outline_width: 0.0,
            softness: 0.24,
            ..Material::CLAY
        },
        ..Part::new(Geometry::SolidSphere, color)
    });
}

fn line(model: &mut Model, root: Mat4, start: Vec3, end: Vec3, width: f32, color: Color32) {
    let offset = end - start;
    if offset.length_squared() < 1e-10 {
        return;
    }
    model.add(Part {
        transform: root
            * Mat4::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, offset.normalize()),
                (start + end) * 0.5,
            )
            * Mat4::from_scale(Vec3::new(width, offset.length() * 0.5 + width, width)),
        material: Material {
            outline_width: 0.0002,
            softness: 0.24,
            ..Material::CLAY
        },
        ..Part::new(Geometry::SolidSphere, color)
    });
}
