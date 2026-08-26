//! Dynamic OpenVR / SteamVR loader and safe FFI interface for VR overlays.
//!
//! Connects to SteamVR using standard OpenVR exported entrypoints,
//! managing overlay lifetime and HMD tracking transform without hard dependencies.

use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const VR_APPLICATION_OVERLAY: i32 = 2;
pub const TRACKED_DEVICE_INDEX_HMD: u32 = 0;
pub const OVERLAY_HANDLE_INVALID: u64 = 0;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HmdMatrix34 {
    pub m: [[f32; 4]; 3],
}

impl HmdMatrix34 {
    /// Automatically calculates optimal pitch angle so the quad faces the viewer's eyes.
    pub fn auto_pitch_degrees(distance: f32, vertical_offset: f32) -> f32 {
        (-vertical_offset).atan2(distance.abs().max(0.1)).to_degrees()
    }

    /// Creates an HMD-locked transform matrix with automatically calculated pitch tilt.
    pub fn auto_hmd_hud(distance: f32, vertical_offset: f32) -> Self {
        let pitch_deg = Self::auto_pitch_degrees(distance, vertical_offset);
        Self::hmd_hud(distance, vertical_offset, pitch_deg)
    }

    /// Creates an HMD-locked transform matrix in front of the viewer with a tilt angle.
    pub fn hmd_hud(distance: f32, vertical_offset: f32, pitch_degrees: f32) -> Self {
        let rad = pitch_degrees.to_radians();
        let cos = rad.cos();
        let sin = rad.sin();

        // In OpenVR coordinate space:
        // +X is right, +Y is up, -Z is forward.
        Self {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, cos, sin, vertical_offset],
                [0.0, -sin, cos, -distance.abs().max(0.2)],
            ],
        }
    }
}

type FnVRInitInternal = unsafe extern "system" fn(*mut i32, i32) -> u32;
type FnVRShutdownInternal = unsafe extern "system" fn();
type FnVRIsRuntimeInstalled = unsafe extern "system" fn() -> bool;
type FnVRIsHmdPresent = unsafe extern "system" fn() -> bool;
type FnVRGetGenericInterface = unsafe extern "system" fn(*const c_char, *mut i32) -> *mut c_void;

#[repr(C)]
struct IVROverlayVtable {
    _find_overlay: usize, // 0
    create_overlay: unsafe extern "system" fn(*mut c_void, *const c_char, *const c_char, *mut u64) -> i32, // 1
    _create_subview_overlay: usize, // 2
    destroy_overlay: unsafe extern "system" fn(*mut c_void, u64) -> i32, // 3
    _get_overlay_key: usize, // 4
    _get_overlay_name: usize, // 5
    _set_overlay_name: usize, // 6
    _get_overlay_image_data: usize, // 7
    _get_overlay_error_name_from_enum: usize, // 8
    _set_overlay_rendering_pid: usize, // 9
    _get_overlay_rendering_pid: usize, // 10
    _set_overlay_flag: usize, // 11
    _get_overlay_flag: usize, // 12
    _get_overlay_flags: usize, // 13
    _set_overlay_color: usize, // 14
    _get_overlay_color: usize, // 15
    set_overlay_alpha: unsafe extern "system" fn(*mut c_void, u64, f32) -> i32, // 16
    _get_overlay_alpha: usize, // 17
    _set_overlay_texel_aspect: usize, // 18
    _get_overlay_texel_aspect: usize, // 19
    _set_overlay_sort_order: usize, // 20
    _get_overlay_sort_order: usize, // 21
    set_overlay_width_in_meters: unsafe extern "system" fn(*mut c_void, u64, f32) -> i32, // 22
    _get_overlay_width_in_meters: usize, // 23
    _set_overlay_curvature: usize, // 24
    _get_overlay_curvature: usize, // 25
    _set_overlay_pre_curve_pitch: usize, // 26
    _get_overlay_pre_curve_pitch: usize, // 27
    _set_overlay_texture_color_space: usize, // 28
    _get_overlay_texture_color_space: usize, // 29
    _set_overlay_texture_bounds: usize, // 30
    _get_overlay_texture_bounds: usize, // 31
    _get_overlay_transform_type: usize, // 32
    _set_overlay_transform_absolute: usize, // 33
    _get_overlay_transform_absolute: usize, // 34
    set_overlay_transform_tracked_device_relative: unsafe extern "system" fn(*mut c_void, u64, u32, *const HmdMatrix34) -> i32, // 35
    _get_overlay_transform_tracked_device_relative: usize, // 36
    _set_overlay_transform_tracked_device_component: usize, // 37
    _get_overlay_transform_tracked_device_component: usize, // 38
    _set_overlay_transform_cursor: usize, // 39
    _get_overlay_transform_cursor: usize, // 40
    _set_overlay_transform_projection: usize, // 41
    _set_subview_position: usize, // 42
    show_overlay: unsafe extern "system" fn(*mut c_void, u64) -> i32, // 43
    hide_overlay: unsafe extern "system" fn(*mut c_void, u64) -> i32, // 44
    _is_overlay_visible: usize, // 45
    _get_transform_for_overlay_coordinates: usize, // 46
    _wait_frame_sync: usize, // 47
    _poll_next_overlay_event: usize, // 48
    _get_overlay_input_method: usize, // 49
    _set_overlay_input_method: usize, // 50
    _get_overlay_mouse_scale: usize, // 51
    _set_overlay_mouse_scale: usize, // 52
    _compute_overlay_intersection: usize, // 53
    _is_hover_target_overlay: usize, // 54
    _set_overlay_intersection_mask: usize, // 55
    _trigger_laser_mouse_haptic_vibration: usize, // 56
    _set_overlay_cursor: usize, // 57
    _set_overlay_cursor_position_override: usize, // 58
    _clear_overlay_cursor_position_override: usize, // 59
    _set_overlay_texture: usize, // 60
    _clear_overlay_texture: usize, // 61
    set_overlay_raw: unsafe extern "system" fn(*mut c_void, u64, *const c_void, u32, u32, u32) -> i32, // 62
}

pub struct OpenVrApi {
    #[cfg(windows)]
    _module: windows::Win32::Foundation::HMODULE,
    vr_init: FnVRInitInternal,
    vr_shutdown: FnVRShutdownInternal,
    vr_is_runtime_installed: FnVRIsRuntimeInstalled,
    _vr_is_hmd_present: FnVRIsHmdPresent,
    vr_get_generic_interface: FnVRGetGenericInterface,
}

impl OpenVrApi {
    pub fn try_load() -> Option<Arc<Self>> {
        #[cfg(windows)]
        {
            use windows::Win32::System::LibraryLoader::LoadLibraryW;
            use windows::core::HSTRING;

            let paths = candidate_dll_paths();
            for path in &paths {
                if !path.is_file() {
                    continue;
                }
                let hstr = HSTRING::from(path.as_os_str());
                if let Ok(module) = unsafe { LoadLibraryW(&hstr) } {
                    if let Some(api) = Self::from_module(module) {
                        return Some(Arc::new(api));
                    }
                }
            }

            // Fallback to system search path
            if let Ok(module) = unsafe { LoadLibraryW(&HSTRING::from("openvr_api.dll")) } {
                if let Some(api) = Self::from_module(module) {
                    return Some(Arc::new(api));
                }
            }
        }
        None
    }

    #[cfg(windows)]
    fn from_module(module: windows::Win32::Foundation::HMODULE) -> Option<Self> {
        use windows::Win32::System::LibraryLoader::GetProcAddress;
        use windows::core::PCSTR;

        unsafe {
            macro_rules! get_sym {
                ($sym:literal) => {
                    std::mem::transmute(GetProcAddress(
                        module,
                        PCSTR::from_raw(concat!($sym, "\0").as_ptr()),
                    )?)
                };
            }

            Some(Self {
                _module: module,
                vr_init: get_sym!("VR_InitInternal"),
                vr_shutdown: get_sym!("VR_ShutdownInternal"),
                vr_is_runtime_installed: get_sym!("VR_IsRuntimeInstalled"),
                _vr_is_hmd_present: get_sym!("VR_IsHmdPresent"),
                vr_get_generic_interface: get_sym!("VR_GetGenericInterface"),
            })
        }
    }

    pub fn is_runtime_installed(&self) -> bool {
        unsafe { (self.vr_is_runtime_installed)() }
    }
}

/// Passively checks if the SteamVR runtime processes are actively running on the system.
/// This prevents XRTranslate from ever waking up or auto-launching SteamVR when the user
/// has not started it.
pub fn is_steamvr_running() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };

        unsafe {
            let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                Ok(h) => h,
                Err(_) => return false,
            };
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }

            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };

            let mut found = false;
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let exe_name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                    if exe_name == "vrserver.exe"
                        || exe_name == "vrmonitor.exe"
                        || exe_name == "vrcompositor.exe"
                    {
                        found = true;
                        break;
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }

            let _ = windows::Win32::Foundation::CloseHandle(snapshot);
            found
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl OpenVrApi {

    pub fn init_overlay(self: &Arc<Self>) -> Result<OpenVrSession, String> {
        let mut err = 0;
        let token = unsafe { (self.vr_init)(&mut err, VR_APPLICATION_OVERLAY) };
        if err != 0 || token == 0 {
            return Err(format!("OpenVR init failed with error code: {err}"));
        }

        // Fetch IVROverlay interface
        let mut overlay_interface: *mut c_void = std::ptr::null_mut();
        for (version_bytes, version_str) in [
            (b"IVROverlay_028\0".as_slice(), "IVROverlay_028"),
            (b"IVROverlay_027\0".as_slice(), "IVROverlay_027"),
            (b"IVROverlay_026\0".as_slice(), "IVROverlay_026"),
            (b"IVROverlay_025\0".as_slice(), "IVROverlay_025"),
            (b"IVROverlay_024\0".as_slice(), "IVROverlay_024"),
            (b"IVROverlay_021\0".as_slice(), "IVROverlay_021"),
            (b"IVROverlay_020\0".as_slice(), "IVROverlay_020"),
            (b"IVROverlay_019\0".as_slice(), "IVROverlay_019"),
        ] {
            let mut iface_err = 0;
            let ptr = unsafe { (self.vr_get_generic_interface)(version_bytes.as_ptr() as *const c_char, &mut iface_err) };
            if !ptr.is_null() && iface_err == 0 {
                overlay_interface = ptr;
                log::info!("[OpenVR] Acquired overlay interface: {version_str}");
                break;
            }
        }

        if overlay_interface.is_null() {
            unsafe { (self.vr_shutdown)() };
            return Err("Failed to get IVROverlay interface from SteamVR".into());
        }

        Ok(OpenVrSession {
            api: Arc::clone(self),
            overlay_interface,
            active: AtomicBool::new(true),
        })
    }
}

pub struct OpenVrSession {
    api: Arc<OpenVrApi>,
    overlay_interface: *mut c_void,
    active: AtomicBool,
}

impl Drop for OpenVrSession {
    fn drop(&mut self) {
        if self.active.swap(false, Ordering::SeqCst) {
            unsafe { (self.api.vr_shutdown)() };
        }
    }
}

impl OpenVrSession {
    pub fn create_overlay(&self, key: &str, name: &str) -> Result<OpenVrOverlay, String> {
        let c_key = CString::new(key).map_err(|e| e.to_string())?;
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let mut handle = OVERLAY_HANDLE_INVALID;

        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let create_fn = (*vtable).create_overlay;
            let err = create_fn(self.overlay_interface, c_key.as_ptr(), c_name.as_ptr(), &mut handle);
            if err != 0 || handle == OVERLAY_HANDLE_INVALID {
                return Err(format!("Failed to create OpenVR overlay (error: {err})"));
            }
        }

        Ok(OpenVrOverlay {
            _api: Arc::clone(&self.api),
            overlay_interface: self.overlay_interface,
            handle,
        })
    }
}

pub struct OpenVrOverlay {
    _api: Arc<OpenVrApi>,
    overlay_interface: *mut c_void,
    handle: u64,
}

impl Drop for OpenVrOverlay {
    fn drop(&mut self) {
        if self.handle != OVERLAY_HANDLE_INVALID && !self.overlay_interface.is_null() {
            unsafe {
                let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
                let destroy_fn = (*vtable).destroy_overlay;
                destroy_fn(self.overlay_interface, self.handle);
            }
        }
    }
}

impl OpenVrOverlay {
    pub fn set_auto_hmd_hud_transform(&self, distance: f32, vertical_offset: f32) {
        let matrix = HmdMatrix34::auto_hmd_hud(distance, vertical_offset);
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let set_transform_fn = (*vtable).set_overlay_transform_tracked_device_relative;
            set_transform_fn(
                self.overlay_interface,
                self.handle,
                TRACKED_DEVICE_INDEX_HMD,
                &matrix,
            );
        }
    }

    #[allow(dead_code)]
    pub fn set_hmd_hud_transform(&self, distance: f32, vertical_offset: f32, pitch_deg: f32) {
        let matrix = HmdMatrix34::hmd_hud(distance, vertical_offset, pitch_deg);
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let set_transform_fn = (*vtable).set_overlay_transform_tracked_device_relative;
            set_transform_fn(
                self.overlay_interface,
                self.handle,
                TRACKED_DEVICE_INDEX_HMD,
                &matrix,
            );
        }
    }

    pub fn set_width(&self, width_meters: f32) {
        let width = width_meters.clamp(0.2, 5.0);
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let set_width_fn = (*vtable).set_overlay_width_in_meters;
            set_width_fn(self.overlay_interface, self.handle, width);
        }
    }

    pub fn set_alpha(&self, alpha: f32) {
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let set_alpha_fn = (*vtable).set_overlay_alpha;
            set_alpha_fn(self.overlay_interface, self.handle, alpha.clamp(0.0, 1.0));
        }
    }

    pub fn set_raw_rgba(&self, buffer: &[u8], width: u32, height: u32) -> Result<(), String> {
        if buffer.len() != (width * height * 4) as usize {
            return Err("Invalid buffer length for raw RGBA overlay".into());
        }
        let err = unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let set_raw_fn = (*vtable).set_overlay_raw;
            set_raw_fn(
                self.overlay_interface,
                self.handle,
                buffer.as_ptr() as *const c_void,
                width,
                height,
                4,
            )
        };
        if err != 0 {
            return Err(format!("SetOverlayRaw failed with error code: {err}"));
        }
        Ok(())
    }

    pub fn show(&self) {
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let show_fn = (*vtable).show_overlay;
            show_fn(self.overlay_interface, self.handle);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let vtable = *(self.overlay_interface as *const *const IVROverlayVtable);
            let hide_fn = (*vtable).hide_overlay;
            hide_fn(self.overlay_interface, self.handle);
        }
    }
}

fn candidate_dll_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. App resources directory
    paths.push(PathBuf::from("resources/bin/openvr_api.dll"));
    paths.push(PathBuf::from("rust-client/resources/bin/openvr_api.dll"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join("openvr_api.dll"));
            paths.push(parent.join("resources/bin/openvr_api.dll"));
            if let Some(grandparent) = parent.parent() {
                paths.push(grandparent.join("resources/bin/openvr_api.dll"));
                paths.push(grandparent.join("rust-client/resources/bin/openvr_api.dll"));
            }
        }
    }

    // 2. Official OpenVR Configuration: %LOCALAPPDATA%\openvr\openvrpaths.vrpath
    #[cfg(windows)]
    {
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            let vrpath_file = Path::new(&local_appdata).join("openvr/openvrpaths.vrpath");
            if let Ok(content) = std::fs::read_to_string(&vrpath_file) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(runtimes) = json.get("runtime").and_then(|v| v.as_array()) {
                        for rt in runtimes {
                            if let Some(rt_str) = rt.as_str() {
                                paths.push(
                                    Path::new(rt_str).join("bin/win64/openvr_api.dll"),
                                );
                            }
                        }
                    }
                }
            }
        }

        // 3. Windows Registry SteamPath lookup (HKCU & HKLM)
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ, REG_SZ};
        use windows::core::{HSTRING, PCWSTR};

        unsafe fn query_reg_string(hkey: windows::Win32::System::Registry::HKEY, subkey: &windows::core::HSTRING, value_name: &windows::core::HSTRING) -> Option<String> {
            let mut key = windows::Win32::System::Registry::HKEY::default();
            unsafe {
                if RegOpenKeyExW(hkey, PCWSTR(subkey.as_ptr()), Some(0), KEY_READ, &mut key).is_ok() {
                    let mut data_type = REG_SZ;
                    let mut data_size: u32 = 512;
                    let mut buffer = vec![0u16; 256];
                    let res = RegQueryValueExW(
                        key,
                        PCWSTR(value_name.as_ptr()),
                        None,
                        Some(&mut data_type),
                        Some(buffer.as_mut_ptr() as *mut u8),
                        Some(&mut data_size),
                    );
                    let _ = RegCloseKey(key);
                    if res.is_ok() {
                        let len = (data_size / 2) as usize;
                        let valid_len = buffer[..len].iter().position(|&c| c == 0).unwrap_or(len);
                        return Some(String::from_utf16_lossy(&buffer[..valid_len]));
                    }
                }
            }
            None
        }

        for (root_key, subkey, val_name) in [
            (HKEY_CURRENT_USER, HSTRING::from("Software\\Valve\\Steam"), HSTRING::from("SteamPath")),
            (HKEY_LOCAL_MACHINE, HSTRING::from("SOFTWARE\\WOW6432Node\\Valve\\Steam"), HSTRING::from("InstallPath")),
            (HKEY_LOCAL_MACHINE, HSTRING::from("SOFTWARE\\Valve\\Steam"), HSTRING::from("InstallPath")),
        ] {
            if let Some(steam_dir) = unsafe { query_reg_string(root_key, &subkey, &val_name) } {
                let steam_path = PathBuf::from(steam_dir.replace('/', "\\"));
                paths.push(steam_path.join("steamapps/common/SteamVR/bin/win64/openvr_api.dll"));
            }
        }

        // 4. Standard Program Files and custom drive search
        for program_files in [
            std::env::var("ProgramFiles(x86)").ok(),
            std::env::var("ProgramFiles").ok(),
        ]
        .into_iter()
        .flatten()
        {
            paths.push(
                Path::new(&program_files)
                    .join("Steam/steamapps/common/SteamVR/bin/win64/openvr_api.dll"),
            );
        }

        for drive in ["C", "D", "E", "F", "G", "H"] {
            paths.push(PathBuf::from(format!(
                "{drive}:/SteamLibrary/steamapps/common/SteamVR/bin/win64/openvr_api.dll"
            )));
            paths.push(PathBuf::from(format!(
                "{drive}:/Steam/steamapps/common/SteamVR/bin/win64/openvr_api.dll"
            )));
            paths.push(PathBuf::from(format!(
                "{drive}:/app_install_path/Steam/steamapps/common/SteamVR/bin/win64/openvr_api.dll"
            )));
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmd_hud_matrix_has_expected_orientation_and_distance() {
        let matrix = HmdMatrix34::hmd_hud(1.2, -0.35, 15.0);
        // X offset is centered
        assert_eq!(matrix.m[0][3], 0.0);
        // Y offset is -0.35
        assert_eq!(matrix.m[1][3], -0.35);
        // Z offset is -1.2 (forward)
        assert_eq!(matrix.m[2][3], -1.2);
        // X axis unit vector
        assert_eq!(matrix.m[0][0], 1.0);
    }

    #[test]
    fn auto_pitch_calculates_sensible_tilt_angles() {
        // Below eye level: should tilt upwards (positive pitch)
        let pitch_below = HmdMatrix34::auto_pitch_degrees(1.2, -0.35);
        assert!(pitch_below > 10.0 && pitch_below < 20.0);

        // At eye level: pitch is zero
        let pitch_center = HmdMatrix34::auto_pitch_degrees(1.2, 0.0);
        assert!((pitch_center - 0.0).abs() < 0.001);

        // Above eye level: should tilt downwards (negative pitch)
        let pitch_above = HmdMatrix34::auto_pitch_degrees(1.2, 0.35);
        assert!(pitch_above < -10.0 && pitch_above > -20.0);
    }

    #[test]
    fn openvr_api_loads_and_detects_runtime() {
        let api = OpenVrApi::try_load();
        assert!(api.is_some(), "OpenVR API should be loadable on this system");
        if let Some(api) = api {
            let installed = api.is_runtime_installed();
            assert!(installed, "SteamVR runtime should be detected as installed");
            if installed {
                if let Ok(session) = api.init_overlay() {
                    if let Ok(overlay) = session.create_overlay("xrtranslate.test_overlay", "Test Overlay") {
                        overlay.set_width(1.0);
                        overlay.set_alpha(0.8);
                        overlay.set_auto_hmd_hud_transform(1.2, -0.35);
                        let buf = vec![255u8; 64 * 64 * 4];
                        let _ = overlay.set_raw_rgba(&buf, 64, 64);
                        overlay.show();
                        overlay.hide();
                    }
                }
            }
        }
    }
}
