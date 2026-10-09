const BACKDROP_ENV: &str = "XRTRANSLATE_WINDOW_BACKDROP";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowBackdrop {
    None,
    Transparent,
    Acrylic,
    Mica,
}

impl WindowBackdrop {
    pub(crate) fn from_environment() -> Self {
        std::env::var(BACKDROP_ENV)
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    pub(crate) const fn uses_transparent_surface(self) -> bool {
        !matches!(self, Self::None)
    }

    pub(crate) fn clear_color(self) -> [f32; 4] {
        match self {
            Self::None => eframe::egui::Color32::from_rgb(248, 250, 252),
            Self::Transparent => eframe::egui::Color32::TRANSPARENT,
            // The system backdrop supplies the frosted material.  Keep the
            // WGPU surface fully transparent so its entire initial backing
            // store is cleared instead of leaving a white compositor tile.
            Self::Acrylic | Self::Mica => eframe::egui::Color32::TRANSPARENT,
        }
        .to_normalized_gamma_f32()
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "transparent" => Some(Self::Transparent),
            "acrylic" => Some(Self::Acrylic),
            "mica" => Some(Self::Mica),
            _ => None,
        }
    }
}

impl Default for WindowBackdrop {
    fn default() -> Self {
        if cfg!(windows) {
            Self::Acrylic
        } else {
            Self::None
        }
    }
}

pub(crate) fn apply(
    context: &eframe::CreationContext<'_>,
    requested: WindowBackdrop,
) -> Result<(), String> {
    #[cfg(windows)]
    return windows_impl::apply(context, requested);

    #[cfg(not(windows))]
    {
        let _ = (context, requested);
        Ok(())
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::WindowBackdrop;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOREDIRECTIONBITMAP,
    };
    use winit::platform::windows::{BackdropType, WindowExtWindows};

    pub(super) fn apply(
        context: &eframe::CreationContext<'_>,
        requested: WindowBackdrop,
    ) -> Result<(), String> {
        let window = context
            .winit_window()
            .ok_or_else(|| "eframe did not expose its winit window".to_owned())?;
        let hwnd = match window
            .window_handle()
            .map_err(|error| format!("failed to get native window handle: {error}"))?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => {
                windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut std::ffi::c_void)
            }
            handle => return Err(format!("unexpected native window handle: {handle:?}")),
        };
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 };
        let ex_style = if ex_style & WS_EX_NOREDIRECTIONBITMAP.0 == 0 {
            let updated = ex_style | WS_EX_NOREDIRECTIONBITMAP.0;
            unsafe {
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, updated as isize);
            }
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
        } else {
            ex_style
        };
        log::info!(
            "transparent window HWND={:?}, ex_style=0x{ex_style:08x}, no_redirection_bitmap={}",
            hwnd,
            ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0
        );
        let backdrop = match requested {
            WindowBackdrop::None | WindowBackdrop::Transparent => BackdropType::None,
            WindowBackdrop::Acrylic => BackdropType::TransientWindow,
            WindowBackdrop::Mica => BackdropType::MainWindow,
        };
        window.set_system_backdrop(backdrop);
        window.request_redraw();
        Ok(())
    }
}
