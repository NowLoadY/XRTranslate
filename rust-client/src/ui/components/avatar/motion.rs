//! Semantic expressions and one reusable clock-driven facial animator for every surface.
use eframe::egui::{self, Id, Vec2};
use std::{f32::consts::PI, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Expression {
    #[default]
    Calm,
    Happy,
    Curious,
    Thinking,
    Concerned,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pose {
    pub gaze: Gaze,
    pub roll: f32,
    /// Independent mouth opening, from silent (0) to fully open (1).
    pub speech: f32,
    /// Root-pinned hair bending, shared by all renderers.
    #[serde(default)]
    pub(super) hair: [f32; 2],
    pub(super) joy: f32,
    pub(super) curiosity: f32,
    pub(super) blink: f32,
    #[serde(default)]
    pub(super) thinking: f32,
    #[serde(default)]
    pub(super) concern: f32,
}

/// Irregular intervals, an occasional double blink, and a slower opening lid.
/// The returned delay lets resting cards sleep until the next actual change.
fn blink_at(clock: f64) -> (f32, f64) {
    const STARTS: [f64; 6] = [2.4, 6.7, 7.0, 12.6, 18.2, 24.7];
    let phase = clock.rem_euclid(29.0);
    for start in STARTS {
        let age = phase - start;
        if age < 0.0 {
            return (0.0, -age);
        }
        if age < 0.21 {
            let t = if age < 0.075 {
                age / 0.075
            } else {
                (0.21 - age) / 0.135
            };
            return (((t as f32).clamp(0.0, 1.0) * PI * 0.5).sin().powi(2), 0.016);
        }
    }
    (0.0, 29.0 - phase + STARTS[0])
}

impl Pose {
    pub(super) fn rotation(self) -> glam::Mat4 {
        glam::Mat4::from_quat(glam::Quat::from_euler(
            glam::EulerRot::YXZ,
            self.gaze.yaw,
            self.gaze.pitch,
            -self.roll,
        ))
    }

    pub(crate) fn idle_at(clock: f64, expression: Expression) -> Self {
        Self {
            blink: blink_at(clock).0,
            joy: f32::from(expression == Expression::Happy),
            curiosity: f32::from(expression == Expression::Curious),
            thinking: f32::from(expression == Expression::Thinking),
            concern: f32::from(expression == Expression::Concerned),
            ..Self::default()
        }
    }

    pub fn animated(ctx: &egui::Context, id: Id, expression: Expression, gaze: Gaze) -> Self {
        Self::interactive(ctx, id, expression, gaze, None)
    }

    pub fn interactive(
        ctx: &egui::Context,
        id: Id,
        expression: Expression,
        gaze: Gaze,
        response: Option<&egui::Response>,
    ) -> Self {
        let clock = ctx.input(|input| input.time) + (id.value() % 997) as f64 / 211.0;
        let interaction = response.map(|response| (response.hovered(), response.clicked()));
        let (pose, delay) = ctx.data_mut(|data| {
            let motion = data.get_temp_mut_or_default::<Motion>(id.with("face_motion"));
            let expression = interaction
                .and_then(|(hovered, clicked)| motion.interact(clock, hovered, clicked))
                .unwrap_or(expression);
            let pose = motion.advance(clock, expression, gaze, 0.0);
            (pose, motion.repaint_after(clock))
        });
        ctx.request_repaint_after(delay);
        pose
    }
}

/// Owned by the companion, not by its desktop/VR renderers. Cards use the same rig.
#[derive(Clone, Default)]
pub(crate) struct Motion {
    pose: Pose,
    previous_clock: Option<f64>,
    expression: Expression,
    changed_at: f64,
    moving: bool,
    clicked_until: f64,
}

impl Motion {
    pub(crate) fn interact(
        &mut self,
        clock: f64,
        hovered: bool,
        clicked: bool,
    ) -> Option<Expression> {
        if clicked {
            self.clicked_until = clock + 1.2;
        }
        if clock < self.clicked_until {
            Some(Expression::Happy)
        } else {
            hovered.then_some(Expression::Curious)
        }
    }

    pub(crate) fn repaint_after(&self, clock: f64) -> Duration {
        Duration::from_secs_f64(if self.moving {
            0.016
        } else {
            blink_at(clock).1
        })
    }

    pub(crate) fn advance(
        &mut self,
        clock: f64,
        expression: Expression,
        gaze: Gaze,
        speech: f32,
    ) -> Pose {
        let dt = self
            .previous_clock
            .replace(clock)
            .map_or(0.0, |last| (clock - last).clamp(0.0, 0.08)) as f32;
        if self.expression != expression {
            self.expression = expression;
            self.changed_at = clock;
        }
        let mut target = Pose::idle_at(clock, expression);
        // Delight opens into a relaxed smile instead of holding squinted eyes indefinitely.
        let delight = (1.0 - (clock - self.changed_at) as f32 / 1.5).clamp(0.0, 1.0);
        target.joy *= 0.32 + 0.68 * delight * delight;
        let mut moving = target.joy > 0.32;
        let mut follow = |value: &mut f32, target: f32, duration: f32| {
            *value += (target - *value) * (1.0 - (-dt / duration).exp());
            if (*value - target).abs() < 0.001 {
                *value = target;
            } else {
                moving = true;
            }
        };
        follow(&mut self.pose.joy, target.joy, 0.16);
        follow(&mut self.pose.curiosity, target.curiosity, 0.16);
        follow(&mut self.pose.thinking, target.thinking, 0.16);
        follow(&mut self.pose.concern, target.concern, 0.16);
        follow(&mut self.pose.gaze.yaw, gaze.yaw.clamp(-0.65, 0.65), 0.11);
        follow(
            &mut self.pose.gaze.pitch,
            gaze.pitch.clamp(-0.45, 0.45),
            0.11,
        );
        follow(
            &mut self.pose.roll,
            target.curiosity * 0.06 - target.thinking * 0.06,
            0.22,
        );
        follow(&mut self.pose.speech, speech.clamp(0.0, 1.0), 0.045);
        self.pose.blink = target.blink;
        self.moving = moving;
        self.pose
    }
}
