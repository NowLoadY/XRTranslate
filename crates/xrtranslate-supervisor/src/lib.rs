//! Supervision primitives for the local `llama-server` processes used by
//! XRTranslate.
//!
//! This crate deliberately has no HTTP or async-runtime dependency.  It owns
//! process construction and cleanup, while the backend supplies the HTTP
//! implementation used for readiness checks.  Keeping those concerns
//! separate makes the process layer usable by the desktop application and by
//! a future standalone Rust backend.

mod command;
mod health;
mod process;
mod spec;

pub use command::LlamaServerCommand;
pub use health::{HealthCheckRequest, HealthCheckStatus, LlamaHealthChecker};
#[cfg(windows)]
pub use process::configure_inference_performance;
pub use process::{
    LlamaServerLauncher, LlamaServerProcess, LlamaServerProcessHandle, StdLlamaServerLauncher,
    SupervisorError,
};
pub use spec::{
    FlashAttention, GpuLayers, LlamaServerEndpoint, LlamaServerRole, LlamaServerSpec,
    SpecValidationError,
};
