use crate::ui::animation::AnimationSystem;
use eframe::egui::{self, Id, Vec2};
use std::{f32::consts::PI, time::Duration};

#[derive(Clone, Copy, Default)]
pub enum Expression {
    #[default]
    Calm,
    Happy,
    Curious,
}

#[derive(Clone, Copy, Default)]
pub struct Gaze {
    pub yaw: f32,
    pub pitch: f32,
}

impl Gaze {
    pub fn toward(offset: Vec2, depth: f32) -> Self {
        Self {
            yaw: offset.x.atan2(depth.max(1.0)).clamp(-0.65, 0.65),
            pitch: offset.y.atan2(depth.max(1.0)).clamp(-0.45, 0.45),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Pose {
    pub gaze: Gaze,
    pub roll: f32,
    /// Independent mouth opening, from silent (0) to fully open (1).
    pub speech: f32,
    pub(super) joy: f32,
    pub(super) curiosity: f32,
    pub(super) blink: f32,
}

fn blink_at(phase: f64) -> f32 {
    if phase < 0.2 {
        (phase as f32 / 0.2 * PI).sin().powi(2)
    } else {
        0.0
    }
}

impl Pose {
    pub(crate) fn idle_at(clock: f64, expression: Expression) -> Self {
        Self {
            blink: blink_at(clock.rem_euclid(4.8)),
            joy: if matches!(expression, Expression::Happy) {
                1.0
            } else {
                0.0
            },
            curiosity: if matches!(expression, Expression::Curious) {
                1.0
            } else {
                0.0
            },
            ..Self::default()
        }
    }

    pub fn animated(ctx: &egui::Context, id: Id, expression: Expression, gaze: Gaze) -> Self {
        let animate = |key, value| AnimationSystem::animate_value(ctx, id.with(key), value, 0.22);
        let phase = (ctx.input(|input| input.time) + (id.value() % 997) as f64 / 211.0) % 4.8;
        let blink = blink_at(phase);
        ctx.request_repaint_after(if phase < 0.2 {
            Duration::from_millis(16)
        } else {
            Duration::from_secs_f64(4.8 - phase)
        });
        Self {
            gaze: Gaze {
                yaw: animate("yaw", gaze.yaw.clamp(-0.65, 0.65)),
                pitch: animate("pitch", gaze.pitch.clamp(-0.45, 0.45)),
            },
            joy: animate(
                "joy",
                if matches!(expression, Expression::Happy) {
                    1.0
                } else {
                    0.0
                },
            ),
            curiosity: animate(
                "curiosity",
                if matches!(expression, Expression::Curious) {
                    1.0
                } else {
                    0.0
                },
            ),
            blink,
            ..Self::default()
        }
    }
}
