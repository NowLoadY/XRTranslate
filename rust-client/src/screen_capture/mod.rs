//! Desktop pixels supplied to content recognition, independent of translation.
use crate::overlay_ipc::OverlayRegion;
use image::RgbImage;
use std::sync::atomic::AtomicBool;

pub struct CapturedFrame {
    pub image: RgbImage,
    bounds: OverlayRegion,
    source_size: (u32, u32),
    offset: (u32, u32),
}

impl CapturedFrame {
    pub fn new(image: RgbImage, bounds: OverlayRegion) -> Self {
        Self {
            source_size: image.dimensions(),
            image,
            bounds,
            offset: (0, 0),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn cropped(
        image: &RgbImage,
        bounds: OverlayRegion,
        area: OverlayRegion,
    ) -> Result<Self, CaptureError> {
        let (left, top, right, bottom) = pixel_bounds(bounds, image.dimensions(), area)?;
        let (width, height) = (right - left, bottom - top);
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 16_777_216 {
            return Err(CaptureError::retry("Choose a smaller area to recognize."));
        }
        Ok(Self {
            image: image::imageops::crop_imm(image, left, top, width, height).to_image(),
            bounds,
            source_size: image.dimensions(),
            offset: (left, top),
        })
    }

    /// Exclude a visible presentation surface, also when the desktop portal
    /// supplies scaled pixels or the requested region was clipped at a monitor.
    pub fn mask(&mut self, excluded: OverlayRegion) -> bool {
        let Ok((left, top, right, bottom)) = pixel_bounds(self.bounds, self.source_size, excluded)
        else {
            return false;
        };
        // Map against the original stream, then subtract the crop offset. A
        // rounded crop cannot accurately reconstruct a fractional pixel scale.
        let left = left.saturating_sub(self.offset.0).min(self.image.width()) as usize;
        let right = right.saturating_sub(self.offset.0).min(self.image.width()) as usize;
        let top = top.saturating_sub(self.offset.1).min(self.image.height()) as usize;
        let bottom = bottom
            .saturating_sub(self.offset.1)
            .min(self.image.height()) as usize;
        if left >= right || top >= bottom {
            return false;
        }
        let stride = self.image.width() as usize * 3;
        let pixels: &mut [u8] = self.image.as_mut();
        for row in pixels.chunks_exact_mut(stride).take(bottom).skip(top) {
            row[left * 3..right * 3].fill(255);
        }
        true
    }
}

fn pixel_bounds(
    bounds: OverlayRegion,
    size: (u32, u32),
    area: OverlayRegion,
) -> Result<(u32, u32, u32, u32), CaptureError> {
    let area = intersection(area, bounds)?;
    let axis = |start: i32, length: u32, origin: i32, extent: u32, pixels: u32| {
        let offset = (i64::from(start) - i64::from(origin)) as u64;
        let first = offset * u64::from(pixels) / u64::from(extent);
        let last = ((offset + u64::from(length)) * u64::from(pixels)).div_ceil(u64::from(extent));
        (first as u32, last.min(u64::from(pixels)) as u32)
    };
    let (left, right) = axis(area.x, area.width, bounds.x, bounds.width, size.0);
    let (top, bottom) = axis(area.y, area.height, bounds.y, bounds.height, size.1);
    Ok((left, top, right, bottom))
}

#[derive(Debug)]
pub struct CaptureError {
    pub message: String,
    pub fatal: bool,
}

impl CaptureError {
    fn retry(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            fatal: false,
        }
    }

    fn fatal(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
            fatal: true,
        }
    }
}

// Use wide arithmetic before clipping: monitors may have negative origins.
pub(crate) fn intersection(
    area: OverlayRegion,
    bounds: OverlayRegion,
) -> Result<OverlayRegion, CaptureError> {
    let left = i64::from(area.x).max(i64::from(bounds.x));
    let top = i64::from(area.y).max(i64::from(bounds.y));
    let right = (i64::from(area.x) + i64::from(area.width))
        .min(i64::from(bounds.x) + i64::from(bounds.width));
    let bottom = (i64::from(area.y) + i64::from(area.height))
        .min(i64::from(bounds.y) + i64::from(bounds.height));
    if right <= left || bottom <= top {
        return Err(CaptureError::retry(
            "Move the frame onto a screen selected for sharing.",
        ));
    }
    Ok(OverlayRegion {
        x: left as i32,
        y: top as i32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod windows;

pub struct ScreenCapture {
    #[cfg(target_os = "linux")]
    backend: linux::Capture,
    #[cfg(windows)]
    backend: windows::Capture,
}

impl ScreenCapture {
    pub fn new(cancelled: &AtomicBool) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        let backend = linux::Capture::new(cancelled)?;
        #[cfg(windows)]
        let backend = windows::Capture::new(cancelled)?;
        Ok(Self { backend })
    }

    pub fn region(
        &mut self,
        region: OverlayRegion,
        cancelled: &AtomicBool,
    ) -> Result<CapturedFrame, CaptureError> {
        if region.width == 0
            || region.height == 0
            || u64::from(region.width) * u64::from(region.height) > 16_777_216
        {
            return Err(CaptureError::retry("Choose a smaller area to recognize."));
        }
        self.backend.region(region, cancelled)
    }
}
