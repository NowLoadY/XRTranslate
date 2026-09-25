use std::{
    fmt, io,
    process::{Child, ExitStatus},
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
}

impl LlamaServerProcess {
    fn new(child: Child, spec: &LlamaServerSpec) -> Self {
        Self {
            child: Some(child),
            endpoint: spec.endpoint.clone(),
            role: spec.role,
            model_alias: spec.model_alias.clone(),
        }
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
        match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
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
            return Ok(status);
        }

        child.kill()?;
        child.wait()
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
        // llama-server is a console executable on Windows. It is managed by
        // the GUI backend, so creating a console for each model server only
        // causes a distracting window flash and provides no user interaction.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            child_command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(target_os = "linux")]
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
        Ok(LlamaServerProcess::new(child, spec))
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
