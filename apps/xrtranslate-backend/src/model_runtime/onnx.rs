//! Consumption of the managed ONNX Runtime selected by the desktop host.
//!
//! Installation and GPU detection stay outside the backend. This module reads
//! the declarative selection marker, initializes one process-wide ORT core and
//! reports provider-neutral diagnostics.

use std::path::Path;

use tracing::{info, warn};
use xrtranslate_config::{AppConfig, NativeRuntimeBackend, NativeRuntimeSelection, RuntimeLayout};
use xrtranslate_inference::{
    OnnxExecutionDevice, initialize_onnx_runtime, preload_onnx_cuda_libraries,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct OnnxRuntimeDiagnostic {
    pub(crate) backend: Option<String>,
    pub(crate) cuda_version: Option<String>,
}

pub(crate) fn initialize_managed_onnx_runtime(
    project_root: &Path,
    config: &AppConfig,
) -> Result<(), String> {
    let layout = config.runtime_layout(project_root);
    let marker_path = layout.native_runtime_selection_file();
    let marker = marker_path
        .is_file()
        .then(|| {
            serde_json::from_slice::<NativeRuntimeSelection>(
                &std::fs::read(&marker_path)
                    .map_err(|error| format!("cannot read {}: {error}", marker_path.display()))?,
            )
            .map_err(|error| format!("invalid {}: {error}", marker_path.display()))
        })
        .transpose()?;
    if marker
        .as_ref()
        .is_some_and(|marker| marker.schema_version != 1)
    {
        return Err(format!(
            "unsupported native runtime marker schema {}",
            marker.as_ref().map_or(0, |marker| marker.schema_version)
        ));
    }

    let requirements = config.runtime_requirements();
    if requirements.onnx_tts && requirements.onnx_cuda {
        if let Some(resolved) = marker
            .as_ref()
            .filter(|marker| marker.onnx_backend == Some(NativeRuntimeBackend::Cuda))
            .map(|marker| layout.resolve_native_runtime_selection(marker))
        {
            let core = resolved.onnx_core_library.clone().or_else(|| {
                resolved
                    .provider_dir
                    .as_ref()
                    .map(|directory| directory.join(RuntimeLayout::ONNX_CORE_LIBRARY))
            });
            let cuda_result = (|| {
                let core = core
                    .as_ref()
                    .ok_or("CUDA runtime marker contains no ONNX core library")?;
                preload_onnx_cuda_libraries(&resolved.preload_libraries)
                    .map_err(|error| error.to_string())?;
                initialize_onnx_runtime(core).map_err(|error| error.to_string())?;
                Ok::<_, String>(())
            })();
            if cuda_result.is_ok() {
                info!(
                    cuda = resolved.cuda_version.as_deref().unwrap_or("unknown"),
                    libraries = resolved.preload_libraries.len(),
                    "managed ONNX CUDA runtime initialized"
                );
                return Ok(());
            }
            warn!(
                error = %cuda_result.unwrap_err(),
                "managed ONNX CUDA runtime is incomplete; using CPU runtime"
            );
        }
    }

    let cpu_core = marker
        .as_ref()
        .map(|marker| layout.resolve_native_runtime_selection(marker))
        .and_then(|runtime| runtime.onnx_core_library)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| layout.onnx_cpu_core_library());
    initialize_onnx_runtime(&cpu_core).map_err(|error| error.to_string())?;
    info!(core = %cpu_core.display(), "managed ONNX CPU runtime initialized");
    Ok(())
}

pub(crate) fn runtime_diagnostic(
    project_root: &Path,
    config: &AppConfig,
    device: OnnxExecutionDevice,
) -> OnnxRuntimeDiagnostic {
    let backend = match device {
        OnnxExecutionDevice::Cuda => "cuda",
        OnnxExecutionDevice::Cpu => "cpu",
        OnnxExecutionDevice::Auto => "auto",
    };
    let cuda_version = (device == OnnxExecutionDevice::Cuda)
        .then(|| {
            let path = config
                .runtime_layout(project_root)
                .native_runtime_selection_file();
            serde_json::from_slice::<NativeRuntimeSelection>(&std::fs::read(path).ok()?)
                .ok()?
                .cuda_version
        })
        .flatten();
    OnnxRuntimeDiagnostic {
        backend: Some(backend.into()),
        cuda_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_diagnostic_never_claims_a_cuda_version() {
        let config = AppConfig::from_json_str(include_str!("../../../../config.json")).unwrap();
        let diagnostic =
            runtime_diagnostic(Path::new("release-root"), &config, OnnxExecutionDevice::Cpu);
        assert_eq!(diagnostic.backend.as_deref(), Some("cpu"));
        assert_eq!(diagnostic.cuda_version, None);
    }
}
