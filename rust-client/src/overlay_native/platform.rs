//! Native input regions for the shared desktop overlay.
use eframe::egui::{self, Rect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

#[cfg(target_os = "linux")]
fn software_environment() -> std::io::Result<[(&'static str, &'static str); 5]> {
    let mesa = [
        "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
        "/usr/local/share/glvnd/egl_vendor.d/50_mesa.json",
        "/etc/glvnd/egl_vendor.d/50_mesa.json",
    ]
    .into_iter()
    .find(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|manifest| {
                manifest["ICD"]["library_path"]
                    .as_str()
                    .is_some_and(|library| {
                        std::path::Path::new(library)
                            .file_name()
                            .is_some_and(|name| {
                                name.to_string_lossy().starts_with("libEGL_mesa.so")
                            })
                    })
            })
    })
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Mesa software rendering is unavailable.",
        )
    })?;
    Ok([
        ("LIBGL_ALWAYS_SOFTWARE", "1"),
        ("__GLX_VENDOR_LIBRARY_NAME", "mesa"),
        ("GALLIUM_DRIVER", "llvmpipe"),
        ("LP_NUM_THREADS", "2"),
        ("__EGL_VENDOR_LIBRARY_FILENAMES", mesa),
    ])
}

#[cfg(target_os = "linux")]
pub(crate) fn configure_software_environment(
    command: &mut std::process::Command,
) -> std::io::Result<()> {
    command
        .env_remove("WAYLAND_DISPLAY")
        .envs(software_environment()?);
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn validate_software_environment() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    if software_environment()?
        .into_iter()
        .any(|(key, value)| std::env::var(key).as_deref() != Ok(value))
    {
        return Err(
            "The floating window requires software rendering. Reopen it from the application."
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct InputRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

pub(super) struct InputRegion {
    platform: Platform,
    rectangles: Vec<InputRect>,
}

impl InputRegion {
    pub fn new(window: &Window) -> Result<Self, String> {
        Ok(Self {
            platform: Platform::new(window)?,
            rectangles: Vec::new(),
        })
    }

    pub fn update(&mut self, regions: &[(Rect, f32)], scale: f32) -> Result<(), String> {
        let mut rectangles = Vec::new();
        for &(rect, radius) in regions {
            if !rect.is_positive() || !rect.is_finite() {
                continue;
            }
            let rect = rect * scale;
            let x = rect.left().ceil() as i32;
            let y = rect.top().ceil() as i32;
            let width = (rect.right().floor() as i32 - x).max(0) as u32;
            let height = (rect.bottom().floor() as i32 - y).max(0) as u32;
            let radius = (radius * scale).ceil().max(0.0) as u32;
            let radius = radius.min(width / 2).min(height / 2);
            // Matching rounded corners leave even the tiny transparent corners
            // click-through, without sampling pixels from the GPU.
            for row in 0..radius {
                let dy = radius as f32 - row as f32 - 0.5;
                let inset =
                    (radius as f32 - ((radius * radius) as f32 - dy * dy).sqrt()).ceil() as u32;
                for y in [y + row as i32, y + height as i32 - row as i32 - 1] {
                    rectangles.push(InputRect {
                        x: x + inset as i32,
                        y,
                        width: width.saturating_sub(inset * 2),
                        height: 1,
                    });
                }
            }
            if height > radius * 2 {
                rectangles.push(InputRect {
                    x,
                    y: y + radius as i32,
                    width,
                    height: height - radius * 2,
                });
            }
        }
        rectangles.retain(|rect| rect.width > 0 && rect.height > 0);
        if rectangles != self.rectangles {
            self.platform.apply(&rectangles)?;
            self.rectangles = rectangles;
        }
        Ok(())
    }

    pub fn work_area(&self, window: &Window) -> Rect {
        let monitor = window
            .current_monitor()
            .or_else(|| window.available_monitors().next());
        let monitor = monitor
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                Rect::from_min_size(
                    egui::pos2(position.x as f32, position.y as f32),
                    egui::vec2(size.width as f32, size.height as f32),
                )
            })
            .unwrap_or(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 1080.0),
            ));
        self.platform
            .work_area()
            .map(|work| monitor.intersect(work))
            .filter(Rect::is_positive)
            .unwrap_or(monitor)
    }
}

#[cfg(target_os = "linux")]
struct Platform {
    connection: x11rb::rust_connection::RustConnection,
    window: u32,
    root: u32,
}

#[cfg(target_os = "linux")]
impl Platform {
    fn new(window: &Window) -> Result<Self, String> {
        use x11rb::connection::Connection;
        let window = match window
            .window_handle()
            .map_err(|error| error.to_string())?
            .as_raw()
        {
            RawWindowHandle::Xlib(handle) => handle.window as u32,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            _ => return Err("Floating windows require an X11 or XWayland display.".into()),
        };
        let (connection, screen) = x11rb::connect(None).map_err(|error| error.to_string())?;
        let root = connection.setup().roots[screen].root;
        Ok(Self {
            connection,
            window,
            root,
        })
    }

    fn apply(&self, rectangles: &[InputRect]) -> Result<(), String> {
        use x11rb::{
            connection::Connection,
            protocol::{
                shape::{ConnectionExt, SK, SO},
                xproto::{ClipOrdering, Rectangle},
            },
        };
        let rectangles: Vec<_> = rectangles
            .iter()
            .map(|rect| Rectangle {
                x: rect.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                y: rect.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                width: rect.width.min(u16::MAX as u32) as u16,
                height: rect.height.min(u16::MAX as u32) as u16,
            })
            .collect();
        self.connection
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                self.window,
                0,
                0,
                &rectangles,
            )
            .map_err(|error| error.to_string())?
            .check()
            .map_err(|error| error.to_string())?;
        self.connection.flush().map_err(|error| error.to_string())
    }

    fn work_area(&self) -> Option<Rect> {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
        let read = |name: &[u8], offset: u32, count: u32| {
            let atom = self
                .connection
                .intern_atom(true, name)
                .ok()?
                .reply()
                .ok()?
                .atom;
            let reply = self
                .connection
                .get_property(false, self.root, atom, AtomEnum::CARDINAL, offset, count)
                .ok()?
                .reply()
                .ok()?;
            Some(reply.value32()?.collect::<Vec<_>>())
        };
        let desktop = read(b"_NET_CURRENT_DESKTOP", 0, 1)?.first().copied()?;
        let work = read(b"_NET_WORKAREA", desktop * 4, 4)?;
        let [x, y, width, height] = work.as_slice() else {
            return None;
        };
        Some(Rect::from_min_size(
            egui::pos2(*x as i32 as f32, *y as i32 as f32),
            egui::vec2(*width as f32, *height as f32),
        ))
    }
}

#[cfg(windows)]
struct Platform(windows::Win32::Foundation::HWND);

#[cfg(windows)]
impl Platform {
    fn new(window: &Window) -> Result<Self, String> {
        match window
            .window_handle()
            .map_err(|error| error.to_string())?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => Ok(Self(windows::Win32::Foundation::HWND(
                handle.hwnd.get() as *mut _,
            ))),
            _ => Err("Unable to open the floating window.".into()),
        }
    }

    fn apply(&self, rectangles: &[InputRect]) -> Result<(), String> {
        use windows::Win32::Graphics::Gdi::{
            CombineRgn, CreateRectRgn, DeleteObject, RGN_ERROR, RGN_OR, SetWindowRgn,
        };
        unsafe {
            let combined = CreateRectRgn(0, 0, 0, 0);
            if combined.0.is_null() {
                return Err(windows::core::Error::from_win32().to_string());
            }
            for rect in rectangles {
                let part = CreateRectRgn(
                    rect.x,
                    rect.y,
                    rect.x + rect.width as i32,
                    rect.y + rect.height as i32,
                );
                let result = CombineRgn(Some(combined), Some(combined), Some(part), RGN_OR);
                let _ = DeleteObject(part.into());
                if result == RGN_ERROR {
                    let _ = DeleteObject(combined.into());
                    return Err(windows::core::Error::from_win32().to_string());
                }
            }
            // Windows owns the region after success. The excluded pixels are
            // outside the window, so clicks reach applications in other processes.
            if SetWindowRgn(self.0, Some(combined), true) == 0 {
                let _ = DeleteObject(combined.into());
                return Err(windows::core::Error::from_win32().to_string());
            }
        }
        Ok(())
    }

    fn work_area(&self) -> Option<Rect> {
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        };
        unsafe {
            let monitor = MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor, &mut info).ok().ok()?;
            let rect = info.rcWork;
            Some(Rect::from_min_max(
                egui::pos2(rect.left as f32, rect.top as f32),
                egui::pos2(rect.right as f32, rect.bottom as f32),
            ))
        }
    }
}
