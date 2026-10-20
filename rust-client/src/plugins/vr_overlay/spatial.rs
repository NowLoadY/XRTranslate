//! Physical voice objects rendered through their own fitted stereo window.
use super::{
    graphics,
    input::Input,
    interaction::{CONTROL_HAND, Interaction, controller_root},
    openvr::{AvatarTracking, OpenVrOverlay, OpenVrSession, OverlayError, VulkanTexture},
    space::camera_through_plane,
};
use crate::{
    ui::components::avatar::{Model, StereoRenderer},
    voice_interaction::{VoiceAction, VoiceSnapshot},
};
use glam::{Mat4, Vec2, Vec3};
use std::{cell::RefCell, path::Path, rc::Rc, time::Instant};

pub(super) struct VoiceOverlay {
    overlay: OpenVrOverlay,
    input: Input,
    tracking: AvatarTracking,
    renderer: Rc<RefCell<StereoRenderer>>,
    texture: VulkanTexture,
    interaction: Interaction,
    visible: bool,
    last_frame: Instant,
}

impl VoiceOverlay {
    pub fn new(
        session: &OpenVrSession,
        gpu: Rc<graphics::Graphics>,
        input_directory: &Path,
    ) -> Result<Self, String> {
        let input = Input::new(session, input_directory)?;
        let tracking = session.avatar_tracking()?;
        let validation = gpu
            .device
            .push_error_scope(eframe::wgpu::ErrorFilter::Validation);
        let renderer = Rc::new(RefCell::new(StereoRenderer::new(
            gpu.device.clone(),
            gpu.queue.clone(),
        )));
        if let Some(error) = futures::executor::block_on(validation.pop()) {
            return Err(error.to_string());
        }
        tracking.retain_graphics(Box::new(Rc::clone(&renderer)));
        let texture = graphics::texture(&gpu.device, &renderer.borrow().texture)?;
        let overlay = session.create_overlay("xrtranslate.voice", "XRTranslate Voice Objects")?;
        overlay.configure_stereo().map_err(|e| e.to_string())?;
        Ok(Self {
            overlay,
            input,
            tracking,
            renderer,
            texture,
            interaction: Interaction::default(),
            visible: false,
            last_frame: Instant::now(),
        })
    }

    pub fn suspend(&mut self) -> Vec<VoiceAction> {
        if self.visible {
            let _ = self.overlay.hide();
            self.visible = false;
        }
        self.last_frame = Instant::now();
        self.interaction.suspend()
    }

    pub fn frame(&mut self, snapshot: &VoiceSnapshot) -> Result<Vec<VoiceAction>, OverlayError> {
        if self.input.suspended_by_dashboard() {
            return Ok(self.suspend());
        }
        if !snapshot.enabled {
            return Ok(self.suspend());
        }
        let Some((eyes, head)) = self.tracking.eyes_and_head() else {
            return Ok(self.suspend());
        };
        let hands = self.input.sample().map_err(OverlayError::InvalidFrame)?;
        let Some(left) = hands[CONTROL_HAND].pose else {
            return Ok(self.suspend());
        };
        let control_root = controller_root(left);
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let (actions, pulses) = self.interaction.update(control_root, hands, snapshot, dt);
        for hand in pulses {
            self.input.pulse(hand);
        }
        let mut model = self.interaction.model(control_root, snapshot);
        let Some((plane, width, cameras)) = window(&model, head, eyes) else {
            // No visible buttons, or the arm is outside the current view.
            if self.visible {
                self.overlay.hide()?;
                self.visible = false;
            }
            return Ok(actions);
        };
        let inverse = plane.inverse();
        for part in &mut model.parts {
            part.transform = inverse * part.transform;
        }
        if self
            .renderer
            .borrow_mut()
            .render_model(model, cameras)
            .map_err(OverlayError::InvalidFrame)?
        {
            self.overlay.set_width(width)?;
            self.overlay.set_world_transform(plane)?;
            self.overlay.set_vulkan_texture(&mut self.texture)?;
            if !self.visible {
                self.overlay.show()?;
                self.visible = true;
            }
        }
        Ok(actions)
    }
}

// The window sits at a comfortable projection distance. The mesh and contact
// points stay in tracking space even when the controller is close to the head.
// Fit both eyes' projected bounds so forward buttons aren't clipped by the
// companion's much smaller native quad. This supplies colour, not game depth.
pub(super) fn window(model: &Model, head: Mat4, eyes: [Mat4; 2]) -> Option<(Mat4, f32, [Mat4; 2])> {
    let distance = 0.65;
    let inverse_head = head.inverse();
    let eye_positions = eyes.map(|eye| inverse_head.transform_point3(eye.w_axis.truncate()));
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for part in &model.parts {
        let transform = inverse_head * part.transform;
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let point = transform.transform_point3(Vec3::new(x, y, z));
                    for eye in eye_positions {
                        if point.z >= eye.z - 0.035 {
                            continue;
                        }
                        let t = (-distance - eye.z) / (point.z - eye.z);
                        let p = (eye + (point - eye) * t).truncate();
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
            }
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let center = (min + max) * 0.5;
    let width = ((max - min).max_element() * 1.08 + 0.04).clamp(0.20, 5.0);
    let plane = head * Mat4::from_translation(Vec3::new(center.x, center.y, -distance));
    let inverse = plane.inverse();
    let cameras = eyes
        .map(|eye| camera_through_plane(inverse.transform_point3(eye.w_axis.truncate()), width));
    Some((plane, width, [cameras[0]?, cameras[1]?]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::avatar::{Geometry, Part};
    use eframe::egui::Color32;

    #[test]
    #[ignore = "Requires SteamVR, valid left-controller tracking and a closed dashboard"]
    fn live_left_controller_overlay() {
        use super::super::openvr::OpenVrApi;
        use std::time::Duration;
        let session = OpenVrApi::try_load().unwrap().init_overlay().unwrap();
        let tracking = session.avatar_tracking().unwrap();
        let gpu = graphics::create(&tracking).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut objects = VoiceOverlay::new(&session, gpu, directory.path()).unwrap();
        let snapshot = VoiceSnapshot {
            enabled: true,
            review_enabled: true,
            can_send: true,
            can_revise: true,
            ..Default::default()
        };
        let mut rendered = false;
        for _ in 0..1200 {
            objects.frame(&snapshot).unwrap();
            if objects.visible && !rendered {
                println!("Left-controller overlay is rendering from live poses");
                rendered = true;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        assert!(rendered, "No valid left-controller frame was rendered");
        objects.suspend();
    }

    #[test]
    fn fitted_stereo_window_preserves_square_proportions() {
        for position in [Vec3::new(0.0, -0.2, -0.7), Vec3::new(-0.3, -0.1, -0.25)] {
            let mut model = Model::default();
            let mut part = Part::new(Geometry::SolidSphere, Color32::WHITE);
            part.transform = Mat4::from_translation(position) * Mat4::from_scale(Vec3::splat(0.05));
            model.add(part);
            let eyes = [-0.032, 0.032].map(|x| Mat4::from_translation(Vec3::X * x));
            let (plane, _, cameras) = window(&model, Mat4::IDENTITY, eyes).unwrap();
            for camera in cameras {
                let project = |p| (camera * plane.inverse()).project_point3(p);
                let center = project(position);
                let horizontal = project(position + Vec3::X * 0.02) - center;
                let vertical = project(position + Vec3::Y * 0.02) - center;
                assert!((horizontal.length() - vertical.length()).abs() < 1e-5);
                assert!(horizontal.y.abs() < 1e-5 && vertical.x.abs() < 1e-5);
            }
        }
    }
}
