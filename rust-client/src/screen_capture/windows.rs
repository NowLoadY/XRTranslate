use super::{CaptureError, CapturedFrame, OverlayRegion, RgbImage, intersection};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::{
    Graphics::Gdi::*,
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        },
    },
};

pub struct Capture(DPI_AWARENESS_CONTEXT);

impl Capture {
    pub fn new(_: &AtomicBool) -> Result<Self, String> {
        // This is the dedicated capture thread. Match the overlay's physical
        // desktop coordinates, including mixed-DPI and negative monitor origins.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err("Unable to configure screen coordinates.".into());
        }
        Ok(Self(previous))
    }

    pub fn region(
        &mut self,
        area: OverlayRegion,
        cancelled: &AtomicBool,
    ) -> Result<CapturedFrame, CaptureError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(CaptureError::fatal("Screen capture stopped."));
        }
        unsafe {
            let area = intersection(
                area,
                OverlayRegion {
                    x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                    y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                    width: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(0) as u32,
                    height: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(0) as u32,
                },
            )?;
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(CaptureError::fatal("Screen capture is unavailable."));
            }
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, area.width as i32, area.height as i32);
            let result = (|| {
                if memory.is_invalid() || bitmap.is_invalid() {
                    return Err(CaptureError::fatal("Unable to allocate a screen image."));
                }
                let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
                if previous.is_invalid() {
                    return Err(CaptureError::fatal("Unable to select the screen image."));
                }
                let copied = BitBlt(
                    memory,
                    0,
                    0,
                    area.width as i32,
                    area.height as i32,
                    Some(screen),
                    area.x,
                    area.y,
                    SRCCOPY | CAPTUREBLT,
                );
                // GetDIBits requires the bitmap to be deselected from the DC.
                SelectObject(memory, previous);
                copied.map_err(|e| CaptureError::retry(e.to_string()))?;
                let mut info = BITMAPINFO::default();
                info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = area.width as i32;
                info.bmiHeader.biHeight = -(area.height as i32);
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB.0;
                let mut pixels = vec![0u8; area.width as usize * area.height as usize * 4];
                if GetDIBits(
                    memory,
                    bitmap,
                    0,
                    area.height,
                    Some(pixels.as_mut_ptr().cast()),
                    &mut info,
                    DIB_RGB_COLORS,
                ) != area.height as i32
                {
                    return Err(CaptureError::retry("Unable to read the screen image."));
                }
                let image = RgbImage::from_fn(area.width, area.height, |x, y| {
                    let i = (y as usize * area.width as usize + x as usize) * 4;
                    image::Rgb([pixels[i + 2], pixels[i + 1], pixels[i]])
                });
                Ok(CapturedFrame::new(image, area))
            })();
            if !bitmap.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            if !memory.is_invalid() {
                let _ = DeleteDC(memory);
            }
            ReleaseDC(None, screen);
            result
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
