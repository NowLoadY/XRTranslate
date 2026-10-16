//! Physical voice objects rendered through their own fitted stereo window.
use super::{
    graphics,
    input::Input,
    interaction::Interaction,
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

    pub fn freeze_controls(&self) -> bool {
        self.visible && self.interaction.freeze_controls
    }

    pub fn suspend(&mut self) -> Vec<VoiceAction> {
        self.input.suspend();
        if self.visible {
            let _ = self.overlay.hide();
            self.visible = false;
        }
        self.last_frame = Instant::now();
        self.interaction.suspend()
    }

    pub fn frame(
        &mut self,
        control_root: Option<Mat4>,
        snapshot: &VoiceSnapshot,
    ) -> Result<Vec<VoiceAction>, OverlayError> {
        if self.input.suspended_by_dashboard() {
            return Ok(self.suspend());
        }
        let Some(control_root) = control_root.filter(|_| snapshot.enabled) else {
            return Ok(self.suspend());
        };
        let Some((eyes, head)) = self.tracking.eyes_and_head() else {
            return Ok(self.suspend());
        };
        let hands = self.input.sample().map_err(OverlayError::InvalidFrame)?;
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let (actions, pulses) = self
            .interaction
            .update(control_root, head, hands, snapshot, dt);
        for hand in pulses {
            self.input.pulse(hand);
        }
        let mut model = self.interaction.model(control_root, snapshot);
        let Some((plane, width, cameras)) = window(&model, head, eyes) else {
            return Ok(self.suspend());
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
// points stay in tracking space, even when the mic is 10 cm from the user's face.
// Fit both eyes' projected bounds so forward buttons aren't clipped by the
// companion's much smaller native quad. This supplies colour, not game depth.
fn window(model: &Model, head: Mat4, eyes: [Mat4; 2]) -> Option<(Mat4, f32, [Mat4; 2])> {
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
