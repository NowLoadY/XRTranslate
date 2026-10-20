//! Small clay props using the companion's mesh, cel lighting and contour passes.
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

// The classic companion's cream, mist-blue and rose accessories supply the
// palette. Colored contours come from Material::CLAY, never a black outer shell.
const CREAM: Color32 = Color32::from_rgb(231, 215, 184);
const MIST: Color32 = Color32::from_rgb(170, 185, 212);
const ROSE: Color32 = Color32::from_rgb(212, 173, 186);
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
    let accent = match button.kind {
        ButtonId::Send => MIST,
        ButtonId::Revise => ROSE,
    };
    let feedback = (button.feedback / 0.22).clamp(0.0, 1.0);
    let color = if button.enabled {
        mix(
            accent,
            PAPER,
            if feedback > 0.0 {
                feedback * 0.36
            } else if button.hover {
                0.10
            } else {
                0.0
            },
        )
    } else {
        Color32::from_gray(if button.hover { 178 } else { 166 })
    };

    // Two low, rounded cushions replace the old rectangular housing. The cap
    // remains within the interaction dimensions and visibly yields under touch.
    solid(
        model,
        button.pose * Mat4::from_translation(-Vec3::Z * button.half.z * 0.55),
        Vec3::new(
            button.half.x + 0.004,
            button.half.y + 0.004,
            button.half.z * 0.72,
        ),
        if button.enabled {
            CREAM
        } else {
            Color32::from_gray(142)
        },
    );
    let depth = if button.enabled {
        button.depth.clamp(0.0, 0.012)
    } else {
        0.0
    };
    let cap = button.pose * Mat4::from_translation(-Vec3::Z * depth);
    let half = Vec3::new(
        button.half.x,
        button.half.y,
        button.half.z * (1.0 - depth * 8.0),
    );
    solid(model, cap, half, color);
    let ink = if button.enabled {
        INK
    } else {
        Color32::from_gray(105)
    };
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
            outline_width: 0.0009,
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

fn mix(a: Color32, b: Color32, amount: f32) -> Color32 {
    let a = a.to_array();
    let b = b.to_array();
    let amount = amount.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * amount) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * amount) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * amount) as u8,
    )
}
