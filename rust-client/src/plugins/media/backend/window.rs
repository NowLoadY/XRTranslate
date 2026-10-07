#![allow(dead_code)]

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOP, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos, ShowWindow, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeWindowHandle(pub *mut std::ffi::c_void);

#[cfg(not(windows))]
unsafe impl Send for NativeWindowHandle {}
#[cfg(not(windows))]
unsafe impl Sync for NativeWindowHandle {}

#[cfg(windows)]
pub struct NativeVideoHost {
    pub hwnd: HWND,
    _parent_hwnd: HWND,
}

#[cfg(not(windows))]
pub struct NativeVideoHost {
    pub hwnd: NativeWindowHandle,
}

#[cfg(windows)]
unsafe impl Send for NativeVideoHost {}
#[cfg(windows)]
unsafe impl Sync for NativeVideoHost {}

#[cfg(windows)]
impl NativeVideoHost {
    pub fn new(parent: usize) -> Result<Self, String> {
        Self::new_with_parent(HWND(parent as *mut std::ffi::c_void))
    }

    pub fn new_with_parent(parent_hwnd: HWND) -> Result<Self, String> {
        let class_name = "STATIC\0".encode_utf16().collect::<Vec<u16>>();

        let hwnd = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(parent_hwnd),
                None,
                None,
                None,
            )
        }
        .map_err(|e| e.to_string())?;

        Ok(Self {
            hwnd,
            _parent_hwnd: parent_hwnd,
        })
    }

    pub fn set_rect(&self, x: i32, y: i32, width: i32, height: i32) {
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .ok();
        }
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
impl NativeVideoHost {
    /// Linux uses eframe's native window directly. Embedding an external mpv
    /// child window is intentionally unavailable until a Wayland/X11 host is
    /// selected, so callers can fall back without platform conditionals.
    pub fn new(_parent: usize) -> Result<Self, String> {
        Err("embedded video windows are not available on this platform".into())
    }

    pub fn set_rect(&self, _x: i32, _y: i32, _width: i32, _height: i32) {}
    pub fn show(&self) {}
    pub fn hide(&self) {}
}

#[cfg(windows)]
impl Drop for NativeVideoHost {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(not(windows))]
impl Drop for NativeVideoHost {
    fn drop(&mut self) {}
}
