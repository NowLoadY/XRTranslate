//! The existing companion presented in tracking space, separate from captions.
use super::{
    graphics,
    openvr::{AvatarTracking, OpenVrOverlay, OpenVrSession, OverlayError, VulkanTexture},
    renderer::VrOverlayRenderer,
    space::{CompanionSpace, WIDTH},
};
use crate::ui::components::avatar::{Presentation, PresentationSampler, StereoRenderer};
use glam::Vec3;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub(super) struct AvatarOverlay {
    // Destroy native overlays before releasing tracking and GPU owners.
    overlay: OpenVrOverlay,
    bubble: OpenVrOverlay,
    tracking: AvatarTracking,
    renderer: Rc<RefCell<StereoRenderer>>,
    text: VrOverlayRenderer,
    presentation: PresentationSampler,
    space: CompanionSpace,
    last_frame: Instant,
    last_bubble: Instant,
    visible: bool,
    attentive: bool,
    greeting: bool,
    bubble_visible: bool,
    bubble_pixels: Vec<u8>,
    bubble_texture: graphics::RgbaTexture,
    bubble_position: Option<Vec3>,
    texture: VulkanTexture,
}

impl AvatarOverlay {
    pub fn new(session: &OpenVrSession, gpu: Rc<graphics::Graphics>) -> Result<Self, String> {
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
        // Retain the actual texture, queue, device and all GPU targets through
        // shutdown even if native creation, a later frame or reconnect fails.
        tracking.retain_graphics(Box::new(Rc::clone(&renderer)));
        let texture = graphics::texture(&gpu.device, &renderer.borrow().texture)?;
        let bubble_texture = graphics::RgbaTexture::new(&tracking, gpu, 640, 320)?;
        let overlay = session.create_overlay("xrtranslate.avatar", "XRTranslate Avatar")?;
        overlay.configure_stereo().map_err(|e| e.to_string())?;
        overlay.set_width(WIDTH).map_err(|e| e.to_string())?;
        let bubble =
            session.create_overlay("xrtranslate.avatar.speech", "XRTranslate Avatar Speech")?;
        bubble.set_width(0.75).map_err(|e| e.to_string())?;
        Ok(Self {
            overlay,
            bubble,
            tracking,
            renderer,
            text: VrOverlayRenderer::new(640, 320)?,
            presentation: PresentationSampler::default(),
            space: CompanionSpace::default(),
            last_frame: Instant::now(),
            last_bubble: Instant::now() - Duration::from_secs(1),
            visible: false,
            attentive: false,
            greeting: false,
            bubble_visible: false,
            bubble_pixels: Vec::new(),
            bubble_texture,
            bubble_position: None,
            texture,
        })
    }

    pub fn request_visit(&mut self) {
        self.space.request_visit();
    }

    pub fn return_from_visit(&mut self) {
        self.space.return_from_visit();
    }

    pub fn resume(&mut self) {
        self.space = CompanionSpace::default();
        self.presentation = PresentationSampler::default();
        self.bubble_position = None;
        self.last_frame = Instant::now();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn attentive(&self) -> bool {
        self.visible && self.attentive
    }

    pub fn greeting(&self) -> bool {
        self.visible && self.greeting
    }

    pub fn frame(&mut self, presentation: &Presentation) -> Result<(), OverlayError> {
        let presentation = self.presentation.sample(presentation);
        if !presentation.visible {
            return self.hide();
        }
        // The worker paces frames. Waiting for another compositor signal here
        // would delay or discard valid frames before sampling the latest pose.
        let Some((eyes, head)) = self.tracking.eyes_and_head() else {
            return self.hide();
        };
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let Some(scene) = self.space.update(head, eyes, dt) else {
            return self.hide();
        };
        let mut renderer = self.renderer.borrow_mut();
        // The shared owner supplies expression gestures; tracking supplies only
        // spatial gaze. Desktop pointer/entrance transforms never enter this view.
        let mut pose = presentation.pose;
        pose.gaze.yaw = (pose.gaze.yaw + scene.gaze.yaw).clamp(-0.65, 0.65);
        pose.gaze.pitch = (pose.gaze.pitch + scene.gaze.pitch).clamp(-0.45, 0.45);
        if !renderer
            .render(pose, &presentation.appearance, scene.model, scene.cameras)
            .map_err(OverlayError::InvalidFrame)?
        {
            return Ok(());
        }
        let (plane, opacity) = (scene.plane, scene.opacity);
        self.overlay.set_world_transform(plane)?;
        self.overlay.set_alpha(opacity)?;
        self.overlay.set_vulkan_texture(&mut self.texture)?;
        if !self.visible {
            self.overlay.show()?;
            self.visible = true;
        }
        self.attentive = scene.attentive;
        self.greeting = scene.greeting;
        drop(renderer);
        let target = plane.w_axis.truncate() + Vec3::Y * 0.38;
        let position = self.bubble_position.get_or_insert(target);
        if opacity <= 0.01 {
            *position = target;
        } else {
            *position = position.lerp(target, 1.0 - (-dt.min(0.05) * 9.0).exp());
        }
        // Keep text upright and facing the viewer, with a little follow delay.
        let away = *position - head.w_axis.truncate();
        self.bubble.set_world_transform(
            glam::camera::rh::view::look_at_mat4(*position, *position + away, Vec3::Y).inverse(),
        )?;
        self.bubble.set_alpha(opacity)?;
        if (!presentation.speech.finished(presentation.clock) || self.bubble_visible)
            && now.duration_since(self.last_bubble) >= Duration::from_secs_f64(1.0 / 30.0)
        {
            let elapsed = now.duration_since(self.last_bubble).as_secs_f32();
            self.last_bubble = now;
            let pixels = self
                .text
                .render_speech(&presentation.speech, presentation.clock, elapsed)
                .map_err(OverlayError::InvalidFrame)?;
            if !self.text.speech_visible() {
                if self.bubble_visible {
                    self.bubble.hide()?;
                    self.bubble_visible = false;
                }
                return Ok(());
            }
            if self.bubble_pixels != pixels {
                self.bubble_texture.upload(&self.bubble, &pixels)?;
                self.bubble_pixels = pixels;
            }
            if !self.bubble_visible {
                self.bubble.show()?;
                self.bubble_visible = true;
            }
        }
        Ok(())
    }

    pub fn hide(&mut self) -> Result<(), OverlayError> {
        self.attentive = false;
        self.greeting = false;
        if self.visible {
            self.overlay.hide()?;
            self.visible = false;
        }
        if self.bubble_visible {
            self.bubble.hide()?;
            self.bubble_visible = false;
        }
        Ok(())
    }
}
