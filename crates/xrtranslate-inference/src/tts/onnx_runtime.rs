//! Shared ONNX Runtime policy for native TTS providers.
//!
//! Model tensor contracts stay in provider modules. This module owns only the
//! process-wide runtime bootstrap and the atomic CUDA-to-CPU session policy so
//! every ONNX TTS provider observes the same accelerator behavior.

use std::path::{Path, PathBuf};

use ort::{
    ep::{ArenaExtendStrategy, CUDA},
    session::{Session, builder::GraphOptimizationLevel},
};

use crate::InferenceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnnxExecutionDevice {
    Auto,
    Cuda,
    Cpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActiveOnnxDevice {
    Cuda,
    Cpu,
}

impl OnnxExecutionDevice {
    #[must_use]
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "cuda" => Self::Cuda,
            "cpu" => Self::Cpu,
            // DirectML is retained as a legacy spelling only. Native TTS has
            // one validated accelerator policy: CUDA with an atomic CPU
            // fallback.
            "directml" | "dml" | "auto" => Self::Auto,
            _ => Self::Auto,
        }
    }
}

impl ActiveOnnxDevice {
    pub(crate) const fn execution_device(self) -> OnnxExecutionDevice {
        match self {
            Self::Cuda => OnnxExecutionDevice::Cuda,
            Self::Cpu => OnnxExecutionDevice::Cpu,
        }
    }
}

/// Preloads an ordered, already-verified CUDA runtime closure before the first
/// ONNX Runtime API call. Runtime archives own library names and versions;
/// inference consumes only the exact resolved paths.
pub fn preload_onnx_cuda_libraries(libraries: &[PathBuf]) -> Result<(), InferenceError> {
    for library in libraries {
        if !library.is_file() {
            return Err(native_error(format!(
                "CUDA runtime library is missing: {}",
                library.display()
            )));
        }
    }
    for library in libraries {
        ort::util::preload_dylib(library).map_err(|error| {
            native_error(format!(
                "cannot preload CUDA runtime library {}: {error}",
                library.display()
            ))
        })?;
    }
    Ok(())
}

/// Selects the process-wide ONNX Runtime core before any model session opens.
pub fn initialize_onnx_runtime(core_library: &Path) -> Result<(), InferenceError> {
    #[cfg(feature = "managed-ort")]
    {
        if !core_library.is_file() {
            return Err(native_error(format!(
                "ONNX Runtime core is missing: {}",
                core_library.display()
            )));
        }
        let builder = ort::init_from(core_library).map_err(|error| {
            native_error(format!(
                "cannot load ONNX Runtime core {}: {error}",
                core_library.display()
            ))
        })?;
        if !builder.commit() {
            return Err(native_error(
                "ONNX Runtime was initialized before the managed runtime was selected",
            ));
        }
    }
    #[cfg(not(feature = "managed-ort"))]
    let _ = core_library;
    Ok(())
}

pub(crate) fn build_session(
    path: &Path,
    requested: OnnxExecutionDevice,
    threads: usize,
    component: &str,
) -> Result<(Session, ActiveOnnxDevice), InferenceError> {
    let (mut sessions, active) = build_session_group(&[path], requested, threads, component)?;
    Ok((sessions.remove(0), active))
}

/// Builds every model in a pipeline on one execution provider. If any CUDA
/// session fails, all sessions from that attempt are dropped before retrying
/// the complete group on CPU.
pub(crate) fn build_session_group(
    paths: &[&Path],
    requested: OnnxExecutionDevice,
    threads: usize,
    component: &str,
) -> Result<(Vec<Session>, ActiveOnnxDevice), InferenceError> {
    if paths.is_empty() {
        return Err(native_error(format!(
            "{component} contains no ONNX model sessions"
        )));
    }
    let mut last_error = None;
    for &active in device_attempts(requested) {
        let mut sessions = Vec::with_capacity(paths.len());
        let mut failed = false;
        for path in paths {
            match build_session_exact(path, active, threads) {
                Ok(session) => sessions.push(session),
                Err(error) => {
                    tracing::debug!(
                        model = %path.display(),
                        device = ?active,
                        %error,
                        component,
                        "ONNX TTS execution provider unavailable"
                    );
                    last_error = Some(error);
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            drop(sessions);
            continue;
        }
        tracing::info!(device = ?active, component, "ONNX TTS provider plan initialized");
        if active == ActiveOnnxDevice::Cpu && requested != OnnxExecutionDevice::Cpu {
            tracing::info!(
                cuda_error = %last_error.as_ref().expect("CPU follows a failed CUDA attempt"),
                component,
                "ONNX TTS CUDA unavailable; using atomic CPU fallback"
            );
        }
        return Ok((sessions, active));
    }
    Err(ort_error(
        last_error.expect("at least one ONNX execution device is attempted"),
    ))
}

fn device_attempts(requested: OnnxExecutionDevice) -> &'static [ActiveOnnxDevice] {
    match requested {
        OnnxExecutionDevice::Cpu => &[ActiveOnnxDevice::Cpu],
        OnnxExecutionDevice::Auto | OnnxExecutionDevice::Cuda => {
            &[ActiveOnnxDevice::Cuda, ActiveOnnxDevice::Cpu]
        }
    }
}

fn build_session_exact(
    path: &Path,
    device: ActiveOnnxDevice,
    threads: usize,
) -> Result<Session, ort::Error> {
    let builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads.max(1))?
        .with_inter_threads((threads / 2).max(1))?;
    let mut builder = match device {
        ActiveOnnxDevice::Cuda => builder.with_execution_providers([CUDA::default()
            .with_tf32(false)
            .with_arena_extend_strategy(ArenaExtendStrategy::NextPowerOfTwo)
            .build()
            .error_on_failure()])?,
        ActiveOnnxDevice::Cpu => builder,
    };
    builder.commit_from_file(path)
}

fn ort_error(error: ort::Error) -> InferenceError {
    native_error(error.to_string())
}

fn native_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.onnx",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_directml_selection_uses_the_supported_auto_policy() {
        assert_eq!(
            OnnxExecutionDevice::from_config("directml"),
            OnnxExecutionDevice::Auto
        );
    }

    #[test]
    fn cpu_selection_never_attempts_an_accelerator() {
        assert_eq!(
            device_attempts(OnnxExecutionDevice::Cpu),
            &[ActiveOnnxDevice::Cpu]
        );
    }
}
