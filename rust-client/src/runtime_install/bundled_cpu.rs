//! Local recovery of the separately installed, packaged CPU ONNX core.
//!
//! A CUDA core may also execute CPU sessions, but only the exact packaged
//! fingerprint may be copied into the CPU directory. Provider libraries and the
//! backend selection marker stay untouched.

use std::{fs, io::Write, path::Path, sync::LazyLock};

use sha2::{Digest, Sha256};
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
    let (bytes, sha256) = if cfg!(windows) {
        (
            RuntimeLayout::ONNX_CPU_CORE_WIN_BYTES,
            RuntimeLayout::ONNX_CPU_CORE_WIN_SHA256,
        )
    } else {
        (
            RuntimeLayout::ONNX_CPU_CORE_LINUX_BYTES,
            RuntimeLayout::ONNX_CPU_CORE_LINUX_SHA256,
        )
    };
    restore_cpu_core_with_fingerprint(layout, config, bytes, sha256)
}

fn restore_cpu_core_with_fingerprint(
    layout: &RuntimeLayout,
    config: &AppConfig,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<bool, String> {
    let destination = layout.onnx_cpu_core_library();
    if destination.is_file() {
        return Ok(true);
    }
    // A configured CPU version/build owns its own resource requirements. The
    // packaged fingerprint only proves compatibility with the supported archive.
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
        if bytes.len() as u64 != expected_bytes
            || !format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(expected_sha256)
        {
            continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const CORE: &[u8] = b"verified packaged CPU core";

    struct Fixture {
        root: PathBuf,
        config: AppConfig,
        layout: RuntimeLayout,
        source: PathBuf,
    }

    impl Fixture {
        fn new(custom_runtime: bool) -> Self {
            let root = std::env::temp_dir()
                .join(format!("xrtranslate-cpu-recovery-{}", uuid::Uuid::new_v4()));
            let mut config =
                AppConfig::from_json_str(include_str!("../../../config.json")).unwrap();
            if custom_runtime {
                config.model_manager.runtime_directory = Some(PathBuf::from("custom/native"));
            }
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
            fs::write(&source, CORE).unwrap();
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

        fn restore(&self) -> bool {
            restore_cpu_core_with_fingerprint(
                &self.layout,
                &self.config,
                CORE.len() as u64,
                &format!("{:x}", Sha256::digest(CORE)),
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn verified_local_core_is_copied_without_changing_gpu_or_marker() {
        let fixture = Fixture::new(false);
        assert!(fixture.restore());
        assert_eq!(
            fs::read(fixture.layout.onnx_cpu_core_library()).unwrap(),
            CORE
        );
        assert_eq!(
            fs::read(fixture.layout.native_runtime_selection_file()).unwrap(),
            b"preserve CUDA selection"
        );
        // A later GPU repair must not modify the separately installed CPU core.
        fs::write(&fixture.source, b"GPU replacement").unwrap();
        assert_eq!(
            fs::read(fixture.layout.onnx_cpu_core_library()).unwrap(),
            CORE
        );
    }

    #[test]
    fn invalid_size_or_digest_keeps_cpu_download_required() {
        let fixture = Fixture::new(false);
        for damaged in [b"truncated".to_vec(), vec![b'x'; CORE.len()]] {
            fs::write(&fixture.source, damaged).unwrap();
            assert!(!fixture.restore());
            assert!(!fixture.layout.onnx_cpu_core_library().exists());
            assert!(
                super::super::missing_base_bundled_downloads(&fixture.root, &fixture.config, false)
                    .iter()
                    .any(|download| download.label == "ONNX Runtime (CPU)")
            );
        }
    }

    #[test]
    fn existing_cpu_core_is_preserved() {
        let fixture = Fixture::new(false);
        fs::create_dir_all(fixture.layout.onnx_cpu_runtime_directory()).unwrap();
        fs::write(
            fixture.layout.onnx_cpu_core_library(),
            b"existing CPU installation",
        )
        .unwrap();
        assert!(fixture.restore());
        assert_eq!(
            fs::read(fixture.layout.onnx_cpu_core_library()).unwrap(),
            b"existing CPU installation"
        );
    }

    #[test]
    fn recovery_follows_custom_runtime_directory() {
        let fixture = Fixture::new(true);
        assert!(fixture.restore());
        assert_eq!(
            fs::read(
                fixture
                    .root
                    .join("custom/native/onnxruntime/cpu")
                    .join(RuntimeLayout::ONNX_CORE_LIBRARY)
            )
            .unwrap(),
            CORE
        );
        assert!(!fixture.root.join("runtime").exists());
    }

    #[test]
    fn undeclared_local_core_is_not_reused() {
        let mut fixture = Fixture::new(false);
        fixture.config.model_manager.onnxruntime.downloads.clear();
        assert!(!fixture.restore());
        assert!(!fixture.layout.onnx_cpu_core_library().exists());
    }

    #[test]
    fn custom_cpu_archive_does_not_reuse_packaged_core() {
        let mut fixture = Fixture::new(false);
        fixture
            .config
            .model_manager
            .onnxruntime
            .cpu_downloads
            .iter_mut()
            .find(|archive| archive.target == super::super::current_runtime_target())
            .unwrap()
            .sha256 = "0".repeat(64);
        assert!(!fixture.restore());
        assert!(!fixture.layout.onnx_cpu_core_library().exists());
        assert!(
            super::super::missing_base_bundled_downloads(&fixture.root, &fixture.config, false)
                .iter()
                .any(|download| download.label == "ONNX Runtime (CPU)")
        );
    }

    #[cfg(windows)]
    #[test]
    fn only_planned_pinned_cuda_archive_replaces_cpu_download() {
        let fixture = Fixture::new(false);
        let provider =
            super::super::onnx_assets_from_config(&fixture.config.model_manager.onnxruntime)
                .unwrap()
                .into_iter()
                .find(|asset| asset.name == RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE)
                .unwrap();
        let downloads = vec![RuntimeDownload {
            label: "ONNX CUDA".into(),
            archive_name: provider.name.clone(),
            bytes: provider.size,
        }];
        let mut selection = OnnxRuntimeSelection {
            backend: RuntimeBackend::Cuda,
            provider: Some(provider),
            cuda_runtime: None,
            cuda_dependency: None,
            cudnn: None,
            cuda_version: Some("13.3".into()),
            fallback_reason: None,
        };
        assert!(provider_download_supplies_cpu_core(
            &fixture.config,
            Some(&selection),
            &downloads
        ));
        assert!(
            !super::super::missing_base_bundled_downloads(&fixture.root, &fixture.config, true)
                .iter()
                .any(|download| download.label == "ONNX Runtime (CPU)")
        );
        assert!(!provider_download_supplies_cpu_core(
            &fixture.config,
            Some(&selection),
            &[]
        ));
        let mut custom_cpu_config = fixture.config.clone();
        custom_cpu_config
            .model_manager
            .onnxruntime
            .cpu_downloads
            .iter_mut()
            .find(|archive| archive.target == super::super::current_runtime_target())
            .unwrap()
            .sha256 = "0".repeat(64);
        assert!(!provider_download_supplies_cpu_core(
            &custom_cpu_config,
            Some(&selection),
            &downloads
        ));
        selection.provider.as_mut().unwrap().browser_download_url =
            "https://example.invalid/custom.zip".into();
        assert!(!provider_download_supplies_cpu_core(
            &fixture.config,
            Some(&selection),
            &downloads
        ));
    }

    #[test]
    fn publishing_does_not_overwrite_an_existing_destination() {
        let fixture = Fixture::new(false);
        let destination = fixture.layout.onnx_cpu_core_library();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"another installation").unwrap();
        publish_missing_core(&destination, CORE).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"another installation");
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            1
        );
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
        let fixture = Fixture::new(false);
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
