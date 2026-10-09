//! Native tracking-space placement and off-axis cameras through an overlay plane.
use crate::ui::components::avatar::Gaze;
use glam::{Mat4, Quat, Vec3, Vec4};

pub(super) const WIDTH: f32 = 0.75;
const RADIUS: f32 = 0.15;

struct Visit {
    anchor: Vec3,
    viewer: Vec3,
    phase: VisitPhase,
}

enum VisitPhase {
    Approach,
    Greeting(f32),
    Return,
}

#[derive(Default)]
pub(super) struct CompanionSpace {
    position: Option<Vec3>,
    target: Option<Vec3>,
    velocity: Vec3,
    facing: f32,
    gaze: Gaze,
    opacity: f32,
    relocating: bool,
    outside_for: f32,
    recenter_requested: bool,
    visit_requested: bool,
    visit: Option<Visit>,
}

pub(super) struct Scene {
    pub plane: Mat4,
    pub model: Mat4,
    pub cameras: [Mat4; 2],
    pub opacity: f32,
    pub gaze: Gaze,
    pub attentive: bool,
    pub greeting: bool,
}

impl CompanionSpace {
    pub fn recenter(&mut self) {
        self.visit = None;
        self.visit_requested = false;
        self.recenter_requested = true;
    }

    pub fn request_visit(&mut self) {
        if self.visit.is_none() {
            self.visit_requested = true;
            self.recenter_requested = false;
        }
    }

    pub fn return_from_visit(&mut self) {
        self.visit_requested = false;
        if let Some(visit) = self.visit.as_mut() {
            if matches!(visit.phase, VisitPhase::Return) {
                return;
            }
            visit.phase = VisitPhase::Return;
            let anchor = visit.anchor;
            if let Some(position) = self.position {
                self.move_to(position, anchor);
            }
        }
    }

    fn move_to(&mut self, position: Vec3, target: Vec3) {
        self.target = Some(target);
        self.relocating = position.distance(target) > 4.0 * RADIUS;
        self.outside_for = 0.0;
    }

    pub fn update(&mut self, head: Mat4, eyes: [Mat4; 2], dt: f32) -> Option<Scene> {
        if !head.is_finite() || eyes.iter().any(|m| !m.is_finite()) {
            return None;
        }
        let viewer = head.w_axis.truncate();
        let looking = head.transform_vector3(-Vec3::Z).try_normalize()?;
        let horizontal = looking * Vec3::new(1.0, 0.0, 1.0);
        let forward = if horizontal.length_squared() > 0.0001 {
            horizontal.normalize()
        } else {
            // Looking straight up or down still has a usable lateral heading.
            Vec3::Y
                .cross(head.transform_vector3(Vec3::X))
                .try_normalize()?
        };
        let right = forward.cross(Vec3::Y);
        let home = viewer + forward * 1.15 + right * 0.43 - Vec3::Y * 0.35;
        let first = self.position.is_none();
        let mut position = self.position.unwrap_or(home);
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.05)
        } else {
            0.0
        };
        if std::mem::take(&mut self.visit_requested) {
            self.visit = Some(Visit {
                anchor: self.target.unwrap_or(position),
                viewer,
                phase: VisitPhase::Approach,
            });
            self.move_to(
                position,
                viewer + forward * 0.82 + right * 0.18 - Vec3::Y * 0.25,
            );
        }
        if self.visit.as_ref().is_some_and(|visit| {
            viewer.distance(visit.viewer) > 0.8
                || viewer.distance(position) < 0.6
                || (matches!(visit.phase, VisitPhase::Return)
                    && !(0.6..=1.9).contains(&viewer.distance(visit.anchor)))
        }) {
            // A visit never chases a walking user or returns through their head.
            // An unexpected close step hides immediately before safe relocation.
            if viewer.distance(position) < 0.6 {
                self.opacity = 0.0;
            }
            self.recenter();
        }
        if let Some(mut visit) = self.visit.take() {
            let finished = match &mut visit.phase {
                VisitPhase::Approach if self.target.is_none() && self.opacity >= 0.95 => {
                    visit.phase = VisitPhase::Greeting(3.0);
                    false
                }
                VisitPhase::Greeting(remaining) => {
                    *remaining -= dt;
                    if *remaining <= 0.0 {
                        if (0.6..=1.9).contains(&viewer.distance(visit.anchor)) {
                            self.move_to(position, visit.anchor);
                            visit.phase = VisitPhase::Return;
                            false
                        } else {
                            self.recenter();
                            true
                        }
                    } else {
                        false
                    }
                }
                VisitPhase::Return => self.target.is_none(),
                _ => false,
            };
            if !finished {
                self.visit = Some(visit);
            }
        }
        // Looking around does not move the anchor. Require a sustained change
        // in personal distance, then finish one target before choosing another.
        self.outside_for = if self.visit.is_none()
            && self.target.is_none()
            && !(0.6..=1.9).contains(&viewer.distance(position))
        {
            self.outside_for + dt
        } else {
            0.0
        };
        if self.recenter_requested || self.outside_for >= 0.35 {
            self.move_to(position, home);
            self.recenter_requested = false;
        }
        let fading = self.relocating;
        if let Some(target) = self.target {
            let offset = target - position;
            let distance = offset.length();
            if self.relocating {
                self.velocity = Vec3::ZERO;
                self.opacity = (self.opacity - dt / 0.3).max(0.0);
                if self.opacity == 0.0 {
                    position = target;
                    self.target = None;
                    self.relocating = false;
                }
            } else if distance < 0.002 {
                position = target;
                self.velocity = Vec3::ZERO;
                self.target = None;
            } else {
                let desired = (offset * 4.0).clamp_length_max(0.38);
                self.velocity += (desired - self.velocity) * (1.0 - (-dt / 0.12).exp());
                position += (self.velocity * dt).clamp_length_max(distance);
            }
        }
        if !fading {
            self.opacity = (self.opacity + dt / 0.45).min(1.0);
        }
        self.position = Some(position);
        let toward = viewer - position;
        let plane_yaw = toward.x.atan2(toward.z);
        let turn = (plane_yaw - self.facing)
            .sin()
            .atan2((plane_yaw - self.facing).cos());
        if first || self.opacity == 0.0 {
            self.facing = plane_yaw;
        } else if turn.abs() > 0.3 {
            // Let the head lead; the body turns only beyond a comfortable angle.
            self.facing += (turn * (1.0 - (-dt / 0.35).exp())).clamp(-1.2 * dt, 1.2 * dt);
        }
        let local_viewer = Quat::from_rotation_y(-self.facing) * toward;
        let gaze = Gaze {
            yaw: local_viewer.x.atan2(local_viewer.z).clamp(-0.65, 0.65),
            pitch: (-local_viewer.y)
                .atan2(local_viewer.x.hypot(local_viewer.z))
                .clamp(-0.45, 0.45),
        };
        let follow = if first { 1.0 } else { 1.0 - (-dt / 0.12).exp() };
        self.gaze.yaw += (gaze.yaw - self.gaze.yaw) * follow;
        self.gaze.pitch += (gaze.pitch - self.gaze.pitch) * follow;
        let plane = Mat4::from_rotation_translation(Quat::from_rotation_y(plane_yaw), position);
        let inverse = plane.inverse();
        let cameras = eyes.map(|eye| {
            camera_through_plane(inverse.transform_point3(eye.w_axis.truncate()), WIDTH)
        });
        let cameras = [cameras[0]?, cameras[1]?];
        let world_model = Mat4::from_scale_rotation_translation(
            Vec3::splat(RADIUS),
            Quat::from_rotation_y(self.facing),
            position,
        );
        Some(Scene {
            plane,
            model: inverse * world_model,
            cameras,
            opacity: self.opacity * self.opacity * (3.0 - 2.0 * self.opacity),
            gaze: self.gaze,
            // This is head direction, not eye tracking. Invisible travel cannot
            // greet a viewer or prolong an existing attention encounter.
            attentive: self.opacity >= 0.5
                && !self.relocating
                && looking.dot(-toward.try_normalize()?) >= 15.0_f32.to_radians().cos(),
            greeting: self.opacity >= 0.5
                && self
                    .visit
                    .as_ref()
                    .is_some_and(|visit| matches!(visit.phase, VisitPhase::Greeting(_))),
        })
    }
}

/// Project through the native quad's corners, rather than using a symmetric
/// headset projection and compensating a second time for compositor placement.
pub(super) fn camera_through_plane(eye: Vec3, width: f32) -> Option<Mat4> {
    if !eye.is_finite() || !width.is_finite() || width <= 0.0 || eye.z < 0.25 {
        return None;
    }
    let near = 0.02;
    let far = 20.0;
    let left = (-width * 0.5 - eye.x) * near / eye.z;
    let right = (width * 0.5 - eye.x) * near / eye.z;
    let bottom = (-width * 0.5 - eye.y) * near / eye.z;
    let top = (width * 0.5 - eye.y) * near / eye.z;
    let projection = Mat4::from_cols(
        Vec4::new(2.0 * near / (right - left), 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * near / (top - bottom), 0.0, 0.0),
        Vec4::new(
            (right + left) / (right - left),
            (top + bottom) / (top - bottom),
            far / (near - far),
            -1.0,
        ),
        Vec4::new(0.0, 0.0, near * far / (near - far), 0.0),
    );
    Some(projection * Mat4::from_translation(-eye))
}
