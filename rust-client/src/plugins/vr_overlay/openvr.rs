//! Dynamic OpenVR / SteamVR loader and safe FFI interface for VR overlays.
//!
//! Connects to SteamVR using standard OpenVR exported entrypoints,
//! managing overlay lifetime and HMD tracking transform without hard dependencies.

use std::ffi::{CStr, CString, c_char, c_void};
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::{any::Any, cell::RefCell};

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
        (-vertical_offset)
            .atan2(distance.abs().max(0.1))
            .to_degrees()
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

// OpenVR exports use VR_CALLTYPE (__cdecl), unlike the FnTable (__stdcall).
type FnVRInitInternal = unsafe extern "C" fn(*mut i32, i32) -> u32;
type FnVRShutdownInternal = unsafe extern "C" fn();
type FnVRIsRuntimeInstalled = unsafe extern "C" fn() -> bool;
type FnVRIsHmdPresent = unsafe extern "C" fn() -> bool;
type FnVRGetGenericInterface = unsafe extern "C" fn(*const c_char, *mut i32) -> *mut c_void;

#[repr(C)]
// Exact prefix of Valve's VR_IVROverlay_FnTable for IVROverlay_028.
// Valve v2.15.6, commit 0924064316de3effbcd1acf1e309182a2deb1c05:
// https://github.com/ValveSoftware/openvr/blob/v2.15.6/headers/openvr_capi.h
// Request the C function table: no C++ `this` pointer or guessed older layouts.
struct IVROverlayFnTable {
    _find_overlay: usize, // 0
    create_overlay: unsafe extern "system" fn(*const c_char, *const c_char, *mut u64) -> i32, // 1
    _create_subview_overlay: usize, // 2
    destroy_overlay: unsafe extern "system" fn(u64) -> i32, // 3
    _get_overlay_key: usize, // 4
    _get_overlay_name: usize, // 5
    _set_overlay_name: usize, // 6
    _get_overlay_image_data: usize, // 7
    _get_overlay_error_name_from_enum: usize, // 8
    _set_overlay_rendering_pid: usize, // 9
    _get_overlay_rendering_pid: usize, // 10
    set_overlay_flag: unsafe extern "system" fn(u64, i32, bool) -> i32, // 11
    _get_overlay_flag: usize, // 12
    _get_overlay_flags: usize, // 13
    _set_overlay_color: usize, // 14
    _get_overlay_color: usize, // 15
    set_overlay_alpha: unsafe extern "system" fn(u64, f32) -> i32, // 16
    _get_overlay_alpha: usize, // 17
    _set_overlay_texel_aspect: usize, // 18
    _get_overlay_texel_aspect: usize, // 19
    _set_overlay_sort_order: usize, // 20
    _get_overlay_sort_order: usize, // 21
    set_overlay_width_in_meters: unsafe extern "system" fn(u64, f32) -> i32, // 22
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
    set_overlay_transform_absolute: unsafe extern "system" fn(u64, i32, *const HmdMatrix34) -> i32, // 33
    _get_overlay_transform_absolute: usize, // 34
    set_overlay_transform_tracked_device_relative:
        unsafe extern "system" fn(u64, u32, *const HmdMatrix34) -> i32, // 35
    _get_overlay_transform_tracked_device_relative: usize, // 36
    _set_overlay_transform_tracked_device_component: usize, // 37
    _get_overlay_transform_tracked_device_component: usize, // 38
    _set_overlay_transform_cursor: usize,   // 39
    _get_overlay_transform_cursor: usize,   // 40
    _set_overlay_transform_projection: usize, // 41
    _set_subview_position: usize,           // 42
    show_overlay: unsafe extern "system" fn(u64) -> i32, // 43
    hide_overlay: unsafe extern "system" fn(u64) -> i32, // 44
    _is_overlay_visible: usize,             // 45
    _get_transform_for_overlay_coordinates: usize, // 46
    _wait_frame_sync: usize,                // 47
    poll_next_overlay_event: unsafe extern "system" fn(u64, *mut OverlayEvent, u32) -> bool, // 48
    _get_overlay_input_method: usize,       // 49
    _set_overlay_input_method: usize,       // 50
    _get_overlay_mouse_scale: usize,        // 51
    _set_overlay_mouse_scale: usize,        // 52
    _compute_overlay_intersection: usize,   // 53
    _is_hover_target_overlay: usize,        // 54
    _set_overlay_intersection_mask: usize,  // 55
    _trigger_laser_mouse_haptic_vibration: usize, // 56
    _set_overlay_cursor: usize,             // 57
    _set_overlay_cursor_position_override: usize, // 58
    _clear_overlay_cursor_position_override: usize, // 59
    set_overlay_texture: unsafe extern "system" fn(u64, *const NativeTexture) -> i32, // 60
    _clear_overlay_texture: usize,          // 61
    set_overlay_raw: unsafe extern "system" fn(u64, *mut c_void, u32, u32, u32) -> i32, // 62
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
                    unsafe {
                        let _ = windows::Win32::Foundation::FreeLibrary(module);
                    }
                }
            }

            // Fallback to system search path
            if let Ok(module) = unsafe { LoadLibraryW(&HSTRING::from("openvr_api.dll")) } {
                if let Some(api) = Self::from_module(module) {
                    return Some(Arc::new(api));
                }
                unsafe {
                    let _ = windows::Win32::Foundation::FreeLibrary(module);
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

#[cfg(windows)]
impl Drop for OpenVrApi {
    fn drop(&mut self) {
        if !self._module.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::FreeLibrary(self._module);
            }
        }
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
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
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

        // A vtable layout is version-specific. Never reinterpret older interfaces
        // as 028: added methods shift native function slots.
        let mut iface_err = 0;
        let overlay_interface = unsafe {
            (self.vr_get_generic_interface)(c"FnTable:IVROverlay_028".as_ptr(), &mut iface_err)
        };

        if overlay_interface.is_null() || iface_err != 0 {
            unsafe { (self.vr_shutdown)() };
            return Err("SteamVR does not provide IVROverlay_028. Please update SteamVR.".into());
        }

        Ok(OpenVrSession {
            inner: Rc::new(OpenVrSessionInner {
                api: Arc::clone(self),
                overlay_interface,
                graphics: RefCell::new(Vec::new()),
            }),
        })
    }
}

// All native calls stay on the worker. An overlay keeps its session alive,
// even if the caller drops the session first; shutdown follows DestroyOverlay.
pub struct OpenVrSession {
    inner: Rc<OpenVrSessionInner>,
}

struct OpenVrSessionInner {
    api: Arc<OpenVrApi>,
    overlay_interface: *mut c_void,
    // Valve requires shutdown before releasing submitted GPU resources.
    graphics: RefCell<Vec<Box<dyn Any>>>,
}

impl Drop for OpenVrSessionInner {
    fn drop(&mut self) {
        unsafe { (self.api.vr_shutdown)() };
    }
}

// Exact IVRSystem_026 prefix (v2.15.6): these slots differ from older versions.
#[repr(C)]
struct IVRSystemFnTable {
    _render_size: usize,
    _projection: usize,
    _projection_raw: usize,
    _distortion: usize,
    _distortion_set: usize,
    eye_to_head: unsafe extern "system" fn(i32) -> HmdMatrix34, // 5
    _vsync: usize,
    _d3d9: usize,
    _dxgi: usize,
    output_device: unsafe extern "system" fn(*mut u64, i32, *mut c_void), // 9
    _on_desktop: usize,
    _display_visibility: usize,
    tracking_pose: unsafe extern "system" fn(i32, f32, *mut TrackedPose, u32), // 12
}
#[repr(C)]
struct IVRCompositorFnTable {
    _prefix: [usize; 41],
    instance_extensions: unsafe extern "system" fn(*mut c_char, u32) -> u32, // 41
    device_extensions: unsafe extern "system" fn(*mut c_void, *mut c_char, u32) -> u32, // 42
}
#[repr(C)]
#[derive(Default)]
struct TrackedPose {
    transform: HmdMatrix34,
    velocity: [f32; 3],
    angular_velocity: [f32; 3],
    result: i32,
    valid: bool,
    connected: bool,
}
#[repr(C)]
pub(super) struct VulkanTexture {
    pub image: u64,
    pub device: *mut c_void,
    pub physical_device: *mut c_void,
    pub instance: *mut c_void,
    pub queue: *mut c_void,
    pub queue_family: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub samples: u32,
}
#[repr(C)]
struct NativeTexture {
    handle: *mut c_void,
    texture_type: i32,
    color_space: i32,
}

// VREvent_t / VREvent_Data_t's reserved payload in the pinned OpenVR header.
#[repr(C)]
#[cfg_attr(unix, repr(packed(4)))]
#[derive(Default)]
struct OverlayEvent {
    event_type: u32,
    tracked_device_index: u32,
    age_seconds: f32,
    data: [u64; 6],
}

/// GPU overlays request these additional versioned interfaces.
pub(super) struct AvatarTracking {
    session: Rc<OpenVrSessionInner>,
    system: *const IVRSystemFnTable,
    compositor: *const IVRCompositorFnTable,
}
impl AvatarTracking {
    pub fn eyes_and_head(&self) -> Option<([glam::Mat4; 2], glam::Mat4)> {
        let table = unsafe { &*self.system };
        let mut head = TrackedPose::default();
        // Standing tracking origin, bounded 11 ms prediction. Placement is native;
        // this pose is used only for the mesh's actual view and spatial movement.
        unsafe { (table.tracking_pose)(1, 0.011, &mut head, 1) };
        if !head.valid || !head.connected {
            return None;
        }
        let head = head.transform.rigid_matrix()?;
        let eyes = [0, 1].map(|eye| unsafe { (table.eye_to_head)(eye) }.rigid_matrix());
        Some(([head * eyes[0]?, head * eyes[1]?], head))
    }
    pub fn output_device(&self, instance: *mut c_void) -> u64 {
        let mut device = 0;
        unsafe { ((*self.system).output_device)(&mut device, 2, instance) };
        device
    }
    pub fn extensions(&self, physical_device: Option<*mut c_void>) -> Result<Vec<CString>, String> {
        let table = unsafe { &*self.compositor };
        let query = |buffer, size| unsafe {
            match physical_device {
                Some(device) => (table.device_extensions)(device, buffer, size),
                None => (table.instance_extensions)(buffer, size),
            }
        };
        let size = query(std::ptr::null_mut(), 0);
        if size == 0 {
            return Ok(Vec::new());
        }
        if size > 16384 {
            return Err("Invalid SteamVR Vulkan extension list".into());
        }
        let mut buffer = vec![0u8; size as usize];
        if query(buffer.as_mut_ptr().cast(), size) > size {
            return Err("SteamVR extension list changed".into());
        }
        let value = CStr::from_bytes_until_nul(&buffer).map_err(|e| e.to_string())?;
        value
            .to_str()
            .map_err(|e| e.to_string())?
            .split_whitespace()
            .map(|s| CString::new(s).map_err(|e| e.to_string()))
            .collect()
    }
    pub fn retain_graphics(&self, owner: Box<dyn Any>) {
        self.session.graphics.borrow_mut().push(owner);
    }
}
impl Default for HmdMatrix34 {
    fn default() -> Self {
        Self { m: [[0.0; 4]; 3] }
    }
}
impl HmdMatrix34 {
    pub fn from_matrix(matrix: glam::Mat4) -> Self {
        let a = matrix.transpose().to_cols_array_2d();
        Self {
            m: [a[0], a[1], a[2]],
        }
    }
    pub fn rigid_matrix(&self) -> Option<glam::Mat4> {
        let matrix = glam::Mat4::from_cols_array(&[
            self.m[0][0],
            self.m[1][0],
            self.m[2][0],
            0.0,
            self.m[0][1],
            self.m[1][1],
            self.m[2][1],
            0.0,
            self.m[0][2],
            self.m[1][2],
            self.m[2][2],
            0.0,
            self.m[0][3],
            self.m[1][3],
            self.m[2][3],
            1.0,
        ]);
        let basis = glam::Mat3::from_mat4(matrix);
        (matrix.is_finite()
            && (basis.determinant() - 1.0).abs() < 0.02
            && (basis.transpose() * basis - glam::Mat3::IDENTITY)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < 0.02))
        .then_some(matrix)
    }
}

impl OpenVrSession {
    pub(super) fn avatar_tracking(&self) -> Result<AvatarTracking, String> {
        let interface = |name: &CStr| {
            let mut error = 0;
            let ptr =
                unsafe { (self.inner.api.vr_get_generic_interface)(name.as_ptr(), &mut error) };
            if ptr.is_null() || error != 0 {
                Err("SteamVR overlay interfaces unavailable; update SteamVR".to_owned())
            } else {
                Ok(ptr)
            }
        };
        Ok(AvatarTracking {
            session: Rc::clone(&self.inner),
            system: interface(c"FnTable:IVRSystem_026")?.cast(),
            compositor: interface(c"FnTable:IVRCompositor_029")?.cast(),
        })
    }
    pub fn create_overlay(&self, key: &str, name: &str) -> Result<OpenVrOverlay, String> {
        let c_key = CString::new(key).map_err(|e| e.to_string())?;
        let c_name = CString::new(name).map_err(|e| e.to_string())?;
        let mut handle = OVERLAY_HANDLE_INVALID;
        let table = unsafe { &*(self.inner.overlay_interface as *const IVROverlayFnTable) };
        let error = unsafe { (table.create_overlay)(c_key.as_ptr(), c_name.as_ptr(), &mut handle) };
        if error != 0 || handle == OVERLAY_HANDLE_INVALID {
            return Err(format!("Failed to create OpenVR overlay (error: {error})"));
        }
        let overlay = OpenVrOverlay {
            session: Rc::clone(&self.inner),
            handle,
            raw_frame: None,
        };
        check_overlay_error("SetOverlayFlag", unsafe {
            (table.set_overlay_flag)(handle, 1 << 21, false)
        })
        .map_err(|e| e.to_string())?;
        Ok(overlay)
    }
}

#[derive(Debug)]
pub enum OverlayError {
    Api(&'static str, i32),
    InvalidFrame(String),
}

impl OverlayError {
    pub fn connection_lost(&self) -> bool {
        // EVROverlayError_UnknownOverlay / InvalidHandle in the pinned header.
        matches!(self, Self::Api(_, 10 | 11))
    }
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(operation, code) => write!(f, "{operation} failed with error code: {code}"),
            Self::InvalidFrame(message) => f.write_str(message),
        }
    }
}

fn check_overlay_error(operation: &'static str, code: i32) -> Result<(), OverlayError> {
    if code == 0 {
        Ok(())
    } else {
        Err(OverlayError::Api(operation, code))
    }
}

// Application budget, not a claim about SteamVR's undocumented raw-byte limit.
// Current subtitles are only 640x320 (819,200 bytes), independent of line count.
const MAX_RAW_DIMENSION: u32 = 1024;
const MAX_RAW_RGBA_BYTES: usize = 4 * 1024 * 1024;
pub(super) fn raw_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    if width > MAX_RAW_DIMENSION || height > MAX_RAW_DIMENSION {
        return Err("Raw RGBA overlay dimensions exceed the application budget".into());
    }
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n > 0 && n <= MAX_RAW_RGBA_BYTES);
    len.ok_or_else(|| "Invalid or oversized raw RGBA overlay dimensions".into())
}

pub struct OpenVrOverlay {
    session: Rc<OpenVrSessionInner>,
    handle: u64,
    // Keep accepted CPU storage through replacement or native destruction.
    // SetOverlayRaw has no row-stride parameter: bytes are tightly packed RGBA.
    raw_frame: Option<Vec<u8>>,
}

impl Drop for OpenVrOverlay {
    fn drop(&mut self) {
        let error = unsafe { (self.table().destroy_overlay)(self.handle) };
        if error != 0 {
            log::warn!("DestroyOverlay failed: {error}");
        }
        // Fields (including the frame and last session reference) drop afterwards.
    }
}

impl OpenVrOverlay {
    pub(super) fn set_world_transform(&self, matrix: glam::Mat4) -> Result<(), OverlayError> {
        let transform = HmdMatrix34::from_matrix(matrix);
        if transform.rigid_matrix().is_none() {
            return Err(OverlayError::InvalidFrame(
                "Invalid avatar transform".into(),
            ));
        }
        check_overlay_error("SetOverlayTransformAbsolute", unsafe {
            (self.table().set_overlay_transform_absolute)(self.handle, 1, &transform)
        })
    }
    pub(super) fn configure_stereo(&self) -> Result<(), OverlayError> {
        for flag in [1 << 10, 1 << 21] {
            check_overlay_error("SetOverlayFlag", unsafe {
                (self.table().set_overlay_flag)(self.handle, flag, true)
            })?;
        }
        Ok(())
    }
    pub(super) fn set_vulkan_texture(&self, data: &mut VulkanTexture) -> Result<(), OverlayError> {
        if data.image == 0
            || [data.device, data.physical_device, data.instance, data.queue]
                .iter()
                .any(|p| p.is_null())
            || raw_rgba_len(data.width, data.height).is_err()
            || data.samples != 1
            || data.format != 37
        {
            return Err(OverlayError::InvalidFrame(
                "Invalid overlay GPU texture".into(),
            ));
        }
        let texture = NativeTexture {
            handle: (data as *mut VulkanTexture).cast(),
            texture_type: 2,
            // RGBA8 pixels are sRGB; the overlay flag separately selects whether
            // alpha is straight (text) or premultiplied (stereo composition).
            color_space: 1,
        };
        self.drain_events();
        check_overlay_error("SetOverlayTexture", unsafe {
            (self.table().set_overlay_texture)(self.handle, &texture)
        })
    }
    fn table(&self) -> &IVROverlayFnTable {
        unsafe { &*(self.session.overlay_interface as *const IVROverlayFnTable) }
    }

    fn drain_events(&self) {
        // SteamVR queues ImageLoaded for raw uploads. Leaving those unread
        // exhausts its queue after about 200 updates and makes uploads fail.
        let mut event = OverlayEvent::default();
        while unsafe {
            (self.table().poll_next_overlay_event)(
                self.handle,
                &mut event,
                std::mem::size_of::<OverlayEvent>() as u32,
            )
        } {}
    }

    pub fn set_auto_hmd_hud_transform(
        &self,
        distance: f32,
        vertical_offset: f32,
    ) -> Result<(), OverlayError> {
        if ![distance, vertical_offset].iter().all(|x| x.is_finite()) {
            return Err(OverlayError::InvalidFrame(
                "Invalid overlay transform".into(),
            ));
        }
        self.set_transform_matrix(&HmdMatrix34::auto_hmd_hud(distance, vertical_offset))
    }

    #[allow(dead_code)]
    pub fn set_hmd_hud_transform(
        &self,
        distance: f32,
        vertical_offset: f32,
        pitch_deg: f32,
    ) -> Result<(), OverlayError> {
        if ![distance, vertical_offset, pitch_deg]
            .iter()
            .all(|x| x.is_finite())
        {
            return Err(OverlayError::InvalidFrame(
                "Invalid overlay transform".into(),
            ));
        }
        self.set_transform_matrix(&HmdMatrix34::hmd_hud(distance, vertical_offset, pitch_deg))
    }

    fn set_transform_matrix(&self, matrix: &HmdMatrix34) -> Result<(), OverlayError> {
        check_overlay_error("SetOverlayTransformTrackedDeviceRelative", unsafe {
            (self.table().set_overlay_transform_tracked_device_relative)(
                self.handle,
                TRACKED_DEVICE_INDEX_HMD,
                matrix,
            )
        })
    }

    pub fn set_width(&self, width_meters: f32) -> Result<(), OverlayError> {
        if !width_meters.is_finite() {
            return Err(OverlayError::InvalidFrame("Invalid overlay width".into()));
        }
        check_overlay_error("SetOverlayWidthInMeters", unsafe {
            (self.table().set_overlay_width_in_meters)(self.handle, width_meters.clamp(0.2, 5.0))
        })
    }

    pub fn set_alpha(&self, alpha: f32) -> Result<(), OverlayError> {
        if !alpha.is_finite() {
            return Err(OverlayError::InvalidFrame("Invalid overlay alpha".into()));
        }
        check_overlay_error("SetOverlayAlpha", unsafe {
            (self.table().set_overlay_alpha)(self.handle, alpha.clamp(0.0, 1.0))
        })
    }

    pub fn set_raw_rgba(
        &mut self,
        mut buffer: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<(), OverlayError> {
        let len = raw_rgba_len(width, height).map_err(OverlayError::InvalidFrame)?;
        if buffer.len() != len {
            return Err(OverlayError::InvalidFrame(
                "Invalid buffer length for raw RGBA overlay".into(),
            ));
        }
        self.drain_events();
        check_overlay_error("SetOverlayRaw", unsafe {
            (self.table().set_overlay_raw)(
                self.handle,
                buffer.as_mut_ptr().cast(),
                width,
                height,
                4,
            )
        })?;
        self.raw_frame = Some(buffer);
        Ok(())
    }

    pub fn show(&self) -> Result<(), OverlayError> {
        check_overlay_error("ShowOverlay", unsafe {
            (self.table().show_overlay)(self.handle)
        })
    }

    pub fn hide(&self) -> Result<(), OverlayError> {
        check_overlay_error("HideOverlay", unsafe {
            (self.table().hide_overlay)(self.handle)
        })
    }
}

#[cfg(windows)]
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
                                paths.push(Path::new(rt_str).join("bin/win64/openvr_api.dll"));
                            }
                        }
                    }
                }
            }
        }

        // 3. Windows Registry SteamPath lookup (HKCU & HKLM)
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW,
            RegQueryValueExW,
        };
        use windows::core::{HSTRING, PCWSTR};

        unsafe fn query_reg_string(
            hkey: windows::Win32::System::Registry::HKEY,
            subkey: &windows::core::HSTRING,
            value_name: &windows::core::HSTRING,
        ) -> Option<String> {
            let mut key = windows::Win32::System::Registry::HKEY::default();
            unsafe {
                if RegOpenKeyExW(hkey, PCWSTR(subkey.as_ptr()), Some(0), KEY_READ, &mut key).is_ok()
                {
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
            (
                HKEY_CURRENT_USER,
                HSTRING::from("Software\\Valve\\Steam"),
                HSTRING::from("SteamPath"),
            ),
            (
                HKEY_LOCAL_MACHINE,
                HSTRING::from("SOFTWARE\\WOW6432Node\\Valve\\Steam"),
                HSTRING::from("InstallPath"),
            ),
            (
                HKEY_LOCAL_MACHINE,
                HSTRING::from("SOFTWARE\\Valve\\Steam"),
                HSTRING::from("InstallPath"),
            ),
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
pub(super) mod tests {
    use super::*;

    use std::sync::{Mutex, MutexGuard, OnceLock};
    static NATIVE_TEST_LOCK: Mutex<()> = Mutex::new(());
    #[derive(Default)]
    struct FakeNative {
        calls: Vec<&'static str>,
        failure: Option<(&'static str, i32)>,
        missing_interface: bool,
        missing_avatar: bool,
        invalid_pose: bool,
        raw_pointer: usize,
        raw_dimensions: (u32, u32, u32),
    }
    static NATIVE: Mutex<FakeNative> = Mutex::new(FakeNative {
        calls: Vec::new(),
        failure: None,
        missing_interface: false,
        missing_avatar: false,
        invalid_pose: false,
        raw_pointer: 0,
        raw_dimensions: (0, 0, 0),
    });
    fn call(operation: &'static str) -> i32 {
        let mut state = NATIVE.lock().unwrap();
        state.calls.push(operation);
        state
            .failure
            .filter(|(name, _)| *name == operation)
            .map_or(0, |(_, code)| code)
    }
    unsafe extern "C" fn fake_init(error: *mut i32, application: i32) -> u32 {
        assert_eq!(application, VR_APPLICATION_OVERLAY);
        unsafe {
            *error = 0;
        }
        call("Init");
        1
    }
    unsafe extern "C" fn fake_shutdown() {
        call("Shutdown");
    }
    unsafe extern "C" fn fake_installed() -> bool {
        true
    }
    unsafe extern "C" fn fake_interface(name: *const c_char, error: *mut i32) -> *mut c_void {
        let name = unsafe { CStr::from_ptr(name) };
        if NATIVE.lock().unwrap().missing_interface {
            unsafe {
                *error = 105;
            }
            return std::ptr::null_mut();
        }
        unsafe {
            *error = 0;
        }
        if name != c"FnTable:IVROverlay_028" && NATIVE.lock().unwrap().missing_avatar {
            unsafe {
                *error = 105;
            }
            return std::ptr::null_mut();
        }
        match name {
            n if n == c"FnTable:IVROverlay_028" => table() as *const _ as *mut c_void,
            n if n == c"FnTable:IVRSystem_026" => system_table() as *const _ as *mut c_void,
            n if n == c"FnTable:IVRCompositor_029" => compositor_table() as *const _ as *mut c_void,
            _ => panic!("Unexpected interface request"),
        }
    }
    unsafe extern "system" fn fake_create(
        _: *const c_char,
        _: *const c_char,
        handle: *mut u64,
    ) -> i32 {
        unsafe {
            *handle = 1;
        }
        call("CreateOverlay")
    }
    unsafe extern "system" fn fake_destroy(handle: u64) -> i32 {
        assert_eq!(handle, 1);
        call("DestroyOverlay")
    }
    unsafe extern "system" fn fake_flag(_: u64, flag: i32, value: bool) -> i32 {
        assert!(flag == 1 << 21 || (flag == 1 << 10 && value));
        call("SetOverlayFlag")
    }
    unsafe extern "system" fn fake_alpha(_: u64, value: f32) -> i32 {
        assert!(value.is_finite());
        call("SetOverlayAlpha")
    }
    unsafe extern "system" fn fake_width(_: u64, value: f32) -> i32 {
        assert!(value.is_finite());
        call("SetOverlayWidthInMeters")
    }
    unsafe extern "system" fn fake_transform(
        _: u64,
        device: u32,
        matrix: *const HmdMatrix34,
    ) -> i32 {
        assert_eq!(device, 0);
        assert!(
            unsafe { &*matrix }
                .m
                .iter()
                .flatten()
                .all(|x| x.is_finite())
        );
        call("SetOverlayTransformTrackedDeviceRelative")
    }
    unsafe extern "system" fn fake_show(_: u64) -> i32 {
        call("ShowOverlay")
    }
    unsafe extern "system" fn fake_hide(_: u64) -> i32 {
        call("HideOverlay")
    }
    unsafe extern "system" fn fake_raw(
        _: u64,
        buffer: *mut c_void,
        w: u32,
        h: u32,
        bpp: u32,
    ) -> i32 {
        assert!(!buffer.is_null());
        let mut state = NATIVE.lock().unwrap();
        state.raw_pointer = buffer as usize;
        state.raw_dimensions = (w, h, bpp);
        drop(state);
        call("SetOverlayRaw")
    }
    unsafe extern "system" fn fake_eye(eye: i32) -> HmdMatrix34 {
        HmdMatrix34::from_matrix(glam::Mat4::from_translation(
            glam::Vec3::X * if eye == 0 { -0.032 } else { 0.032 },
        ))
    }
    unsafe extern "system" fn fake_tracking(
        origin: i32,
        prediction: f32,
        pose: *mut TrackedPose,
        count: u32,
    ) {
        assert_eq!(origin, 1);
        assert_eq!(count, 1);
        assert!((0.0..=0.02).contains(&prediction));
        unsafe {
            *pose = TrackedPose {
                transform: HmdMatrix34::from_matrix(glam::Mat4::from_translation(
                    glam::Vec3::Y * 1.6,
                )),
                valid: !NATIVE.lock().unwrap().invalid_pose,
                connected: true,
                ..Default::default()
            };
        }
    }
    unsafe extern "system" fn fake_output(device: *mut u64, _: i32, _: *mut c_void) {
        unsafe {
            *device = 123;
        }
    }
    unsafe extern "system" fn fake_instance_extensions(buffer: *mut c_char, size: u32) -> u32 {
        let value = c"VK_KHR_external_memory_capabilities".to_bytes_with_nul();
        if size >= value.len() as u32 {
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), buffer.cast(), value.len());
            }
        }
        value.len() as u32
    }
    unsafe extern "system" fn fake_device_extensions(
        _: *mut c_void,
        buffer: *mut c_char,
        size: u32,
    ) -> u32 {
        fake_instance_extensions_call(buffer, size)
    }
    fn fake_instance_extensions_call(buffer: *mut c_char, size: u32) -> u32 {
        unsafe { fake_instance_extensions(buffer, size) }
    }
    fn system_table() -> &'static IVRSystemFnTable {
        static TABLE: IVRSystemFnTable = IVRSystemFnTable {
            _render_size: 0,
            _projection: 0,
            _projection_raw: 0,
            _distortion: 0,
            _distortion_set: 0,
            eye_to_head: fake_eye,
            _vsync: 0,
            _d3d9: 0,
            _dxgi: 0,
            output_device: fake_output,
            _on_desktop: 0,
            _display_visibility: 0,
            tracking_pose: fake_tracking,
        };
        &TABLE
    }
    fn compositor_table() -> &'static IVRCompositorFnTable {
        static TABLE: IVRCompositorFnTable = IVRCompositorFnTable {
            _prefix: [0; 41],
            instance_extensions: fake_instance_extensions,
            device_extensions: fake_device_extensions,
        };
        &TABLE
    }
    unsafe extern "system" fn fake_absolute(
        _: u64,
        origin: i32,
        matrix: *const HmdMatrix34,
    ) -> i32 {
        assert_eq!(origin, 1);
        assert!(unsafe { &*matrix }.rigid_matrix().is_some());
        call("SetOverlayTransformAbsolute")
    }
    unsafe extern "system" fn fake_poll(_: u64, _: *mut OverlayEvent, size: u32) -> bool {
        assert_eq!(size as usize, std::mem::size_of::<OverlayEvent>());
        call("PollNextOverlayEvent");
        false
    }
    unsafe extern "system" fn fake_texture(_: u64, texture: *const NativeTexture) -> i32 {
        let texture = unsafe { &*texture };
        assert_eq!(texture.texture_type, 2);
        assert_eq!(texture.color_space, 1);
        call("SetOverlayTexture")
    }
    fn table() -> &'static IVROverlayFnTable {
        static TABLE: OnceLock<IVROverlayFnTable> = OnceLock::new();
        TABLE.get_or_init(|| IVROverlayFnTable {
            _find_overlay: 0,
            create_overlay: fake_create,
            _create_subview_overlay: 0,
            destroy_overlay: fake_destroy,
            _get_overlay_key: 0,
            _get_overlay_name: 0,
            _set_overlay_name: 0,
            _get_overlay_image_data: 0,
            _get_overlay_error_name_from_enum: 0,
            _set_overlay_rendering_pid: 0,
            _get_overlay_rendering_pid: 0,
            set_overlay_flag: fake_flag,
            _get_overlay_flag: 0,
            _get_overlay_flags: 0,
            _set_overlay_color: 0,
            _get_overlay_color: 0,
            set_overlay_alpha: fake_alpha,
            _get_overlay_alpha: 0,
            _set_overlay_texel_aspect: 0,
            _get_overlay_texel_aspect: 0,
            _set_overlay_sort_order: 0,
            _get_overlay_sort_order: 0,
            set_overlay_width_in_meters: fake_width,
            _get_overlay_width_in_meters: 0,
            _set_overlay_curvature: 0,
            _get_overlay_curvature: 0,
            _set_overlay_pre_curve_pitch: 0,
            _get_overlay_pre_curve_pitch: 0,
            _set_overlay_texture_color_space: 0,
            _get_overlay_texture_color_space: 0,
            _set_overlay_texture_bounds: 0,
            _get_overlay_texture_bounds: 0,
            _get_overlay_transform_type: 0,
            set_overlay_transform_absolute: fake_absolute,
            _get_overlay_transform_absolute: 0,
            set_overlay_transform_tracked_device_relative: fake_transform,
            _get_overlay_transform_tracked_device_relative: 0,
            _set_overlay_transform_tracked_device_component: 0,
            _get_overlay_transform_tracked_device_component: 0,
            _set_overlay_transform_cursor: 0,
            _get_overlay_transform_cursor: 0,
            _set_overlay_transform_projection: 0,
            _set_subview_position: 0,
            show_overlay: fake_show,
            hide_overlay: fake_hide,
            _is_overlay_visible: 0,
            _get_transform_for_overlay_coordinates: 0,
            _wait_frame_sync: 0,
            poll_next_overlay_event: fake_poll,
            _get_overlay_input_method: 0,
            _set_overlay_input_method: 0,
            _get_overlay_mouse_scale: 0,
            _set_overlay_mouse_scale: 0,
            _compute_overlay_intersection: 0,
            _is_hover_target_overlay: 0,
            _set_overlay_intersection_mask: 0,
            _trigger_laser_mouse_haptic_vibration: 0,
            _set_overlay_cursor: 0,
            _set_overlay_cursor_position_override: 0,
            _clear_overlay_cursor_position_override: 0,
            set_overlay_texture: fake_texture,
            _clear_overlay_texture: 0,
            set_overlay_raw: fake_raw,
        })
    }
    fn api() -> Arc<OpenVrApi> {
        Arc::new(OpenVrApi {
            #[cfg(windows)]
            _module: windows::Win32::Foundation::HMODULE::default(),
            vr_init: fake_init,
            vr_shutdown: fake_shutdown,
            vr_is_runtime_installed: fake_installed,
            _vr_is_hmd_present: fake_installed,
            vr_get_generic_interface: fake_interface,
        })
    }
    pub(in crate::plugins::vr_overlay) fn fake_session() -> (MutexGuard<'static, ()>, OpenVrSession)
    {
        let lock = NATIVE_TEST_LOCK.lock().unwrap();
        *NATIVE.lock().unwrap() = FakeNative::default();
        (lock, api().init_overlay().unwrap())
    }
    pub(in crate::plugins::vr_overlay) fn fail_operation(operation: &'static str, code: i32) {
        NATIVE.lock().unwrap().failure = Some((operation, code));
    }

    #[test]
    fn raw_dimensions_reject_zero_overflow_and_large_allocations() {
        assert_eq!(raw_rgba_len(640, 320).unwrap(), 819200);
        for (w, h) in [
            (0, 320),
            (640, 0),
            (u32::MAX, u32::MAX),
            (u32::MAX, 2),
            (2048, 2048),
            (1_000_000, 1),
            (1, 1_000_000),
        ] {
            assert!(raw_rgba_len(w, h).is_err());
        }
    }

    #[test]
    fn overlay_owns_session_and_accepted_buffer_until_native_destruction() {
        let (_lock, session) = fake_session();
        let mut overlay = session.create_overlay("test", "test").unwrap();
        drop(session);
        assert!(!NATIVE.lock().unwrap().calls.contains(&"Shutdown"));
        let first = vec![7; 16];
        let pointer = first.as_ptr() as usize;
        overlay.set_raw_rgba(first, 2, 2).unwrap();
        assert_eq!(NATIVE.lock().unwrap().raw_pointer, pointer);
        assert_eq!(NATIVE.lock().unwrap().raw_dimensions, (2, 2, 4));
        fail_operation("SetOverlayRaw", 23);
        assert!(overlay.set_raw_rgba(vec![9; 16], 2, 2).is_err());
        assert_eq!(overlay.raw_frame.as_ref().unwrap(), &vec![7; 16]);
        let count = NATIVE.lock().unwrap().calls.len();
        assert!(overlay.set_raw_rgba(vec![1; 15], 2, 2).is_err());
        assert!(overlay.set_raw_rgba(vec![], u32::MAX, u32::MAX).is_err());
        assert_eq!(NATIVE.lock().unwrap().calls.len(), count);
        drop(overlay);
        let state = NATIVE.lock().unwrap();
        assert_eq!(
            &state.calls[state.calls.len() - 2..],
            &["DestroyOverlay", "Shutdown"]
        );
    }

    #[test]
    fn every_used_native_setter_and_visibility_error_is_propagated() {
        let (_lock, session) = fake_session();
        let overlay = session.create_overlay("test", "test").unwrap();
        fail_operation("SetOverlayTransformTrackedDeviceRelative", 23);
        assert!(overlay.set_auto_hmd_hud_transform(1.0, 0.0).is_err());
        fail_operation("SetOverlayWidthInMeters", 23);
        assert!(overlay.set_width(1.0).is_err());
        fail_operation("SetOverlayAlpha", 23);
        assert!(overlay.set_alpha(1.0).is_err());
        fail_operation("ShowOverlay", 11);
        assert!(overlay.show().unwrap_err().connection_lost());
        fail_operation("HideOverlay", 10);
        assert!(overlay.hide().unwrap_err().connection_lost());
        let count = NATIVE.lock().unwrap().calls.len();
        assert!(overlay.set_alpha(f32::NAN).is_err());
        assert!(overlay.set_width(f32::INFINITY).is_err());
        assert!(overlay.set_hmd_hud_transform(1.0, f32::NAN, 0.0).is_err());
        assert_eq!(NATIVE.lock().unwrap().calls.len(), count);
    }

    #[test]
    fn missing_exact_interface_and_failed_create_setup_release_session() {
        let (_lock, session) = fake_session();
        drop(session);
        *NATIVE.lock().unwrap() = FakeNative {
            missing_interface: true,
            ..Default::default()
        };
        assert!(api().init_overlay().is_err());
        assert_eq!(NATIVE.lock().unwrap().calls, vec!["Init", "Shutdown"]);
        *NATIVE.lock().unwrap() = FakeNative::default();
        let session = api().init_overlay().unwrap();
        fail_operation("SetOverlayFlag", 23);
        assert!(session.create_overlay("test", "test").is_err());
        drop(session);
        let state = NATIVE.lock().unwrap();
        assert_eq!(
            &state.calls[state.calls.len() - 2..],
            &["DestroyOverlay", "Shutdown"]
        );
    }

    #[test]
    fn avatar_tracking_uses_eye_transforms_and_hides_invalid_tracking() {
        let (_lock, session) = fake_session();
        let tracking = session.avatar_tracking().unwrap();
        let (eyes, head) = tracking.eyes_and_head().unwrap();
        assert!((eyes[1].w_axis.x - eyes[0].w_axis.x - 0.064).abs() < 0.00001);
        assert_eq!(head.w_axis.y, 1.6);
        assert_eq!(tracking.output_device(std::ptr::null_mut()), 123);
        assert_eq!(
            tracking.extensions(None).unwrap()[0].as_c_str(),
            c"VK_KHR_external_memory_capabilities"
        );
        NATIVE.lock().unwrap().invalid_pose = true;
        assert!(tracking.eyes_and_head().is_none());
    }
    #[test]
    fn avatar_interfaces_are_optional_and_graphics_outlive_shutdown() {
        let (_lock, session) = fake_session();
        NATIVE.lock().unwrap().missing_avatar = true;
        assert!(session.avatar_tracking().is_err());
        let overlay = session.create_overlay("caption", "caption").unwrap();
        NATIVE.lock().unwrap().missing_avatar = false;
        let tracking = session.avatar_tracking().unwrap();
        struct Graphics;
        impl Drop for Graphics {
            fn drop(&mut self) {
                call("FreeGraphics");
            }
        }
        tracking.retain_graphics(Box::new(Graphics));
        drop(tracking);
        drop(session);
        assert!(!NATIVE.lock().unwrap().calls.contains(&"Shutdown"));
        drop(overlay);
        let state = NATIVE.lock().unwrap();
        assert_eq!(
            &state.calls[state.calls.len() - 3..],
            &["DestroyOverlay", "Shutdown", "FreeGraphics"]
        );
    }
    #[test]
    fn stereo_texture_and_world_transform_validate_and_propagate_native_failures() {
        let (_lock, session) = fake_session();
        let overlay = session.create_overlay("avatar", "avatar").unwrap();
        overlay.configure_stereo().unwrap();
        let world = glam::Mat4::from_rotation_translation(
            glam::Quat::from_rotation_y(0.3),
            glam::Vec3::new(0.4, 1.2, -1.0),
        );
        overlay.set_world_transform(world).unwrap();
        assert!(
            HmdMatrix34::from_matrix(world)
                .rigid_matrix()
                .unwrap()
                .abs_diff_eq(world, 0.00001)
        );
        assert!(
            overlay
                .set_world_transform(glam::Mat4::from_scale(glam::Vec3::splat(2.0)))
                .is_err()
        );
        let mut texture = VulkanTexture {
            image: 1,
            device: 1usize as *mut _,
            physical_device: 2usize as *mut _,
            instance: 3usize as *mut _,
            queue: 4usize as *mut _,
            queue_family: 0,
            width: 1024,
            height: 512,
            format: 37,
            samples: 1,
        };
        overlay.set_vulkan_texture(&mut texture).unwrap();
        texture.width = 640;
        texture.height = 320;
        overlay.set_vulkan_texture(&mut texture).unwrap();
        fail_operation("SetOverlayTexture", 24);
        assert!(matches!(
            overlay.set_vulkan_texture(&mut texture),
            Err(OverlayError::Api(_, 24))
        ));
        texture.width = 0;
        let count = NATIVE.lock().unwrap().calls.len();
        assert!(overlay.set_vulkan_texture(&mut texture).is_err());
        assert_eq!(NATIVE.lock().unwrap().calls.len(), count);
    }
    #[test]
    fn avatar_ffi_slots_and_payload_match_pinned_c_header() {
        use std::mem::{offset_of, size_of};
        let slot = size_of::<usize>();
        assert_eq!(offset_of!(IVRSystemFnTable, eye_to_head), 5 * slot);
        assert_eq!(offset_of!(IVRSystemFnTable, output_device), 9 * slot);
        assert_eq!(offset_of!(IVRSystemFnTable, tracking_pose), 12 * slot);
        assert_eq!(
            offset_of!(IVRCompositorFnTable, instance_extensions),
            41 * slot
        );
        assert_eq!(
            offset_of!(IVRCompositorFnTable, device_extensions),
            42 * slot
        );
        assert_eq!(
            offset_of!(IVROverlayFnTable, set_overlay_transform_absolute),
            33 * slot
        );
        assert_eq!(offset_of!(IVROverlayFnTable, _wait_frame_sync), 47 * slot);
        assert_eq!(
            offset_of!(IVROverlayFnTable, set_overlay_texture),
            60 * slot
        );
        assert_eq!(size_of::<TrackedPose>(), 80);
        assert_eq!(offset_of!(TrackedPose, valid), 76);
        assert_eq!(offset_of!(VulkanTexture, queue_family), 5 * slot);
        assert_eq!(size_of::<NativeTexture>(), slot + 8);
    }

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
    fn overlay_028_function_table_matches_valve_header_offsets() {
        use std::mem::{offset_of, size_of};
        let slot = size_of::<usize>();
        assert_eq!(offset_of!(IVROverlayFnTable, create_overlay), slot);
        assert_eq!(offset_of!(IVROverlayFnTable, destroy_overlay), 3 * slot);
        assert_eq!(offset_of!(IVROverlayFnTable, set_overlay_flag), 11 * slot);
        assert_eq!(offset_of!(IVROverlayFnTable, set_overlay_alpha), 16 * slot);
        assert_eq!(
            offset_of!(IVROverlayFnTable, set_overlay_width_in_meters),
            22 * slot
        );
        assert_eq!(
            offset_of!(
                IVROverlayFnTable,
                set_overlay_transform_tracked_device_relative
            ),
            35 * slot
        );
        assert_eq!(offset_of!(IVROverlayFnTable, show_overlay), 43 * slot);
        assert_eq!(offset_of!(IVROverlayFnTable, hide_overlay), 44 * slot);
        assert_eq!(offset_of!(IVROverlayFnTable, set_overlay_raw), 62 * slot);
        assert_eq!(size_of::<HmdMatrix34>(), 12 * size_of::<f32>());
    }

    #[test]
    #[ignore = "Requires installed SteamVR, a connected headset, and explicit hardware testing"]
    fn openvr_api_loads_and_detects_runtime() {
        use super::super::graphics;
        let api = OpenVrApi::try_load().expect("OpenVR API should be loadable on this system");
        assert!(api.is_runtime_installed(), "SteamVR should be installed");
        let session = api.init_overlay().unwrap();
        let tracking = session.avatar_tracking().unwrap();
        let graphics = graphics::create(&tracking).unwrap();
        let overlay = session
            .create_overlay("xrtranslate.test_overlay", "Test Overlay")
            .unwrap();
        overlay.set_width(1.0).unwrap();
        overlay.set_alpha(0.8).unwrap();
        overlay.set_auto_hmd_hud_transform(1.2, -0.35).unwrap();
        let mut image = graphics::RgbaTexture::new(&tracking, graphics, 640, 320).unwrap();
        let mut pixels = vec![255; 640 * 320 * 4];
        // Keep hidden. Cross the event-queue capacity using one persistent image;
        // a single upload misses failures after sustained caption updates.
        for frame in 0..256 {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[..3].fill(23 + (frame % 191) as u8);
            }
            image.upload(&overlay, &pixels).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // Optional native readback, which some runtimes expose only for raw images.
        let read: unsafe extern "system" fn(u64, *mut c_void, u32, *mut u32, *mut u32) -> i32 =
            unsafe { std::mem::transmute(overlay.table()._get_overlay_image_data) };
        let mut actual = vec![0; pixels.len()];
        let (mut width, mut height) = (0, 0);
        let result = unsafe {
            read(
                overlay.handle,
                actual.as_mut_ptr().cast(),
                actual.len() as u32,
                &mut width,
                &mut height,
            )
        };
        if result == 0 {
            assert_eq!((width, height), (640, 320));
            assert_eq!(actual, pixels);
        } else {
            eprintln!("Native GPU image readback unavailable: {result}");
        }
    }
}
