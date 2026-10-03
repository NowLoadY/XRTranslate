//! The existing companion presented in tracking space, separate from captions.
use super::{
    graphics,
    openvr::{AvatarTracking, OpenVrOverlay, OpenVrSession, OverlayError, VulkanTexture},
    renderer::VrOverlayRenderer,
    space::{CompanionSpace, WIDTH},
};
use crate::ui::components::avatar::{Presentation, StereoRenderer};
use glam::{Mat4, Vec3};
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
    space: CompanionSpace,
    last_frame: Instant,
    last_bubble: Instant,
    visible: bool,
    bubble_visible: bool,
    bubble_pixels: Vec<u8>,
    pending_plane: Option<Mat4>,
    texture: VulkanTexture,
}

impl AvatarOverlay {
    pub fn new(session: &OpenVrSession) -> Result<Self, String> {
        let tracking = session.avatar_tracking()?;
        let renderer = Rc::new(RefCell::new(graphics::create(&tracking)?));
        // Retain the actual texture, queue, device and all GPU targets through
        // shutdown even if native creation, a later frame or reconnect fails.
        tracking.retain_graphics(Box::new(Rc::clone(&renderer)));
        let texture = graphics::texture(&renderer.borrow())?;
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
            space: CompanionSpace::default(),
            last_frame: Instant::now(),
            last_bubble: Instant::now() - Duration::from_secs(1),
            visible: false,
            bubble_visible: false,
            bubble_pixels: Vec::new(),
            pending_plane: None,
            texture,
        })
    }

    pub fn recenter(&mut self) {
        self.space = CompanionSpace::default();
        self.pending_plane = None;
    }

    pub fn frame(&mut self, presentation: &Presentation) -> Result<(), OverlayError> {
        if !presentation.visible {
            return self.hide();
        }
        if self.bubble_visible && presentation.speech.finished(presentation.clock) {
            self.bubble.hide()?;
            self.bubble_visible = false;
        }
        // A timeout is VROverlayError_TimedOut (34), not NoNeighbor (27).
        // Wait before reading predicted poses, with a bounded 8 ms wait.
        if let Err(error) = self.tracking.wait_frame() {
            if matches!(error, OverlayError::Api(_, 34)) {
                return Ok(());
            }
            return Err(error);
        }
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
        if !renderer.is_pending() {
            self.pending_plane = Some(scene.plane);
        }
        if !renderer
            .render(
                presentation.pose,
                &presentation.appearance,
                scene.model,
                scene.cameras,
            )
            .map_err(OverlayError::InvalidFrame)?
        {
            return Ok(());
        }
        // Tracking loss or hiding invalidates the associated spatial metadata.
        // Drain such a GPU frame without ever making the stale result visible.
        let Some(plane) = self.pending_plane.take() else {
            return Ok(());
        };
        self.overlay.set_world_transform(plane)?;
        self.overlay.set_vulkan_texture(&mut self.texture)?;
        if !self.visible {
            self.overlay.show()?;
            self.visible = true;
        }
        drop(renderer);
        self.bubble
            .set_world_transform(plane * Mat4::from_translation(Vec3::Y * 0.38))?;
        if presentation.speech.finished(presentation.clock) {
            if self.bubble_visible {
                self.bubble.hide()?;
                self.bubble_visible = false;
            }
        } else if now.duration_since(self.last_bubble) >= Duration::from_millis(80) {
            self.last_bubble = now;
            let pixels = self
                .text
                .render_speech(&presentation.speech, presentation.clock)
                .map_err(OverlayError::InvalidFrame)?;
            if self.bubble_pixels != pixels {
                self.bubble.set_raw_rgba(pixels.clone(), 640, 320)?;
                self.bubble_pixels = pixels;
            }
            if !self.bubble_visible {
                self.bubble.show()?;
                self.bubble_visible = true;
            }
        }
        Ok(())
    }

    fn hide(&mut self) -> Result<(), OverlayError> {
        self.pending_plane = None;
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
