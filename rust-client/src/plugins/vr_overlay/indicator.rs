//! A static green HUD dot. Only its alpha changes during the slow pulse.
use super::{
    graphics,
    openvr::{AvatarTracking, HmdMatrix34, OpenVrOverlay, OpenVrSession, OverlayError},
    runtime::VrOverlaySettings,
};
use glam::{Mat4, Vec3};
use std::{rc::Rc, time::Instant};

pub(super) struct VoiceIndicator {
    overlay: OpenVrOverlay,
    _texture: graphics::RgbaTexture,
    visible: bool,
    clock: Instant,
}

impl VoiceIndicator {
    pub fn new(
        session: &OpenVrSession,
        tracking: &AvatarTracking,
        gpu: Rc<graphics::Graphics>,
    ) -> Result<Self, String> {
        let overlay =
            session.create_overlay("xrtranslate.voice.indicator", "XRTranslate Voice Range")?;
        overlay.set_width(0.2).map_err(|e| e.to_string())?;
        let mut texture = graphics::RgbaTexture::new(tracking, gpu, 128, 128)?;
        texture
            .upload(&overlay, &pixels())
            .map_err(|e| e.to_string())?;
        Ok(Self {
            overlay,
            _texture: texture,
            visible: false,
            clock: Instant::now(),
        })
    }

    pub fn hide(&mut self) -> Result<(), OverlayError> {
        if self.visible {
            self.overlay.hide()?;
            self.visible = false;
        }
        Ok(())
    }

    pub fn frame(
        &mut self,
        active: bool,
        settings: &VrOverlaySettings,
    ) -> Result<(), OverlayError> {
        if !active {
            return self.hide();
        }
        if !self.visible {
            self.clock = Instant::now();
        }
        self.overlay.set_hmd_transform(hud_corner(settings))?;
        self.overlay
            .set_alpha(pulse_alpha(self.clock.elapsed().as_secs_f32()))?;
        if !self.visible {
            self.overlay.show()?;
            self.visible = true;
        }
        Ok(())
    }
}

fn hud_corner(settings: &VrOverlaySettings) -> Mat4 {
    let hud = HmdMatrix34::auto_hmd_hud(settings.distance_meters, settings.vertical_offset_meters)
        .rigid_matrix()
        .expect("HUD transform is rigid");
    // The subtitle HUD is 640 x 320. Inset the dot 1 cm from its top-left corner.
    hud * Mat4::from_translation(Vec3::new(
        -settings.overlay_width_meters * 0.5 + 0.01,
        settings.overlay_width_meters * 0.25 - 0.01,
        0.002,
    ))
}

fn pulse_alpha(seconds: f32) -> f32 {
    0.2 + 0.8 * (0.5 + 0.5 * (seconds * std::f32::consts::TAU / 2.4).cos())
}

pub(super) fn pixels() -> Vec<u8> {
    // Transparent padding lets the existing 20 cm minimum overlay width carry
    // a small 8 mm dot. Upload once; pulsing needs no rerasterization or uploads.
    (0..128 * 128)
        .flat_map(|i| {
            let x = (i % 128) as f32 + 0.5 - 64.0;
            let y = (i / 128) as f32 + 0.5 - 64.0;
            let alpha = ((3.0 - x.hypot(y)).clamp(0.0, 1.0) * 255.0).round() as u8;
            [72, 225, 118, alpha]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires SteamVR and a connected headset"]
    fn live_hud_indicator() {
        use super::super::openvr::OpenVrApi;
        use std::time::Duration;
        let session = OpenVrApi::try_load().unwrap().init_overlay().unwrap();
        let tracking = session.avatar_tracking().unwrap();
        let gpu = graphics::create(&tracking).unwrap();
        let mut indicator = VoiceIndicator::new(&session, &tracking, gpu).unwrap();
        for _ in 0..360 {
            indicator
                .frame(true, &VrOverlaySettings::default())
                .unwrap();
            std::thread::sleep(Duration::from_millis(16));
        }
        indicator
            .frame(false, &VrOverlaySettings::default())
            .unwrap();
        assert!(!indicator.visible);
    }

    #[test]
    fn dot_is_small_and_pulse_is_slow_and_smooth() {
        let pixels = pixels();
        assert_eq!(pixels.len(), 128 * 128 * 4);
        let visible = pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!((12..40).contains(&visible));
        assert!((pulse_alpha(0.0) - 1.0).abs() < 1e-5);
        assert!((pulse_alpha(1.2) - 0.2).abs() < 1e-5);
        assert!((pulse_alpha(2.4) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dot_follows_the_top_left_of_the_configured_hud() {
        let settings = VrOverlaySettings::default();
        let hud =
            HmdMatrix34::auto_hmd_hud(settings.distance_meters, settings.vertical_offset_meters)
                .rigid_matrix()
                .unwrap();
        let local = hud.inverse() * hud_corner(&settings);
        assert!((local.w_axis.x + settings.overlay_width_meters * 0.5 - 0.01).abs() < 1e-5);
        assert!((local.w_axis.y - settings.overlay_width_meters * 0.25 + 0.01).abs() < 1e-5);
    }
}
