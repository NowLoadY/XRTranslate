//! Small IVRInput action set. Normal priority observes input without taking it
//! from the scene application; this never enables SteamVR's global laser mouse.
use super::openvr::{OpenVrSession, TrackedPose};
use glam::Mat4;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

#[derive(Clone, Copy, Default)]
pub(super) struct Hand {
    /// Standing-space pose selected by the SteamVR action binding. Defaults use
    /// the grip origin within the hand; -Z runs along a held handle toward the
    /// thumb. Keep this distinct from the device origin or its pointing ray.
    /// Custom pose bindings are used as-is; choose grip for the same pivot.
    pub pose: Option<Mat4>,
    pub grab: bool,
    pub pressed: bool,
}

// Exact IVRInput_011 C table / structures from Valve v2.15.6 openvr_capi.h:
// https://github.com/ValveSoftware/openvr/blob/v2.15.6/headers/openvr_capi.h
#[repr(C)]
struct InputTable {
    manifest: unsafe extern "system" fn(*const c_char) -> i32, // 0
    action_set: unsafe extern "system" fn(*const c_char, *mut u64) -> i32,
    action: unsafe extern "system" fn(*const c_char, *mut u64) -> i32,
    source: unsafe extern "system" fn(*const c_char, *mut u64) -> i32,
    update: unsafe extern "system" fn(*mut ActiveSet, u32, u32) -> i32,
    digital: unsafe extern "system" fn(u64, *mut Digital, u32, u64) -> i32,
    _analog: usize,
    pose: unsafe extern "system" fn(u64, i32, f32, *mut Pose, u32, u64) -> i32, // 7
    _unused: [usize; 15],
    haptic: unsafe extern "system" fn(u64, f32, f32, f32, f32, u64) -> i32, // 23
}

#[repr(C)]
#[derive(Default)]
struct Digital {
    active: bool,
    origin: u64,
    state: bool,
    changed: bool,
    update_time: f32,
}
#[repr(C)]
#[derive(Default)]
struct Pose {
    active: bool,
    origin: u64,
    pose: TrackedPose,
}
#[repr(C)]
struct ActiveSet {
    set: u64,
    device: u64,
    secondary: u64,
    padding: u32,
    priority: i32,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<TrackedPose>() == 80);
    assert!(std::mem::size_of::<Digital>() == 24);
    assert!(std::mem::size_of::<Pose>() == 96);
    assert!(std::mem::size_of::<ActiveSet>() == 32);
    assert!(std::mem::offset_of!(InputTable, haptic) == 23 * 8);
};

pub(super) struct Input {
    _session: OpenVrSession,
    table: *const InputTable,
    set: u64,
    pose: u64,
    grab: u64,
    haptic: u64,
    hands: [u64; 2],
    was_valid: [bool; 2],
}

fn check(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("SteamVR input error {code}"))
    }
}

impl Input {
    pub fn suspended_by_dashboard(&self) -> bool {
        self._session.dashboard_visible()
    }

    pub fn suspend(&mut self) {
        self.was_valid = [false; 2];
    }

    pub fn new(session: &OpenVrSession, directory: &Path) -> Result<Self, String> {
        let table = session
            .interface(c"FnTable:IVRInput_011")?
            .cast::<InputTable>();
        let api = unsafe { &*table };
        // Stable bundled defaults let SteamVR retain user bindings across runs.
        // User custom bindings are stored by SteamVR, not in these resource files.
        // Valve's profiles expose the OpenXR grip pose as /pose/grip on Vive and
        // Index, but /pose/openxr_grip on Touch (CV1's "grip" is a moving button).
        // https://github.com/ValveSoftware/openvr/wiki/Render-Model-Reference
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        for (name, body) in [
            (
                "actions.json",
                include_str!("../../../resources/plugins/vr_overlay/input/actions.json"),
            ),
            (
                "vive.json",
                include_str!("../../../resources/plugins/vr_overlay/input/vive.json"),
            ),
            (
                "touch.json",
                include_str!("../../../resources/plugins/vr_overlay/input/touch.json"),
            ),
            (
                "index.json",
                include_str!("../../../resources/plugins/vr_overlay/input/index.json"),
            ),
        ] {
            let path = directory.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
                std::fs::write(path, body).map_err(|e| e.to_string())?;
            }
        }
        let path = CString::new(directory.join("actions.json").to_string_lossy().as_bytes())
            .map_err(|e| e.to_string())?;
        check(unsafe { (api.manifest)(path.as_ptr()) })?;
        let handle =
            |name: &CStr, get: unsafe extern "system" fn(*const c_char, *mut u64) -> i32| {
                let mut value = 0;
                check(unsafe { get(name.as_ptr(), &mut value) })?;
                Ok::<_, String>(value)
            };
        Ok(Self {
            _session: session.clone(),
            table,
            set: handle(c"/actions/voice", api.action_set)?,
            pose: handle(c"/actions/voice/in/hand", api.action)?,
            grab: handle(c"/actions/voice/in/grab", api.action)?,
            haptic: handle(c"/actions/voice/out/touch", api.action)?,
            hands: [
                handle(c"/user/hand/left", api.source)?,
                handle(c"/user/hand/right", api.source)?,
            ],
            was_valid: [false; 2],
        })
    }

    pub fn sample(&mut self) -> Result<[Hand; 2], String> {
        let api = unsafe { &*self.table };
        let mut set = ActiveSet {
            set: self.set,
            device: 0,
            secondary: 0,
            padding: 0,
            priority: 0,
        };
        check(unsafe { (api.update)(&mut set, std::mem::size_of::<ActiveSet>() as u32, 1) })?;
        let mut hands = [Hand::default(); 2];
        for (index, source) in self.hands.into_iter().enumerate() {
            let mut pose = Pose::default();
            let mut grab = Digital::default();
            // Same standing origin and 11 ms prediction as eyes_and_head().
            let pose_result = unsafe {
                (api.pose)(
                    self.pose,
                    1,
                    0.011,
                    &mut pose,
                    std::mem::size_of::<Pose>() as u32,
                    source,
                )
            };
            let grab_result = unsafe {
                (api.digital)(
                    self.grab,
                    &mut grab,
                    std::mem::size_of::<Digital>() as u32,
                    source,
                )
            };
            // One missing/reconnecting controller must not disable the other.
            if matches!(pose_result, 9 | 13) || matches!(grab_result, 9 | 13) {
                self.was_valid[index] = false;
                continue;
            }
            check(pose_result)?;
            check(grab_result)?;
            let matrix = (pose.active && pose.pose.valid && pose.pose.connected)
                .then(|| pose.pose.transform.rigid_matrix())
                .flatten();
            let valid = matrix.is_some() && grab.active;
            hands[index] = Hand {
                pose: matrix,
                grab: valid && grab.state,
                // Recovering tracking with grip already held must not grab a prop.
                pressed: valid && self.was_valid[index] && grab.changed && grab.state,
            };
            self.was_valid[index] = valid;
        }
        Ok(hands)
    }

    pub fn pulse(&self, hand: usize) {
        let api = unsafe { &*self.table };
        let _ = unsafe { (api.haptic)(self.haptic, 0.0, 0.025, 90.0, 0.25, self.hands[hand]) };
    }
}
