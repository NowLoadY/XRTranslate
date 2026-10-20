use std::{
    collections::VecDeque,
    fmt,
    io::{self, Read, Write},
    process::{Child, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use crate::{
    LlamaServerCommand, LlamaServerEndpoint, LlamaServerRole, LlamaServerSpec, SpecValidationError,
};

/// A started llama-server process with deterministic cleanup on drop.
pub struct LlamaServerProcess {
    child: Option<Child>,
    endpoint: LlamaServerEndpoint,
    role: LlamaServerRole,
    model_alias: String,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl LlamaServerProcess {
    fn new(mut child: Child, spec: &LlamaServerSpec) -> io::Result<Self> {
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let mut stderr = child.stderr.take().expect("managed stderr is piped");
        let tail = Arc::clone(&stderr_tail);
        let reader = std::thread::Builder::new()
            .name("llama-stderr".into())
            .spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(count) = stderr.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    if let Ok(mut tail) = tail.lock() {
                        retain_stderr(&mut tail, &bytes[..count]);
                    }
                    // Preserve the existing backend log while retaining a bounded
                    // diagnostic for the GUI, which cannot see inherited stderr.
                    let _ = io::stderr().write_all(&bytes[..count]);
                }
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child: Some(child),
            endpoint: spec.endpoint.clone(),
            role: spec.role,
            model_alias: spec.model_alias.clone(),
            stderr_tail,
            stderr_reader: Some(reader),
        })
    }

    #[must_use]
    pub const fn role(&self) -> LlamaServerRole {
        self.role
    }

    #[must_use]
    pub fn model_alias(&self) -> &str {
        &self.model_alias
    }

    #[must_use]
    pub fn endpoint(&self) -> &LlamaServerEndpoint {
        &self.endpoint
    }

    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Explains a failed startup using the child log instead of only an OS code.
    #[must_use]
    pub fn startup_error(&self, status: ExitStatus) -> String {
        let tail = self
            .stderr_tail
            .lock()
            .map(|tail| {
                String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<_>>()).into_owned()
            })
            .unwrap_or_default();
        startup_error(
            &self.model_alias,
            &status.to_string(),
            &tail,
            system_commit(),
        )
    }

    fn finish_stderr(&mut self) {
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

const STDERR_TAIL_BYTES: usize = 8192;

fn retain_stderr(tail: &mut VecDeque<u8>, bytes: &[u8]) {
    tail.extend(bytes.iter().copied());
    let overflow = tail.len().saturating_sub(STDERR_TAIL_BYTES);
    tail.drain(..overflow);
}

fn startup_error(alias: &str, status: &str, stderr: &str, commit: Option<(u64, u64)>) -> String {
    let mut message = format!("managed {alias} llama-server exited during startup ({status})");
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("out of memory") || lower.contains("failed to allocate") {
        message.push_str(": model memory allocation failed. ");
        if let Some((used, limit)) = commit {
            const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
            message.push_str(&format!(
                "Windows committed memory: {:.1}/{:.1} GiB ({:.1} GiB available). ",
                used as f64 / GIB,
                limit as f64 / GIB,
                limit.saturating_sub(used) as f64 / GIB,
            ));
            if limit.saturating_sub(used) < 2 * 1024 * 1024 * 1024 {
                message.push_str("Enable a system-managed Windows paging file or close unused applications, then retry. ");
            } else {
                message.push_str("Free GPU/system memory or select a smaller model, then retry. ");
            }
        } else {
            message.push_str("Free GPU/system memory or select a smaller model, then retry. ");
        }
    }
    // Include one relevant native error, without flooding the UI with the full log.
    if let Some(line) = stderr.lines().rev().find(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("out of memory") || line.contains("error:") || line.contains("failed")
    }) {
        message.push_str("\n");
        message.extend(line.chars().take(512));
    } else {
        message.push_str("; see the backend startup log for details");
    }
    message
}

#[cfg(windows)]
fn system_commit() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
    let mut info = PERFORMANCE_INFORMATION::default();
    let size = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    info.cb = size;
    // The API fills this initialized, correctly sized structure synchronously.
    if unsafe { GetPerformanceInfo(&mut info, size) } == 0 {
        return None;
    }
    Some((
        info.CommitTotal as u64 * info.PageSize as u64,
        info.CommitLimit as u64 * info.PageSize as u64,
    ))
}

#[cfg(not(windows))]
fn system_commit() -> Option<(u64, u64)> {
    None
}

/// Lifecycle operations that a backend can use without depending on a
/// concrete launcher.  A test implementation can supply a fake process while
/// production uses [`LlamaServerProcess`].
pub trait LlamaServerProcessHandle {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn stop(&mut self) -> io::Result<ExitStatus>;
}

impl LlamaServerProcessHandle for LlamaServerProcess {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }?;
        if status.is_some() {
            self.finish_stderr();
        }
        Ok(status)
    }

    /// Stops the child if it is still running, then always reaps it.
    fn stop(&mut self) -> io::Result<ExitStatus> {
        let Some(mut child) = self.child.take() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "llama-server process has already been stopped",
            ));
        };

        if let Some(status) = child.try_wait()? {
            self.finish_stderr();
            return Ok(status);
        }

        child.kill()?;
        let status = child.wait()?;
        self.finish_stderr();
        Ok(status)
    }
}

impl Drop for LlamaServerProcess {
    fn drop(&mut self) {
        // A best-effort kill and wait prevents an orphaned model process when
        // the application exits or initialization fails after spawning.
        let _ = self.stop();
    }
}

/// A process-spawning abstraction.  It makes lifecycle orchestration
/// independently testable while preserving a standard-library production
/// implementation.
pub trait LlamaServerLauncher {
    type Process: LlamaServerProcessHandle;
    type Error;

    fn launch(&self, spec: &LlamaServerSpec) -> Result<Self::Process, Self::Error>;
}

/// Standard-library launcher for a real `llama-server` executable.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdLlamaServerLauncher;

impl LlamaServerLauncher for StdLlamaServerLauncher {
    type Process = LlamaServerProcess;
    type Error = SupervisorError;

    fn launch(&self, spec: &LlamaServerSpec) -> Result<Self::Process, Self::Error> {
        let command = LlamaServerCommand::from_spec(spec)?;
        let mut child_command = command.as_std_command();
        child_command.stderr(Stdio::piped());
        // llama-server is a console executable on Windows. It is managed by
        // the GUI backend, so creating a console for each model server only
        // causes a distracting window flash and provides no user interaction.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            child_command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            use std::os::unix::process::CommandExt;

            // The GUI may terminate its backend abruptly. Ask Linux to kill
            // this model process if that backend dies before Rust can drop it.
            let parent_pid = unsafe { libc::getpid() };
            unsafe {
                child_command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    // Close the race where the parent exits before prctl runs.
                    if libc::getppid() != parent_pid {
                        return Err(io::Error::from_raw_os_error(libc::ESRCH));
                    }
                    Ok(())
                });
            }
        }
        let child = child_command.spawn()?;
        LlamaServerProcess::new(child, spec).map_err(SupervisorError::Spawn)
    }
}

/// Failure to construct or start a local llama-server process.
#[derive(Debug)]
pub enum SupervisorError {
    InvalidSpec(SpecValidationError),
    Spawn(io::Error),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(error) => {
                write!(formatter, "invalid llama-server configuration: {error}")
            }
            Self::Spawn(error) => write!(formatter, "failed to start llama-server: {error}"),
        }
    }
}

impl std::error::Error for SupervisorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSpec(error) => Some(error),
            Self::Spawn(error) => Some(error),
        }
    }
}

impl From<SpecValidationError> for SupervisorError {
    fn from(error: SpecValidationError) -> Self {
        Self::InvalidSpec(error)
    }
}

impl From<io::Error> for SupervisorError {
    fn from(error: io::Error) -> Self {
        Self::Spawn(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn memory_failure_explains_windows_commit_limit() {
        let message = startup_error(
            "qwen3-asr",
            "0xc0000409",
            "cudaMalloc failed: out of memory\nGGML_ASSERT(buffer) failed",
            Some((43 * 1024 * 1024 * 1024, 44 * 1024 * 1024 * 1024)),
        );
        assert!(message.contains("43.0/44.0 GiB (1.0 GiB available)"));
        assert!(message.contains("system-managed Windows paging file"));
    }

    #[test]
    fn unrelated_failure_does_not_claim_memory_exhaustion() {
        let message = startup_error("asr", "1", "error: invalid model file", Some((1, 2)));
        assert!(message.contains("invalid model file"));
        assert!(!message.contains("paging file"));
        assert!(!message.contains("memory allocation failed"));
    }

    #[test]
    fn stderr_tail_is_bounded_and_keeps_the_final_error() {
        let mut tail = VecDeque::new();
        retain_stderr(&mut tail, &vec![b'x'; STDERR_TAIL_BYTES * 2]);
        retain_stderr(&mut tail, b"\ncudaMalloc failed: out of memory\n");
        assert_eq!(tail.len(), STDERR_TAIL_BYTES);
        assert!(
            String::from_utf8(tail.into())
                .unwrap()
                .ends_with("out of memory\n")
        );
    }

    #[test]
    fn child_failure_is_drained_before_reporting() {
        let executable = std::env::current_exe().unwrap();
        let child = Command::new(&executable)
            .args([
                "--ignored",
                "--exact",
                "process::tests::stderr_child",
                "--nocapture",
            ])
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let spec = LlamaServerSpec::new(
            LlamaServerRole::Translation,
            executable,
            "unused.gguf",
            None,
            "test",
        );
        let mut process = LlamaServerProcess::new(child, &spec).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = process.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(!status.success());
        assert!(
            process
                .startup_error(status)
                .contains("model memory allocation failed")
        );
        assert!(!process.stop().unwrap().success());
    }

    #[test]
    #[ignore = "subprocess fixture"]
    fn stderr_child() {
        eprintln!("cudaMalloc failed: out of memory");
        std::process::exit(1);
    }
}
