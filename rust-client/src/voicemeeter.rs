//! Host adapter for the VoiceMeeter Remote API.
//!
//! The adapter is constructed only when the official VoiceMeeter uninstall
//! registration exists. It loads the architecture-matching Remote DLL from
//! that installation, logs in once, and logs out before unloading the DLL.

use std::{
    fmt,
    path::{Path, PathBuf},
};

const UNINSTALL_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMeeterEdition {
    Standard,
    Banana,
    Potato,
    Unknown(i32),
}

impl VoiceMeeterEdition {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            1 => Self::Standard,
            2 => Self::Banana,
            3 | 6 => Self::Potato,
            value => Self::Unknown(value),
        }
    }

    const fn run_code(self) -> Option<i32> {
        match self {
            Self::Standard => Some(1),
            Self::Banana => Some(2),
            Self::Potato if cfg!(target_pointer_width = "64") => Some(6),
            Self::Potato => Some(3),
            Self::Unknown(_) => None,
        }
    }

    const fn strip_count(self) -> Option<u8> {
        match self {
            Self::Standard => Some(3),
            Self::Banana => Some(5),
            Self::Potato => Some(8),
            Self::Unknown(_) => None,
        }
    }

    const fn supports_bus(self, bus: VoiceMeeterBus) -> bool {
        match self {
            Self::Standard => matches!(bus, VoiceMeeterBus::B1),
            Self::Banana => matches!(bus, VoiceMeeterBus::B1 | VoiceMeeterBus::B2),
            Self::Potato => true,
            Self::Unknown(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMeeterBus {
    B1,
    B2,
    B3,
}

impl VoiceMeeterBus {
    const fn parameter_suffix(self) -> &'static str {
        match self {
            Self::B1 => "B1",
            Self::B2 => "B2",
            Self::B3 => "B3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceMeeterVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub build: u8,
}

impl VoiceMeeterVersion {
    pub const fn from_packed(value: u32) -> Self {
        Self {
            major: ((value >> 24) & 0xff) as u8,
            minor: ((value >> 16) & 0xff) as u8,
            patch: ((value >> 8) & 0xff) as u8,
            build: (value & 0xff) as u8,
        }
    }
}

impl fmt::Display for VoiceMeeterVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{}.{}.{}",
            self.major, self.minor, self.patch, self.build
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceMeeterStatus {
    pub running: bool,
    pub edition: Option<VoiceMeeterEdition>,
    pub version: Option<VoiceMeeterVersion>,
}

pub fn strip_bus_parameter(strip: u8, bus: VoiceMeeterBus) -> String {
    format!("Strip[{strip}].{}", bus.parameter_suffix())
}

pub fn remote_dll_name() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "VoicemeeterRemote64.dll"
    } else {
        "VoicemeeterRemote.dll"
    }
}

fn installation_dir_from_uninstall_string(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let executable = if let Some(rest) = value.strip_prefix('"') {
        let end = rest.find('"')?;
        &rest[..end]
    } else {
        let lowercase = value.to_ascii_lowercase();
        let end = lowercase
            .find(".exe")
            .map(|offset| offset + 4)
            .or_else(|| value.find(char::is_whitespace))
            .unwrap_or(value.len());
        value[..end].trim()
    };
    let separator = executable.rfind(['\\', '/'])?;
    (separator > 0).then(|| PathBuf::from(&executable[..separator]))
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum VoiceMeeterError {
    UnsupportedPlatform,
    Registry {
        operation: &'static str,
        code: u32,
    },
    InvalidUninstallString(String),
    MissingRemoteDll(PathBuf),
    LoadLibrary {
        path: PathBuf,
        detail: String,
    },
    MissingSymbol(&'static str),
    Api {
        operation: &'static str,
        code: i32,
    },
    InvalidParameterName,
    NotRunning,
    UnsupportedEdition(VoiceMeeterEdition),
    InvalidStrip {
        edition: VoiceMeeterEdition,
        strip: u8,
    },
    UnsupportedBus {
        edition: VoiceMeeterEdition,
        bus: VoiceMeeterBus,
    },
    LockPoisoned,
}

impl fmt::Display for VoiceMeeterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("VoiceMeeter is available only on Windows")
            }
            Self::Registry { operation, code } => {
                write!(
                    formatter,
                    "VoiceMeeter registry {operation} failed with Windows error {code}"
                )
            }
            Self::InvalidUninstallString(value) => {
                write!(formatter, "VoiceMeeter UninstallString is invalid: {value}")
            }
            Self::MissingRemoteDll(path) => {
                write!(
                    formatter,
                    "VoiceMeeter Remote DLL is missing: {}",
                    path.display()
                )
            }
            Self::LoadLibrary { path, detail } => {
                write!(formatter, "could not load {}: {detail}", path.display())
            }
            Self::MissingSymbol(symbol) => {
                write!(formatter, "VoiceMeeter Remote DLL has no {symbol} export")
            }
            Self::Api { operation, code } => {
                write!(formatter, "VoiceMeeter {operation} failed with code {code}")
            }
            Self::InvalidParameterName => {
                formatter.write_str("VoiceMeeter parameter contains a NUL byte")
            }
            Self::NotRunning => formatter.write_str("VoiceMeeter is not running"),
            Self::UnsupportedEdition(edition) => {
                write!(formatter, "unsupported VoiceMeeter edition {edition:?}")
            }
            Self::InvalidStrip { edition, strip } => {
                write!(formatter, "strip {strip} is invalid for {edition:?}")
            }
            Self::UnsupportedBus { edition, bus } => {
                write!(formatter, "bus {bus:?} is unavailable in {edition:?}")
            }
            Self::LockPoisoned => formatter.write_str("VoiceMeeter Remote API lock is poisoned"),
        }
    }
}

impl std::error::Error for VoiceMeeterError {}

pub type VoiceMeeterResult<T> = Result<T, VoiceMeeterError>;

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{CString, c_char},
        sync::{Arc, Mutex},
    };
    use windows::{
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, FreeLibrary, HMODULE},
            System::{
                LibraryLoader::{
                    GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW,
                },
                Registry::{
                    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
                    REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
                    RegQueryValueExW,
                },
            },
        },
        core::{PCSTR, PCWSTR},
    };

    type LoginFn = unsafe extern "system" fn() -> i32;
    type LogoutFn = unsafe extern "system" fn() -> i32;
    type RunFn = unsafe extern "system" fn(i32) -> i32;
    type GetInfoFn = unsafe extern "system" fn(*mut i32) -> i32;
    type GetParameterFloatFn = unsafe extern "system" fn(*mut c_char, *mut f32) -> i32;
    type SetParameterFloatFn = unsafe extern "system" fn(*mut c_char, f32) -> i32;

    #[derive(Clone, Copy)]
    struct Api {
        login: LoginFn,
        logout: LogoutFn,
        run: RunFn,
        get_type: GetInfoFn,
        get_version: GetInfoFn,
        get_parameter_float: GetParameterFloatFn,
        set_parameter_float: SetParameterFloatFn,
    }

    struct Inner {
        module: HMODULE,
        api: Api,
        calls: Mutex<()>,
        installation_dir: PathBuf,
    }

    // The module remains loaded for `Inner`'s lifetime and every API call is
    // serialized by `calls`; function pointers are immutable after loading.
    unsafe impl Send for Inner {}
    unsafe impl Sync for Inner {}

    impl Drop for Inner {
        fn drop(&mut self) {
            if let Ok(_guard) = self.calls.lock() {
                // SAFETY: the function was resolved from `module`, which is
                // still loaded, and this is the final Arc owner.
                unsafe { (self.api.logout)() };
            }
            // SAFETY: `module` was returned by LoadLibraryExW exactly once.
            let _ = unsafe { FreeLibrary(self.module) };
        }
    }

    #[derive(Clone)]
    pub struct VoiceMeeterRemote {
        inner: Arc<Inner>,
    }

    impl fmt::Debug for VoiceMeeterRemote {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("VoiceMeeterRemote")
                .field("installation_dir", &self.inner.installation_dir)
                .finish_non_exhaustive()
        }
    }

    #[allow(dead_code)]
    impl VoiceMeeterRemote {
        pub fn discover() -> VoiceMeeterResult<Option<Self>> {
            let Some(uninstall) = read_uninstall_string()? else {
                return Ok(None);
            };
            let installation_dir = installation_dir_from_uninstall_string(&uninstall)
                .ok_or_else(|| VoiceMeeterError::InvalidUninstallString(uninstall.clone()))?;
            let dll_path = installation_dir.join(remote_dll_name());
            if !dll_path.is_file() {
                return Err(VoiceMeeterError::MissingRemoteDll(dll_path));
            }
            let wide = wide_null(&dll_path.as_os_str().to_string_lossy());
            // SAFETY: `wide` is NUL-terminated and the full installed DLL path
            // remains alive for this call.
            let module = unsafe {
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
            }
            .map_err(|error| VoiceMeeterError::LoadLibrary {
                path: dll_path,
                detail: error.to_string(),
            })?;
            let api = match unsafe { Api::load(module) } {
                Ok(api) => api,
                Err(error) => {
                    let _ = unsafe { FreeLibrary(module) };
                    return Err(error);
                }
            };
            // Login code 1 is successful but means the application is not yet
            // running. Both 0 and 1 establish a client that must log out.
            let login = unsafe { (api.login)() };
            if !matches!(login, 0 | 1) {
                let _ = unsafe { FreeLibrary(module) };
                return Err(VoiceMeeterError::Api {
                    operation: "Login",
                    code: login,
                });
            }
            Ok(Some(Self {
                inner: Arc::new(Inner {
                    module,
                    api,
                    calls: Mutex::new(()),
                    installation_dir,
                }),
            }))
        }

        pub fn installation_dir(&self) -> &Path {
            &self.inner.installation_dir
        }

        pub fn status(&self) -> VoiceMeeterResult<VoiceMeeterStatus> {
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            let mut raw_type = 0;
            let type_result = unsafe { (self.inner.api.get_type)(&mut raw_type) };
            if type_result == -2 {
                return Ok(VoiceMeeterStatus {
                    running: false,
                    edition: None,
                    version: None,
                });
            }
            check_api("GetVoicemeeterType", type_result)?;
            let mut raw_version = 0;
            check_api("GetVoicemeeterVersion", unsafe {
                (self.inner.api.get_version)(&mut raw_version)
            })?;
            Ok(VoiceMeeterStatus {
                running: true,
                edition: Some(VoiceMeeterEdition::from_raw(raw_type)),
                version: Some(VoiceMeeterVersion::from_packed(raw_version as u32)),
            })
        }

        pub fn start(&self, edition: VoiceMeeterEdition) -> VoiceMeeterResult<()> {
            let code = edition
                .run_code()
                .ok_or(VoiceMeeterError::UnsupportedEdition(edition))?;
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            check_api("RunVoicemeeter", unsafe { (self.inner.api.run)(code) })
        }

        /// Request a clean application shutdown. Callers must use this only
        /// for an instance they started themselves.
        pub fn shutdown(&self) -> VoiceMeeterResult<()> {
            self.set_parameter_float("Command.Shutdown", 1.0)
        }

        pub fn get_parameter_float(&self, parameter: &str) -> VoiceMeeterResult<f32> {
            let name =
                CString::new(parameter).map_err(|_| VoiceMeeterError::InvalidParameterName)?;
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            raw_get(&self.inner.api, &name)
        }

        pub fn set_parameter_float(&self, parameter: &str, value: f32) -> VoiceMeeterResult<()> {
            let name =
                CString::new(parameter).map_err(|_| VoiceMeeterError::InvalidParameterName)?;
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            raw_set(&self.inner.api, &name, value)
        }

        pub fn configure(
            &self,
            strip: u8,
            bus: VoiceMeeterBus,
            enabled: bool,
        ) -> VoiceMeeterResult<VoiceMeeterStripRouteGuard> {
            let status = self.status()?;
            let edition = status.edition.ok_or(VoiceMeeterError::NotRunning)?;
            let strip_count = edition
                .strip_count()
                .ok_or(VoiceMeeterError::UnsupportedEdition(edition))?;
            if strip >= strip_count {
                return Err(VoiceMeeterError::InvalidStrip { edition, strip });
            }
            if !edition.supports_bus(bus) {
                return Err(VoiceMeeterError::UnsupportedBus { edition, bus });
            }
            let parameter = strip_bus_parameter(strip, bus);
            let name = CString::new(parameter.as_str()).expect("generated parameter has no NUL");
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            let original = raw_get(&self.inner.api, &name)?;
            raw_set(&self.inner.api, &name, if enabled { 1.0 } else { 0.0 })?;
            Ok(VoiceMeeterStripRouteGuard {
                inner: Arc::clone(&self.inner),
                parameter,
                original,
                active: true,
            })
        }
    }

    pub struct VoiceMeeterStripRouteGuard {
        inner: Arc<Inner>,
        parameter: String,
        original: f32,
        active: bool,
    }

    impl fmt::Debug for VoiceMeeterStripRouteGuard {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("VoiceMeeterStripRouteGuard")
                .field("parameter", &self.parameter)
                .field("original", &self.original)
                .field("active", &self.active)
                .finish()
        }
    }

    #[allow(dead_code)]
    impl VoiceMeeterStripRouteGuard {
        pub fn parameter(&self) -> &str {
            &self.parameter
        }

        pub fn clear(mut self) -> VoiceMeeterResult<()> {
            self.restore()?;
            self.active = false;
            Ok(())
        }

        fn restore(&self) -> VoiceMeeterResult<()> {
            let name =
                CString::new(self.parameter.as_str()).expect("generated parameter has no NUL");
            let _guard = self
                .inner
                .calls
                .lock()
                .map_err(|_| VoiceMeeterError::LockPoisoned)?;
            raw_set(&self.inner.api, &name, self.original)
        }
    }

    impl Drop for VoiceMeeterStripRouteGuard {
        fn drop(&mut self) {
            if self.active {
                let _ = self.restore();
                self.active = false;
            }
        }
    }

    impl Api {
        unsafe fn load(module: HMODULE) -> VoiceMeeterResult<Self> {
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {{
                    let pointer =
                        unsafe { GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr())) }
                            .ok_or(VoiceMeeterError::MissingSymbol($name))?;
                    unsafe {
                        std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(pointer)
                    }
                }};
            }
            Ok(Self {
                login: symbol!("VBVMR_Login", LoginFn),
                logout: symbol!("VBVMR_Logout", LogoutFn),
                run: symbol!("VBVMR_RunVoicemeeter", RunFn),
                get_type: symbol!("VBVMR_GetVoicemeeterType", GetInfoFn),
                get_version: symbol!("VBVMR_GetVoicemeeterVersion", GetInfoFn),
                get_parameter_float: symbol!("VBVMR_GetParameterFloat", GetParameterFloatFn),
                set_parameter_float: symbol!("VBVMR_SetParameterFloat", SetParameterFloatFn),
            })
        }
    }

    fn raw_get(api: &Api, name: &CString) -> VoiceMeeterResult<f32> {
        let mut value = 0.0;
        check_api("GetParameterFloat", unsafe {
            (api.get_parameter_float)(name.as_ptr().cast_mut(), &mut value)
        })?;
        Ok(value)
    }

    fn raw_set(api: &Api, name: &CString, value: f32) -> VoiceMeeterResult<()> {
        check_api("SetParameterFloat", unsafe {
            (api.set_parameter_float)(name.as_ptr().cast_mut(), value)
        })
    }

    fn check_api(operation: &'static str, code: i32) -> VoiceMeeterResult<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(VoiceMeeterError::Api { operation, code })
        }
    }

    fn read_uninstall_string() -> VoiceMeeterResult<Option<String>> {
        let views = if cfg!(target_pointer_width = "64") {
            [KEY_WOW64_64KEY, KEY_WOW64_32KEY]
        } else {
            [KEY_WOW64_32KEY, KEY_WOW64_64KEY]
        };
        for view in views {
            if let Some(value) = read_registry_value(view)? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    fn read_registry_value(
        view: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> VoiceMeeterResult<Option<String>> {
        let key_path = wide_null(UNINSTALL_KEY);
        let mut key = HKEY(std::ptr::null_mut());
        let result = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_path.as_ptr()),
                None,
                KEY_READ | view,
                &mut key,
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if result != ERROR_SUCCESS {
            return Err(VoiceMeeterError::Registry {
                operation: "open",
                code: result.0,
            });
        }
        let value_name = wide_null("UninstallString");
        let mut value_type = REG_VALUE_TYPE(0);
        let mut byte_count = 0u32;
        let size_result = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_count),
            )
        };
        if size_result != ERROR_SUCCESS {
            let _ = unsafe { RegCloseKey(key) };
            return Err(VoiceMeeterError::Registry {
                operation: "query UninstallString size",
                code: size_result.0,
            });
        }
        if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
            let _ = unsafe { RegCloseKey(key) };
            return Err(VoiceMeeterError::InvalidUninstallString(format!(
                "registry value type {}",
                value_type.0
            )));
        }
        let mut buffer = vec![0u16; (byte_count as usize).div_ceil(2).max(1)];
        let read_result = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut byte_count),
            )
        };
        let _ = unsafe { RegCloseKey(key) };
        if read_result != ERROR_SUCCESS {
            return Err(VoiceMeeterError::Registry {
                operation: "read UninstallString",
                code: read_result.0,
            });
        }
        let end = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        Ok(Some(String::from_utf16_lossy(&buffer[..end])))
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct VoiceMeeterRemote;

    impl VoiceMeeterRemote {
        pub fn discover() -> VoiceMeeterResult<Option<Self>> {
            Ok(None)
        }
        pub fn installation_dir(&self) -> &Path {
            Path::new("")
        }
        pub fn status(&self) -> VoiceMeeterResult<VoiceMeeterStatus> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
        pub fn start(&self, _edition: VoiceMeeterEdition) -> VoiceMeeterResult<()> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }

        pub fn shutdown(&self) -> VoiceMeeterResult<()> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
        pub fn get_parameter_float(&self, _parameter: &str) -> VoiceMeeterResult<f32> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
        pub fn set_parameter_float(&self, _parameter: &str, _value: f32) -> VoiceMeeterResult<()> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
        pub fn configure(
            &self,
            _strip: u8,
            _bus: VoiceMeeterBus,
            _enabled: bool,
        ) -> VoiceMeeterResult<VoiceMeeterStripRouteGuard> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
    }

    #[derive(Debug)]
    pub struct VoiceMeeterStripRouteGuard;

    impl VoiceMeeterStripRouteGuard {
        pub fn parameter(&self) -> &str {
            ""
        }
        pub fn clear(self) -> VoiceMeeterResult<()> {
            Err(VoiceMeeterError::UnsupportedPlatform)
        }
    }
}

pub use platform::{VoiceMeeterRemote, VoiceMeeterStripRouteGuard};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_version_uses_official_byte_order() {
        let version = VoiceMeeterVersion::from_packed(0x0301_0204);
        assert_eq!(
            version,
            VoiceMeeterVersion {
                major: 3,
                minor: 1,
                patch: 2,
                build: 4
            }
        );
        assert_eq!(version.to_string(), "3.1.2.4");
    }

    #[test]
    fn editions_and_bus_support_match_mixer_layouts() {
        assert_eq!(
            VoiceMeeterEdition::from_raw(1),
            VoiceMeeterEdition::Standard
        );
        assert_eq!(VoiceMeeterEdition::from_raw(2), VoiceMeeterEdition::Banana);
        assert_eq!(VoiceMeeterEdition::from_raw(3), VoiceMeeterEdition::Potato);
        assert_eq!(VoiceMeeterEdition::from_raw(6), VoiceMeeterEdition::Potato);
        assert!(VoiceMeeterEdition::Standard.supports_bus(VoiceMeeterBus::B1));
        assert!(!VoiceMeeterEdition::Standard.supports_bus(VoiceMeeterBus::B2));
        assert!(VoiceMeeterEdition::Banana.supports_bus(VoiceMeeterBus::B2));
        assert!(!VoiceMeeterEdition::Banana.supports_bus(VoiceMeeterBus::B3));
        assert!(VoiceMeeterEdition::Potato.supports_bus(VoiceMeeterBus::B3));
    }

    #[test]
    fn strip_bus_parameter_changes_only_the_target_button() {
        assert_eq!(strip_bus_parameter(0, VoiceMeeterBus::B1), "Strip[0].B1");
        assert_eq!(strip_bus_parameter(7, VoiceMeeterBus::B3), "Strip[7].B3");
    }

    #[test]
    fn uninstall_command_resolves_quoted_and_unquoted_executables() {
        assert_eq!(
            installation_dir_from_uninstall_string(
                r#""C:\Program Files (x86)\VB\Voicemeeter\voicemeeter8setup.exe" -uninstall"#
            ),
            Some(PathBuf::from(r"C:\Program Files (x86)\VB\Voicemeeter"))
        );
        assert_eq!(
            installation_dir_from_uninstall_string(
                r"C:\Program Files\VB\Voicemeeter\voicemeetersetup.exe /U"
            ),
            Some(PathBuf::from(r"C:\Program Files\VB\Voicemeeter"))
        );
    }

    #[test]
    fn dll_name_matches_process_architecture() {
        assert_eq!(
            remote_dll_name(),
            if cfg!(target_pointer_width = "64") {
                "VoicemeeterRemote64.dll"
            } else {
                "VoicemeeterRemote.dll"
            }
        );
    }
}
