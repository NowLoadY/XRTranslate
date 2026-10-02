use super::{CaptureError, CapturedFrame, OverlayRegion, RgbImage, intersection};
use std::sync::atomic::{AtomicBool, Ordering};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{ConnectionExt, ImageFormat, ImageOrder},
    },
    rust_connection::RustConnection,
};

pub enum Capture {
    X11 {
        connection: RustConnection,
        screen: usize,
    },
    Wayland(super::wayland::Capture),
}

impl Capture {
    pub fn new(cancelled: &AtomicBool) -> Result<Self, String> {
        // XWayland GetImage is not a screen-sharing API. Always use the portal
        // in a Wayland session, even if only DISPLAY was inherited.
        if std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
        {
            return super::wayland::Capture::new(cancelled).map(Self::Wayland);
        }
        let (connection, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        Ok(Self::X11 { connection, screen })
    }

    pub fn region(
        &mut self,
        area: OverlayRegion,
        cancelled: &AtomicBool,
    ) -> Result<CapturedFrame, CaptureError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(CaptureError::fatal("Screen capture stopped."));
        }
        let Self::X11 { connection, screen } = self else {
            let Self::Wayland(capture) = self else {
                unreachable!()
            };
            return capture.region(area, cancelled);
        };
        let setup = connection.setup();
        let screen = &setup.roots[*screen];
        let area = intersection(
            area,
            OverlayRegion {
                x: 0,
                y: 0,
                width: screen.width_in_pixels.into(),
                height: screen.height_in_pixels.into(),
            },
        )?;
        let x = i16::try_from(area.x).map_err(|_| {
            CaptureError::retry("This screen position is outside the capture range.")
        })?;
        let y = i16::try_from(area.y).map_err(|_| {
            CaptureError::retry("This screen position is outside the capture range.")
        })?;
        let reply = connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                screen.root,
                x,
                y,
                area.width as u16,
                area.height as u16,
                u32::MAX,
            )
            .map_err(CaptureError::fatal)?
            .reply()
            .map_err(CaptureError::fatal)?;
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == reply.depth)
            .ok_or_else(|| CaptureError::fatal("Unsupported screen pixel format."))?;
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == screen.root_visual)
            .ok_or_else(|| CaptureError::fatal("Unsupported screen color format."))?;
        if !matches!(format.bits_per_pixel, 8 | 16 | 24 | 32)
            || !matches!(format.scanline_pad, 8 | 16 | 32)
        {
            return Err(CaptureError::fatal("Unsupported screen pixel format."));
        }
        let bytes = (format.bits_per_pixel / 8) as usize;
        let stride = (area.width as usize * format.bits_per_pixel as usize)
            .div_ceil(format.scanline_pad as usize)
            * format.scanline_pad as usize
            / 8;
        if reply.data.len() < stride * area.height as usize {
            return Err(CaptureError::fatal("Incomplete screen image."));
        }
        let component = |pixel: u32, mask: u32| -> u8 {
            if mask == 0 {
                return 0;
            }
            let value = (pixel & mask) >> mask.trailing_zeros();
            (value as u64 * 255 / (mask >> mask.trailing_zeros()) as u64) as u8
        };
        let image = RgbImage::from_fn(area.width, area.height, |x, y| {
            let start = y as usize * stride + x as usize * bytes;
            let mut raw = [0u8; 4];
            let pixel = if setup.image_byte_order == ImageOrder::LSB_FIRST {
                raw[..bytes].copy_from_slice(&reply.data[start..start + bytes]);
                u32::from_le_bytes(raw)
            } else {
                raw[4 - bytes..].copy_from_slice(&reply.data[start..start + bytes]);
                u32::from_be_bytes(raw)
            };
            image::Rgb([
                component(pixel, visual.red_mask),
                component(pixel, visual.green_mask),
                component(pixel, visual.blue_mask),
            ])
        });
        Ok(CapturedFrame::new(image, area))
    }
}

/// Metadata only: safe to query on XWayland without reading any drawable.
pub(super) fn monitors(
    connection: &RustConnection,
    screen: usize,
) -> Result<Vec<OverlayRegion>, String> {
    let reply = connection
        .randr_get_monitors(connection.setup().roots[screen].root, true)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let mut monitors = Vec::new();
    for monitor in reply.monitors {
        let bounds = OverlayRegion {
            x: monitor.x.into(),
            y: monitor.y.into(),
            width: monitor.width.into(),
            height: monitor.height.into(),
        };
        if bounds.width != 0 && bounds.height != 0 && !monitors.contains(&bounds) {
            monitors.push(bounds);
        }
    }
    Ok(monitors)
}
