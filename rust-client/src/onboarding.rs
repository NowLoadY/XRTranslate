//! Onboarding prerequisite validation, resource readiness detection, and step verification.
//!
//! Provides a unified contract for validating that all required services, models,
//! API keys, and runtime binaries are in place before allowing access to the main session.

use std::path::Path;

use crate::{
    backend::BackendManager,
    model_install::{self, NativeModelTaskManager},
    runtime_install::RuntimeInstaller,
    service_config::ServiceConfigEditor,
};

/// Evaluates whether any required resource (API keys, ASR/MT models, TTS models,
/// or runtime acceleration packages) is missing from the system.
///
/// Returns `true` if any prerequisite is unmet, indicating that the onboarding
/// setup flow must be completed.
#[must_use]
pub fn has_unmet_prerequisites(
    project_root: &Path,
    service_config: &ServiceConfigEditor,
    backend_manager: &BackendManager,
    model_task_manager: &NativeModelTaskManager,
    runtime_installer: &RuntimeInstaller,
) -> bool {
    let requirements = service_config.runtime_requirements();

    // 1. API key requirements
    if requirements.missing_api_key {
        return true;
    }

    // 2. Every selected provider-declared local model package.
    let packages = model_install::configured_model_packages(project_root).unwrap_or_default();
    if !packages.iter().all(|package| {
        model_task_manager.is_model_ready(package.id)
            || model_install::model_asset_is_present(project_root, package.id).unwrap_or(false)
    }) {
        return true;
    }

    // 3. Runtime binary and acceleration dependencies
    let llama_ready = !requirements.llama_cpp || backend_manager.llama_server_path_is_valid();
    let onnx_ready =
        !(requirements.onnx_tts || requirements.onnx_cpu) || runtime_installer.plan_is_ready();
    if !llama_ready || !onnx_ready {
        return true;
    }

    false
}

/// Evaluates unmet prerequisites for a specific step in the onboarding flow.
/// Used by the footer navigation to guard advancing to subsequent steps.
#[must_use]
pub fn evaluate_step_requirement(
    step: usize,
    project_root: &Path,
    service_config: &ServiceConfigEditor,
    backend_manager: &BackendManager,
    model_task_manager: &NativeModelTaskManager,
    runtime_installer: &RuntimeInstaller,
) -> Option<&'static str> {
    match step {
        1 => {
            let requirements = service_config.runtime_requirements();
            if requirements.missing_api_key {
                Some("Configure every required API key to continue.")
            } else {
                None
            }
        }
        2 => None,
        3 => {
            if model_task_manager.is_busy() {
                return Some("Wait for the current model task to finish.");
            }
            if runtime_installer.is_busy() {
                return Some("Wait for runtime preparation to finish.");
            }
            let requirements = service_config.runtime_requirements();
            if requirements.missing_api_key {
                return Some("Configure every required API key to continue.");
            }
            let packages = match model_install::configured_model_packages(project_root) {
                Ok(packages) => packages,
                Err(_) => return Some("Download every required model package to continue."),
            };
            if !packages.iter().all(|package| {
                model_task_manager.is_model_ready(package.id)
                    || model_install::model_asset_is_present(project_root, package.id)
                        .unwrap_or(false)
            }) {
                return Some("Download every required model package to continue.");
            }

            let llama_ready =
                !requirements.llama_cpp || backend_manager.llama_server_path_is_valid();
            let onnx_ready = !(requirements.onnx_tts || requirements.onnx_cpu)
                || runtime_installer.plan_is_ready();
            if !llama_ready || !onnx_ready {
                Some("Install the runtime to continue.")
            } else {
                None
            }
        }
        _ => None,
    }
}
