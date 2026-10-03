//! Damped inertia from actual movement; roots stay attached and idle hair settles.
use super::super::Pose;
use eframe::egui::{self, Id, Pos2};
use glam::{Vec2, Vec3};
use std::{
    f32::consts::{PI, TAU},
    time::Duration,
};

#[derive(Clone, Default)]
pub(in super::super) struct Motion {
    previous: Option<(Vec3, Vec3, f64)>,
    bend: Vec2,
    velocity: Vec2,
}

impl Motion {
    pub fn sample(&mut self, angles: Vec3, center: Vec3, scale: f32, now: f64) -> [f32; 2] {
        let Some((last_angles, last_center, last_time)) =
            self.previous.replace((angles, center, now))
        else {
            return self.bend.to_array();
        };
        let dt = (now - last_time) as f32;
        if dt <= 0.0 {
            return self.bend.to_array();
        }
        let movement = (center - last_center) / scale.max(0.001);
        if dt > 0.25 || movement.length() > 4.0 {
            self.bend = Vec2::ZERO;
            self.velocity = Vec2::ZERO;
            return self.bend.to_array();
        }
        let turn = (angles - last_angles).map(|a| (a + PI).rem_euclid(TAU) - PI) / dt;
        let target = Vec2::new(
            -turn.x * 0.25 + turn.z * 0.36 - movement.x / dt * 0.028,
            turn.y * 0.28 - (movement.y + movement.z) / dt * 0.022,
        )
        .clamp(Vec2::splat(-0.8), Vec2::splat(0.8));
        let steps = (dt * 120.0).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        // A slower spring and lighter damping give the tips a soft trailing swing.
        let frequency = TAU * 1.7;
        let damping = 2.0 * 0.42 * frequency;
        for _ in 0..steps {
            self.velocity +=
                ((target - self.bend) * frequency * frequency - self.velocity * damping) * h;
            self.bend = (self.bend + self.velocity * h).clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
        }
        if !self.moving() {
            self.bend = Vec2::ZERO;
            self.velocity = Vec2::ZERO;
        }
        self.bend.to_array()
    }

    fn moving(&self) -> bool {
        self.bend.length_squared() + self.velocity.length_squared() > 0.0001
    }
}

pub(in super::super) fn paint(
    ctx: &egui::Context,
    id: Id,
    center: Pos2,
    radius: f32,
    pose: Pose,
) -> [f32; 2] {
    let id = id.with("hair_motion");
    let mut motion = ctx
        .data(|data| data.get_temp::<Motion>(id))
        .unwrap_or_default();
    let origin = ctx.input(|input| {
        input
            .viewport()
            .inner_rect
            .map_or(Pos2::ZERO, |rect| rect.min)
    });
    let bend = motion.sample(
        Vec3::new(pose.gaze.yaw, pose.gaze.pitch, pose.roll),
        Vec3::new(center.x + origin.x, center.y + origin.y, 0.0),
        radius,
        ctx.input(|input| input.time),
    );
    if motion.moving() {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    ctx.data_mut(|data| data.insert_temp(id, motion));
    bend
}
