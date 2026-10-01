//! Native tracking-space placement and off-axis cameras through an overlay plane.
use glam::{Mat4, Quat, Vec3, Vec4};

pub(super) const WIDTH: f32 = 0.75;
const RADIUS: f32 = 0.15;

#[derive(Default)]
pub(super) struct CompanionSpace {
    position: Option<Vec3>,
    target: Option<Vec3>,
    facing: f32,
    walk_phase: f32,
}

pub(super) struct Scene {
    pub plane: Mat4,
    pub model: Mat4,
    pub cameras: [Mat4; 2],
}

impl CompanionSpace {
    pub fn update(&mut self, head: Mat4, eyes: [Mat4; 2], dt: f32) -> Option<Scene> {
        if !head.is_finite() || eyes.iter().any(|m| !m.is_finite()) {
            return None;
        }
        let viewer = head.w_axis.truncate();
        let forward =
            (head.transform_vector3(-Vec3::Z) * Vec3::new(1.0, 0.0, 1.0)).try_normalize()?;
        let right = forward.cross(Vec3::Y);
        let home = viewer + forward * 1.15 + right * 0.43 - Vec3::Y * 0.35;
        let first = self.position.is_none();
        let position = self.position.get_or_insert(home);
        let distance = viewer.distance(*position);
        // Stay put when the user looks around. Follow gently only after leaving
        // the companion behind, or stepping too close to its geometry.
        if distance > 1.9 || distance < 0.6 {
            self.target = Some(home);
        }
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.05)
        } else {
            0.0
        };
        let mut moving = false;
        if let Some(target) = self.target {
            let offset = target - *position;
            let distance = offset.length();
            if distance < 0.01 {
                self.target = None;
            } else {
                *position += offset / distance * (0.38 * dt).min(distance);
                moving = true;
            }
        }
        if first || moving {
            let toward = viewer - *position;
            self.facing = toward.x.atan2(toward.z);
        }
        let toward = viewer - *position;
        let plane_yaw = toward.x.atan2(toward.z);
        let plane = Mat4::from_rotation_translation(Quat::from_rotation_y(plane_yaw), *position);
        let inverse = plane.inverse();
        let cameras = eyes.map(|eye| {
            camera_through_plane(inverse.transform_point3(eye.w_axis.truncate()), WIDTH)
        });
        let cameras = [cameras[0]?, cameras[1]?];
        self.walk_phase += if moving { dt * 14.0 } else { 0.0 };
        let bob = if moving {
            self.walk_phase.sin() * 0.012
        } else {
            0.0
        };
        let world_model = Mat4::from_scale_rotation_translation(
            Vec3::splat(RADIUS),
            Quat::from_rotation_y(self.facing),
            *position + Vec3::Y * bob,
        );
        Some(Scene {
            plane,
            model: inverse * world_model,
            cameras,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_eye_maps_native_plane_corners_to_the_same_texture_edges() {
        for eye in [
            Vec3::new(-0.032, 0.0, 1.0),
            Vec3::new(0.032, 0.0, 1.0),
            Vec3::new(0.3, 0.2, 0.8),
        ] {
            let camera = camera_through_plane(eye, WIDTH).unwrap();
            for (x, y) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
                let clip = camera * Vec4::new(x * WIDTH * 0.5, y * WIDTH * 0.5, 0.0, 1.0);
                assert!((clip.x / clip.w - x).abs() < 0.00001);
                assert!((clip.y / clip.w - y).abs() < 0.00001);
                assert!((0.0..1.0).contains(&(clip.z / clip.w)));
            }
        }
    }
    #[test]
    fn stereo_disparity_changes_sign_in_front_of_and_behind_native_plane() {
        let left = camera_through_plane(Vec3::new(-0.032, 0.0, 1.0), WIDTH).unwrap();
        let right = camera_through_plane(Vec3::new(0.032, 0.0, 1.0), WIDTH).unwrap();
        let disparity = |z| {
            let p = Vec4::new(0.0, 0.0, z, 1.0);
            let a = left * p;
            let b = right * p;
            a.x / a.w - b.x / b.w
        };
        assert!(disparity(0.15) > 0.0);
        assert!(disparity(-0.15) < 0.0);
        assert!(disparity(0.0).abs() < 0.00001);
        assert_ne!(left, right);
    }
    #[test]
    fn looking_around_keeps_anchor_while_walking_away_moves_it_at_bounded_speed() {
        let mut space = CompanionSpace::default();
        let head = Mat4::from_translation(Vec3::Y * 1.6);
        let eyes = [
            head * Mat4::from_translation(-Vec3::X * 0.032),
            head * Mat4::from_translation(Vec3::X * 0.032),
        ];
        space.update(head, eyes, 0.016).unwrap();
        let position = space.position.unwrap();
        let turned = head * Mat4::from_rotation_y(0.8);
        space.update(turned, eyes, 0.016).unwrap();
        assert_eq!(space.position, Some(position));
        let moved = head * Mat4::from_translation(Vec3::X * 3.0);
        space.update(moved, [moved; 2], 0.05).unwrap();
        assert!(space.position.unwrap().distance(position) <= 0.0191);
        assert!(space.position.unwrap().distance(position) > 0.0);
        assert!(camera_through_plane(Vec3::ZERO, WIDTH).is_none());
        assert!(camera_through_plane(Vec3::splat(f32::NAN), WIDTH).is_none());
    }
}
