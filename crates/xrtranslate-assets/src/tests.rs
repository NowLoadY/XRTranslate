use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    AUDIO8_TTS_ONNX_FP16, AtomicInstallError, HUNYUAN_MT_GGUF, MANAGED_LOCAL_MODEL_HARDWARE,
    MODEL_ASSET_CATALOG, ModelAssetDiagnostic, ModelAssetId, ModelAssetManifest, ModelAssetProblem,
    ModelAssetsConfig, ModelCapability, ModelFileRole, ModelLevel, ModelSource,
    OPENVOICE_V2_ONNX_FP16, OPENVOICE_V3_ONNX_FP16, QWEN3_ASR_GGUF, RequiredModelFile,
    ResolvedModelAsset, ResolvedModelAssets, install::install_verified_directory, manifest_for,
    preflight::sha256_file,
};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temporary_project_root() -> PathBuf {
    let unique = format!(
        "xrtranslate-assets-test-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed),
    );
    std::env::temp_dir().join(unique)
}

#[test]
fn static_catalog_declares_every_native_model_package() {
    assert_eq!(MODEL_ASSET_CATALOG.len(), 6);
    assert_eq!(QWEN3_ASR_GGUF.required_files.len(), 2);
    assert_eq!(HUNYUAN_MT_GGUF.required_files.len(), 1);
    assert_eq!(
        OPENVOICE_V2_ONNX_FP16.source.archive.unwrap().sha256,
        "266dc4662965858e07a1c8cb086f17e1c30f0fdc3202e8934103dc7927314811"
    );
    assert_eq!(
        manifest_for(ModelAssetId::Qwen3AsrGguf).provider,
        "qwen3-gguf"
    );
    assert_eq!(
        manifest_for(ModelAssetId::HunyuanMtGguf).source.repository,
        "tencent/Hy-MT2-1.8B-GGUF"
    );
    assert_eq!(
        QWEN3_ASR_GGUF
            .source
            .hugging_face_resolve_url("Qwen3-ASR-1.7B.Q4_K_M.gguf"),
        "https://huggingface.co/mradermacher/Qwen3-ASR-1.7B-GGUF/resolve/cc946c78d3804752f7ba1bc42720c0f7aaf3d1ad/Qwen3-ASR-1.7B.Q4_K_M.gguf"
    );
    assert_eq!(
        AUDIO8_TTS_ONNX_FP16
            .source
            .hugging_face_resolve_url("slow_ar_fp16.onnx"),
        "https://huggingface.co/OpenVoiceOS/phoonnx-audio8-tts/resolve/6e4de996325cebb25df81efd6b0adc08792cd21f/slow_ar_fp16.onnx"
    );
    assert_eq!(
        AUDIO8_TTS_ONNX_FP16
            .source
            .hugging_face_resolve_url("runtime_manifest.json"),
        "https://huggingface.co/Audio8/Audio8-TTS-Preview-0.6B-ONNX-INT4/resolve/818569c6b832118ad68d61bbd873abe250fcd68a/runtime_manifest.json"
    );
    let archive = OPENVOICE_V3_ONNX_FP16.source.archive.unwrap();
    assert_eq!(archive.bytes, 204_513_198);
    assert_eq!(OPENVOICE_V3_ONNX_FP16.download_bytes(), 207_772_473);
    for manifest in MODEL_ASSET_CATALOG {
        assert!(
            manifest.required_files.iter().any(|file| matches!(
                file.role,
                ModelFileRole::Weights | ModelFileRole::SlowArGraph | ModelFileRole::BaseTtsGraph
            )),
            "{} must declare model weights",
            manifest.id
        );
        for (index, file) in manifest.required_files.iter().enumerate() {
            assert!(
                matches!(
                    file.role,
                    ModelFileRole::License | ModelFileRole::SpeakerEmbedding
                ) || !manifest.required_files[..index]
                    .iter()
                    .any(|previous| previous.role == file.role),
                "{} declares duplicate file role {:?}",
                manifest.id,
                file.role
            );
        }
    }
}

#[test]
fn defaults_are_resolved_from_the_project_root() {
    let root = Path::new("release-root");
    let assets = ResolvedModelAssets::for_project_root(root);
    let paths = assets.llama_cpp_paths();

    assert_eq!(
        paths.qwen3_asr_model,
        root.join("models")
            .join("Qwen3-ASR-1.7B-GGUF")
            .join("Qwen3-ASR-1.7B.Q4_K_M.gguf")
    );
    assert_eq!(
        paths.qwen3_asr_mmproj,
        root.join("models")
            .join("Qwen3-ASR-1.7B-GGUF")
            .join("Qwen3-ASR-1.7B.mmproj-f16.gguf")
    );
    assert_eq!(
        paths.hunyuan_mt_model,
        root.join("models")
            .join("HY-MT2")
            .join("Hy-MT2-1.8B-Q4_K_M.gguf")
    );
    assert_eq!(assets.catalog_assets().count(), MODEL_ASSET_CATALOG.len());
}

#[test]
fn configuration_overrides_are_still_relative_to_project_root() {
    let config = ModelAssetsConfig::with_directory_overrides(
        Some(PathBuf::from("installed-models")),
        Some(PathBuf::from("custom/qwen")),
        None,
    );
    let assets = config.resolve("release-root");

    assert_eq!(
        assets.models_directory,
        PathBuf::from("release-root/installed-models")
    );
    assert_eq!(
        assets.qwen3_asr.directory(),
        Path::new("release-root/custom/qwen")
    );
    assert_eq!(
        assets.hunyuan_mt.directory(),
        Path::new("release-root/installed-models/HY-MT2")
    );
}

#[test]
fn selected_translation_level_changes_the_runtime_and_download_manifest() {
    let mut config = ModelAssetsConfig::default();
    config.select_asset(ModelAssetId::HunyuanMt7bGguf);
    let assets = config.resolve("release-root");

    assert_eq!(assets.hunyuan_mt.manifest().level, ModelLevel::Big);
    assert_eq!(
        assets.llama_cpp_paths().hunyuan_mt_model,
        Path::new("release-root/models/Hy-MT2-7B-GGUF/Hy-MT2-7B-Q4_K_M.gguf")
    );
    assert_eq!(
        assets
            .asset(ModelAssetId::HunyuanMt7bGguf)
            .manifest()
            .source
            .repository,
        "tencent/Hy-MT2-7B-GGUF"
    );
}

#[test]
fn selecting_an_asset_replaces_only_the_same_capability() {
    let mut config = ModelAssetsConfig::default();
    config.select_asset(ModelAssetId::Qwen3AsrGguf);
    config.select_asset(ModelAssetId::HunyuanMtGguf);
    config.select_asset(ModelAssetId::HunyuanMt7bGguf);

    assert_eq!(
        config.selected_asset_ids().collect::<Vec<_>>(),
        vec![ModelAssetId::Qwen3AsrGguf, ModelAssetId::HunyuanMt7bGguf]
    );
}

#[test]
fn tts_language_packages_compose_and_can_be_deselected() {
    let mut config = ModelAssetsConfig::default();
    config.select_asset(ModelAssetId::Audio8TtsOnnxFp16);
    config.select_asset(ModelAssetId::OpenVoiceV3OnnxFp16);
    assert_eq!(
        config.selected_asset_ids().collect::<Vec<_>>(),
        vec![
            ModelAssetId::Audio8TtsOnnxFp16,
            ModelAssetId::OpenVoiceV3OnnxFp16
        ]
    );
    config.deselect_asset(ModelAssetId::Audio8TtsOnnxFp16);
    assert_eq!(
        config.selected_asset_ids().collect::<Vec<_>>(),
        vec![ModelAssetId::OpenVoiceV3OnnxFp16]
    );
}

#[test]
fn tts_model_variants_for_the_same_language_replace_each_other() {
    let mut config = ModelAssetsConfig::default();
    config.select_asset(ModelAssetId::OpenVoiceV3OnnxFp16);
    config.select_asset(ModelAssetId::OpenVoiceV2OnnxFp16);

    assert_eq!(
        config.selected_asset_ids().collect::<Vec<_>>(),
        vec![ModelAssetId::OpenVoiceV2OnnxFp16]
    );
    let manifest = manifest_for(ModelAssetId::OpenVoiceV2OnnxFp16);
    assert_eq!(manifest.voice_presets.len(), 5);
    assert_eq!(manifest.voice_presets[0].key, "en-us");
    assert_eq!(manifest.installed_bytes(), 255_966_606);
}

#[test]
fn legacy_directory_override_stays_bound_to_its_original_asset() {
    let mut config = ModelAssetsConfig::with_directory_overrides(
        None,
        None,
        Some(PathBuf::from("custom/hy-mt2-normal")),
    );
    config.select_asset(ModelAssetId::HunyuanMt7bGguf);

    let assets = config.resolve("release-root");

    assert_eq!(
        assets.asset(ModelAssetId::HunyuanMtGguf).directory(),
        Path::new("release-root/custom/hy-mt2-normal")
    );
    assert_eq!(
        assets
            .active_asset(ModelCapability::Translation)
            .directory(),
        Path::new("release-root/models/Hy-MT2-7B-GGUF")
    );
}

#[test]
fn runtime_files_are_addressed_by_role_instead_of_manifest_order() {
    let assets = ModelAssetsConfig::default().resolve("release-root");
    let asr = assets.active_asset(ModelCapability::Asr);

    assert!(
        asr.file_path(ModelFileRole::Weights)
            .unwrap()
            .ends_with("Qwen3-ASR-1.7B.Q4_K_M.gguf")
    );
    assert!(
        asr.file_path(ModelFileRole::MultimodalProjection)
            .unwrap()
            .ends_with("Qwen3-ASR-1.7B.mmproj-f16.gguf")
    );
}

#[test]
fn catalog_resolution_keeps_unselected_sizes_available() {
    let assets = ResolvedModelAssets::for_project_root("release-root");
    assert_eq!(
        assets.asset(ModelAssetId::HunyuanMt7bGguf).directory(),
        Path::new("release-root/models/Hy-MT2-7B-GGUF")
    );
    assert_eq!(assets.active_assets().count(), 2);
}

#[test]
fn preflight_reports_each_missing_file_with_its_expected_path() {
    let root = temporary_project_root();
    let assets = ResolvedModelAssets::for_project_root(&root);
    let preflight = assets.check();

    assert!(!preflight.is_ready());
    assert_eq!(preflight.diagnostics().len(), 3);
    assert!(preflight.diagnostics().iter().any(|diagnostic| {
        diagnostic.asset_id == ModelAssetId::Qwen3AsrGguf
            && diagnostic.problem == ModelAssetProblem::Missing
            && diagnostic.path.ends_with("Qwen3-ASR-1.7B.Q4_K_M.gguf")
    }));
    assert!(
        preflight
            .into_result()
            .unwrap_err()
            .to_string()
            .contains("default GGUF assets are not ready")
    );
}

#[test]
fn preflight_accepts_files_and_rejects_a_directory_in_their_place() {
    let root = temporary_project_root();
    let assets = ResolvedModelAssets::for_project_root(&root);
    for asset in assets.active_assets() {
        fs::create_dir_all(asset.directory()).unwrap();
        for index in 0..asset.manifest().required_files.len() {
            fs::write(asset.required_file_path(index), b"fixture").unwrap();
        }
    }
    assert!(assets.check().is_ready());

    let mmproj = assets.qwen3_asr.required_file_path(1);
    fs::remove_file(&mmproj).unwrap();
    fs::create_dir(&mmproj).unwrap();
    let preflight = assets.check();

    assert_eq!(preflight.diagnostics().len(), 1);
    assert_eq!(
        preflight.diagnostics()[0].problem,
        ModelAssetProblem::NotAFile
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_integrity_verification_accepts_matching_files_and_reports_hash_tampering() {
    let root = temporary_project_root();
    let directory = root.join("staging");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("fixture.gguf");
    fs::write(&path, b"fixture").unwrap();
    let digest = Box::leak(sha256_file(&path).unwrap().into_boxed_str());
    let files = Box::leak(Box::new([RequiredModelFile {
        role: ModelFileRole::Weights,
        relative_path: "fixture.gguf",
        purpose: "test fixture",
        bytes: 7,
        sha256: digest,
    }]));
    let manifest = Box::leak(Box::new(ModelAssetManifest {
        id: ModelAssetId::Qwen3AsrGguf,
        label: "fixture",
        capability: ModelCapability::Asr,
        level: ModelLevel::Normal,
        provider: "fixture",
        languages: &[],
        voice_presets: &[],
        hardware: MANAGED_LOCAL_MODEL_HARDWARE,
        audio_output: None,
        relative_directory: "fixture",
        required_files: files,
        source: ModelSource {
            repository: "fixture/repository",
            revision: "0000000000000000000000000000000000000000",
            include_patterns: &["fixture.gguf"],
            file_overrides: &[],
            archive: None,
        },
    }));
    let asset = ResolvedModelAsset::new(manifest, directory);
    assert!(asset.verify_integrity().is_empty());

    fs::write(asset.required_file_path(0), b"changed").unwrap();
    let problems = asset.verify_integrity();
    assert!(matches!(
        problems.as_slice(),
        [ModelAssetDiagnostic {
            problem: ModelAssetProblem::HashMismatch { .. },
            ..
        }]
    ));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_staging_directory_is_promoted_without_overwriting_an_install() {
    let root = temporary_project_root();
    let staging = root.join("models").join(".staging-fixture");
    fs::create_dir_all(&staging).unwrap();
    let staged_file = staging.join("fixture.gguf");
    fs::write(&staged_file, b"fixture").unwrap();
    let digest = Box::leak(sha256_file(&staged_file).unwrap().into_boxed_str());
    let files = Box::leak(Box::new([RequiredModelFile {
        role: ModelFileRole::Weights,
        relative_path: "fixture.gguf",
        purpose: "test fixture",
        bytes: 7,
        sha256: digest,
    }]));
    let manifest = Box::leak(Box::new(ModelAssetManifest {
        id: ModelAssetId::HunyuanMtGguf,
        label: "fixture",
        capability: ModelCapability::Translation,
        level: ModelLevel::Normal,
        provider: "fixture",
        languages: &[],
        voice_presets: &[],
        hardware: MANAGED_LOCAL_MODEL_HARDWARE,
        audio_output: None,
        relative_directory: "fixture",
        required_files: files,
        source: ModelSource {
            repository: "fixture/repository",
            revision: "0000000000000000000000000000000000000000",
            include_patterns: &["fixture.gguf"],
            file_overrides: &[],
            archive: None,
        },
    }));
    let target = ResolvedModelAsset::new(manifest, root.join("models").join("fixture"));

    let installed = install_verified_directory(&target, &staging).unwrap();
    assert_eq!(installed, target.directory());
    assert!(installed.join("fixture.gguf").is_file());
    assert!(!staging.exists());
    let second_staging = root.join("models").join("second-staging");
    fs::create_dir_all(&second_staging).unwrap();
    fs::write(second_staging.join("fixture.gguf"), b"fixture").unwrap();
    assert!(matches!(
        install_verified_directory(&target, &second_staging),
        Err(AtomicInstallError::DestinationExists(_))
    ));

    fs::remove_dir_all(root).unwrap();
}
