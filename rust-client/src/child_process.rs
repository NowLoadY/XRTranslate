use std::process::Command;

pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(target_os = "android")]
    {
        command.env(
            "LD_LIBRARY_PATH",
            crate::android::native_library_directory(),
        );
        use std::os::unix::process::CommandExt;
        let parent = unsafe { libc::getpid() };
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    command
}

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

/// Closing the last job handle also ends all managed children, including when
/// Windows tears down the parent process without running Rust destructors.
#[cfg(windows)]
pub(crate) struct KillOnCloseJob {
    handle: OwnedHandle,
}

#[cfg(windows)]
impl KillOnCloseJob {
    pub(crate) fn new() -> Result<Self, String> {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Cannot create process job: {}",
                std::io::Error::last_os_error()
            ));
        }
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(format!(
                "Cannot configure process job: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(job)
    }

    pub(crate) fn assign(&self, process: &impl AsRawHandle) -> Result<(), String> {
        let assigned = unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                self.handle.as_raw_handle(),
                process.as_raw_handle(),
            )
        };
        if assigned == 0 {
            return Err(format!(
                "Cannot manage child process tree: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub(crate) fn assign_pid(&self, process_id: u32) -> Result<(), String> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id) };
        if handle.is_null() {
            return Err(format!(
                "Cannot open child process: {}",
                std::io::Error::last_os_error()
            ));
        }
        let process = unsafe { OwnedHandle::from_raw_handle(handle) };
        self.assign(&process)
    }

    pub(crate) fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.handle.as_raw_handle(),
                1,
            );
        }
    }
}
