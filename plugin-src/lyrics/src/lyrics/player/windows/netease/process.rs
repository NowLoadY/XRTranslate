use super::is_cloudmusic;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, STILL_ACTIVE},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
    },
    core::PWSTR,
};

pub(super) struct PlayerProcess {
    pub pid: u32,
    handle: HANDLE,
    started: u64,
}

impl PlayerProcess {
    pub fn open(pid: u32, uptime_ms: u64) -> Option<Self> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
        let mut process = Self {
            pid,
            handle,
            started: 0,
        };
        let mut path = [0_u16; 32_768];
        let mut length = path.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
            .ok()?;
        }
        if !is_cloudmusic(&String::from_utf16_lossy(&path[..length as usize])) {
            return None;
        }
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).ok()? };
        let created_ms =
            ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)) / 10_000;
        let created_ms = created_ms.checked_sub(11_644_473_600_000)?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        process.started = uptime_ms.saturating_sub(now_ms.checked_sub(created_ms)?);
        Some(process)
    }

    pub fn accepts(&self, event_ms: u64) -> bool {
        let mut exit_code = 0;
        event_ms >= self.started.saturating_sub(1000)
            && unsafe { GetExitCodeProcess(self.handle, &mut exit_code).is_ok() }
            && exit_code == STILL_ACTIVE.0 as u32
    }
}

impl Drop for PlayerProcess {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.handle) };
    }
}
