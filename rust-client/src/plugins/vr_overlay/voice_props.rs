//! Small clay props using the companion's mesh, cel lighting and contour passes.
use super::interaction::ButtonId;
use crate::{
    ui::components::avatar::{Geometry, Material, Model, Part},
    voice_interaction::VoiceSnapshot,
};
use eframe::egui::Color32;
use glam::{Mat4, Quat, Vec3};

pub(super) const MICROPHONE_HEAD: Vec3 = Vec3::new(0.0, 0.115, 0.0);

pub(super) struct ButtonVisual {
    pub kind: ButtonId,
    pub pose: Mat4,
    pub half: Vec3,
    pub enabled: bool,
    pub held: bool,
    pub hover: bool,
    pub depth: f32,
    pub feedback: f32,
}

// The classic companion's cream, mist-blue and rose accessories supply the
// palette. Colored contours come from Material::CLAY, never a black outer shell.
const CREAM: Color32 = Color32::from_rgb(231, 215, 184);
const MIST: Color32 = Color32::from_rgb(170, 185, 212);
const BLUE: Color32 = Color32::from_rgb(143, 167, 198);
const ROSE: Color32 = Color32::from_rgb(212, 173, 186);
const PAPER: Color32 = Color32::from_rgb(252, 242, 225);
const INK: Color32 = Color32::from_rgb(92, 79, 72);

pub(super) fn render(
    dock: Mat4,
    microphone: Mat4,
    held: bool,
    snapshot: &VoiceSnapshot,
    buttons: &[ButtonVisual],
) -> Model {
    let mut model = Model::default();
    if !held {
        let cushion = dock * Mat4::from_translation(-Vec3::Y * 0.105);
        solid(&mut model, cushion, Vec3::new(0.049, 0.012, 0.041), CREAM);
        solid(
            &mut model,
            cushion * Mat4::from_translation(Vec3::Y * 0.006),
            Vec3::new(0.037, 0.009, 0.031),
            MIST,
        );
    }

    // A soft tapered grip and a slightly oversized head read as a microphone
    // without adding a face, metal cage or tiny decorative controls.
    solid(
        &mut model,
        microphone,
        Vec3::new(0.023, 0.089, 0.022),
        CREAM,
    );
    let head = microphone * Mat4::from_translation(MICROPHONE_HEAD);
    let head_half = Vec3::new(0.038, 0.048, 0.036);
    solid(&mut model, head, head_half, MIST);
    let seam = mix(BLUE, INK, 0.22);
    for side in [-1.0, 1.0] {
        for y in [-0.016, 0.0, 0.016] {
            // Three broad shallow grille grooves follow the head's real curved
            // surface; both faces stay legible when the held prop rotates.
            for step in 0..8 {
                let point = |step: usize| {
                    let x = (-0.66 + step as f32 * 0.165) * head_half.x;
                    let z = dome(head_half, x, y) + 0.0006;
                    Vec3::new(x, y, z * side)
                };
                line(&mut model, head, point(step), point(step + 1), 0.0012, seam);
            }
        }
    }
    let collar = if snapshot.instruction {
        ROSE
    } else if snapshot.recording {
        BLUE
    } else if held {
        MIST
    } else {
        mix(CREAM, PAPER, 0.35)
    };
    solid(
        &mut model,
        microphone * Mat4::from_translation(Vec3::Y * 0.059),
        Vec3::new(0.025, if snapshot.recording { 0.007 } else { 0.005 }, 0.024),
        collar,
    );

    if snapshot.review_enabled {
        for button in buttons {
            button_model(&mut model, button, snapshot.revise_cancels);
        }
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
            } else if button.held {
                0.16
            } else if button.hover {
                0.10
            } else {
                0.0
            },
        )
    } else {
        Color32::from_gray(if button.held { 184 } else { 166 })
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
/// its curved edges. The same surface construction is used for grille grooves.
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
