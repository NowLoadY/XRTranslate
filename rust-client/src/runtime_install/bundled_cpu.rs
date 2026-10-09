//! Local recovery of the separately installed, packaged CPU ONNX core.
//!
//! Reuse a compatible local core when its configured release and size match
//! the packaged CPU core. Provider selection remains independent.

use std::{fs, io::Write, path::Path, sync::LazyLock};

use xrtranslate_config::{AppConfig, ManagedRuntimeArchive, RuntimeLayout};

use super::{ManagedRuntimeAsset, OnnxRuntimeSelection, RuntimeBackend, RuntimeDownload};

struct RecoveryCatalog {
    cpu_archives: Vec<ManagedRuntimeArchive>,
    provider: Option<ManagedRuntimeAsset>,
}

static SUPPORTED: LazyLock<Option<RecoveryCatalog>> = LazyLock::new(|| {
    let config = AppConfig::from_json_str(include_str!("../../../config.json")).ok()?;
    let runtime = &config.model_manager.onnxruntime;
    Some(RecoveryCatalog {
        cpu_archives: runtime.resolved_cpu_downloads(),
        provider: super::onnx_assets_from_config(runtime)
            .ok()?
            .into_iter()
            .find(|asset| asset.name == RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE),
    })
});

fn supports_configured_cpu_archive(config: &AppConfig) -> bool {
    let target = super::current_runtime_target();
    let Some(supported) = SUPPORTED.as_ref() else {
        return false;
    };
    let Some(expected) = supported
        .cpu_archives
        .iter()
        .find(|archive| archive.target == target)
    else {
        return false;
    };
    config
        .model_manager
        .onnxruntime
        .resolved_cpu_downloads()
        .iter()
        .find(|archive| archive.target.trim() == target)
        == Some(expected)
}

pub(super) fn restore_cpu_core(layout: &RuntimeLayout, config: &AppConfig) -> Result<bool, String> {
    let expected_bytes = if cfg!(windows) {
        RuntimeLayout::ONNX_CPU_CORE_WIN_BYTES
    } else {
        RuntimeLayout::ONNX_CPU_CORE_LINUX_BYTES
    };
    let destination = layout.onnx_cpu_core_library();
    if super::file_has_expected_size(&destination, super::expected_cpu_core_bytes(config)) {
        return Ok(true);
    }
    // A configured CPU version/build owns its own resource requirements. The
    // local copy is only reused for the supported release and expected size.
    if !supports_configured_cpu_archive(config) {
        return Ok(false);
    }
    for archive in &config.model_manager.onnxruntime.downloads {
        if archive.target != super::current_runtime_target()
            || archive.cuda_version.is_empty()
            || !archive
                .required_files
                .iter()
                .any(|file| file == RuntimeLayout::ONNX_CORE_LIBRARY)
        {
            continue;
        }
        let source = layout
            .onnx_runtime_directory(&archive.cuda_version)
            .join(RuntimeLayout::ONNX_CORE_LIBRARY);
        if !fs::metadata(&source)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == expected_bytes)
        {
            continue;
        }
        let bytes = fs::read(&source).map_err(|error| {
            format!(
                "Cannot inspect local ONNX core {}: {error}",
                source.display()
            )
        })?;
        if bytes.len() as u64 != expected_bytes {
            continue;
        }
        if destination.is_file() {
            fs::remove_file(&destination)
                .map_err(|error| format!("Cannot remove incomplete CPU ONNX core: {error}"))?;
        }
        publish_missing_core(&destination, &bytes)?;
        log::info!("Restored bundled CPU ONNX core from {}", source.display());
        return Ok(true);
    }
    Ok(false)
}

fn publish_missing_core(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = destination
        .parent()
        .ok_or("CPU ONNX core has no parent directory")?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Cannot create {}: {error}", directory.display()))?;
    let staging = directory.join(format!(".onnx-core-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // Publish the complete staged copy without replacing a CPU core another
        // process may have installed. Windows moves without REPLACE_EXISTING;
        // Unix links the staged copy. Neither shares identity with the GPU core.
        match super::publish_file_if_absent(&staging, destination) {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && destination.is_file() =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    })();
    let _ = fs::remove_file(&staging);
    result.map_err(|error| {
        format!(
            "Cannot restore CPU ONNX core {}: {error}",
            destination.display()
        )
    })
}

/// Defer the CPU copy only when this plan will actually install its known
/// source archive. Compare the complete pinned declaration, so a custom URL,
/// version or extraction layout cannot accidentally suppress a CPU download.
pub(super) fn provider_download_supplies_cpu_core(
    config: &AppConfig,
    onnx: Option<&OnnxRuntimeSelection>,
    downloads: &[RuntimeDownload],
) -> bool {
    if !cfg!(windows) || !supports_configured_cpu_archive(config) {
        return false;
    }
    let Some(selection) = onnx.filter(|selection| selection.backend == RuntimeBackend::Cuda) else {
        return false;
    };
    let Some(provider) = &selection.provider else {
        return false;
    };
    SUPPORTED
        .as_ref()
        .and_then(|catalog| catalog.provider.as_ref())
        == Some(provider)
        && downloads
            .iter()
            .any(|download| download.archive_name == provider.name)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        config: AppConfig,
        layout: RuntimeLayout,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("xrtranslate-cpu-recovery-{}", uuid::Uuid::new_v4()));
            let config = AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
            let layout = config.runtime_layout(&root);
            let archive = config
                .model_manager
                .onnxruntime
                .downloads
                .iter()
                .find(|archive| archive.target == super::super::current_runtime_target())
                .unwrap();
            let source = layout
                .onnx_runtime_directory(&archive.cuda_version)
                .join(RuntimeLayout::ONNX_CORE_LIBRARY);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(
                layout.native_runtime_selection_file(),
                b"preserve CUDA selection",
            )
            .unwrap();
            Self {
                root,
                config,
                layout,
                source,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the repository's locally installed official CUDA ONNX core"]
    fn installed_cuda_core_restores_cpu_without_download() {
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let config = super::super::load_app_config(project_root).unwrap();
        let actual_layout = config.runtime_layout(project_root);
        let archive = config
            .model_manager
            .onnxruntime
            .downloads
            .iter()
            .find(|archive| archive.name == RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE)
            .unwrap();
        let actual_core = actual_layout
            .onnx_runtime_directory(&archive.cuda_version)
            .join(RuntimeLayout::ONNX_CORE_LIBRARY);
        let fixture = Fixture::new();
        fs::copy(actual_core, &fixture.source).unwrap();

        assert!(restore_cpu_core(&fixture.layout, &fixture.config).unwrap());
        assert!(
            !super::super::missing_base_bundled_downloads(&fixture.root, &fixture.config, false)
                .iter()
                .any(|download| download.label == "ONNX Runtime (CPU)")
        );
        assert_eq!(
            fs::read(fixture.layout.native_runtime_selection_file()).unwrap(),
            b"preserve CUDA selection"
        );
    }
}
