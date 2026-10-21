use std::fs;

use xrtranslate_assets::{
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelFileRole, ModelRuntime,
    NativeModelInstaller,
};

#[test]
fn gtcrn_is_optional_and_keeps_the_existing_model_path() {
    let root = std::env::temp_dir().join("xrtranslate-gtcrn-catalog");
    let assets = ModelAssetsConfig::default().resolve(&root);
    let id = ModelAssetId::GtcrnOnnx;
    assert!(
        !assets
            .active_assets()
            .any(|asset| asset.manifest().id == id)
    );
    assert!(
        assets
            .check()
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.asset_id != id)
    );
    let asset = assets.asset(id);
    assert_eq!(
        asset.manifest().capability,
        ModelCapability::NoiseSuppression
    );
    assert_eq!(asset.manifest().runtime, Some(ModelRuntime::GtcrnOnnx));
    assert_eq!(
        asset.file_path(ModelFileRole::Weights).unwrap(),
        root.join("models/gtcrn/gtcrn_simple.onnx")
    );
    assert_eq!(asset.manifest().source.revision, "v0.3.4");
    assert_eq!(
        asset.manifest().source.file_url("gtcrn_simple.onnx"),
        "https://github.com/NowLoadY/XRTranslate/releases/download/v0.3.4/gtcrn_simple.onnx"
    );
    assert_eq!(
        asset.manifest().source.file_url("LICENSE"),
        "https://github.com/NowLoadY/XRTranslate/releases/download/v0.3.4/GTCRN-LICENSE.txt"
    );
}

#[test]
fn legacy_gtcrn_installation_only_adds_its_missing_license() {
    let root =
        std::env::temp_dir().join(format!("xrtranslate-legacy-gtcrn-{}", uuid::Uuid::new_v4()));
    let assets = ModelAssetsConfig::default().resolve_selected(&root);
    let id = ModelAssetId::GtcrnOnnx;
    let asset = assets.asset(id);
    fs::create_dir_all(asset.directory()).unwrap();
    let weights = asset.file_path(ModelFileRole::Weights).unwrap();
    let original = vec![42; 535638];
    fs::write(&weights, &original).unwrap();
    let staging = root.join("models/.xrtranslate-staging").join(format!(
        "{}-{}",
        id.as_str(),
        asset.manifest().source.revision
    ));
    fs::create_dir_all(&staging).unwrap();
    let notice = vec![b'L'; 1069];
    fs::write(staging.join("LICENSE"), &notice).unwrap();
    let installer = NativeModelInstaller::new(assets.clone()).unwrap();
    let mut progress = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(installer.install(id, |value| progress.push(value)))
        .unwrap();
    assert_eq!(fs::read(weights).unwrap(), original);
    assert_eq!(
        fs::read(asset.file_path(ModelFileRole::License).unwrap()).unwrap(),
        notice
    );
    assert!(asset.check().is_empty());
    assert_eq!(progress.first().unwrap().downloaded_bytes, 535638);
    assert_eq!(progress.last().unwrap().downloaded_bytes, 536707);
    assert!(!staging.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pausing_an_install_preserves_its_partial_staging() {
    let root =
        std::env::temp_dir().join(format!("xrtranslate-paused-gtcrn-{}", uuid::Uuid::new_v4()));
    let assets = ModelAssetsConfig::default().resolve_selected(&root);
    let id = ModelAssetId::GtcrnOnnx;
    let asset = assets.asset(id);
    let staging = root.join("models/.xrtranslate-staging").join(format!(
        "{}-{}",
        id.as_str(),
        asset.manifest().source.revision
    ));
    fs::create_dir_all(&staging).unwrap();
    let partial = staging.join("gtcrn_simple.onnx.part");
    fs::write(&partial, b"resume-me").unwrap();
    let cancellation = xrtranslate_download::DownloadCancellation::default();
    cancellation.cancel();
    let installer = NativeModelInstaller::with_download_source_and_cancellation(
        assets.clone(),
        None,
        xrtranslate_download::DownloadSource::Official,
        cancellation,
    )
    .unwrap();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(installer.install(id, |_| {}));
    assert!(result.unwrap_err().is_cancelled());
    assert_eq!(fs::read(partial).unwrap(), b"resume-me");
    assert!(!asset.directory().exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_existing_model_with_the_wrong_size_is_never_overwritten() {
    let root = std::env::temp_dir().join(format!(
        "xrtranslate-invalid-gtcrn-{}",
        uuid::Uuid::new_v4()
    ));
    let assets = ModelAssetsConfig::default().resolve_selected(&root);
    let id = ModelAssetId::GtcrnOnnx;
    let asset = assets.asset(id);
    fs::create_dir_all(asset.directory()).unwrap();
    let path = asset.file_path(ModelFileRole::Weights).unwrap();
    fs::write(&path, b"preserve-existing-file").unwrap();
    let installer = NativeModelInstaller::new(assets.clone()).unwrap();
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(installer.install(id, |_| {}))
        .unwrap_err();
    assert!(error.to_string().contains("expected 535638 bytes"));
    assert!(!error.allows_source_fallback());
    assert_eq!(fs::read(path).unwrap(), b"preserve-existing-file");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "downloads the optional 0.54 MB public GTCRN package"]
fn public_gtcrn_package_installs_from_the_project_release() {
    let root =
        std::env::temp_dir().join(format!("xrtranslate-public-gtcrn-{}", uuid::Uuid::new_v4()));
    let assets = ModelAssetsConfig::default().resolve_selected(&root);
    let id = ModelAssetId::GtcrnOnnx;
    let installer = NativeModelInstaller::new(assets.clone()).unwrap();
    let mut updates = 0;
    let mut downloaded = 0;
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(installer.install(id, |progress| {
            updates += 1;
            downloaded = progress.downloaded_bytes;
        }))
        .unwrap();
    assert_eq!(result, assets.asset(id).directory());
    assert!(assets.asset(id).check().is_empty());
    assert!(assets.active_assets().next().is_none());
    assert!(updates > 1);
    assert_eq!(downloaded, 536707);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn vocal_separation_is_optional_and_respects_the_shared_model_directory() {
    let root = std::env::temp_dir().join("xrtranslate-model-catalog");
    let config =
        ModelAssetsConfig::with_directory_overrides(Some("custom-models".into()), None, None);
    let assets = config.resolve(&root);
    let id = ModelAssetId::UvrMdxnet9482Onnx;
    assert!(
        !assets
            .active_assets()
            .any(|asset| asset.manifest().id == id)
    );
    assert!(
        assets
            .check()
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.asset_id != id)
    );
    let asset = assets.asset(id);
    assert_eq!(
        asset.manifest().capability,
        ModelCapability::SourceSeparation
    );
    assert_eq!(asset.manifest().runtime, Some(ModelRuntime::UvrMdxOnnx));
    assert_eq!(
        asset.file_path(ModelFileRole::Weights).unwrap(),
        root.join("custom-models/UVR-MDXNET-9482/UVR_MDXNET_9482.onnx")
    );
    assert_eq!(
        asset.manifest().source.file_url("UVR_MDXNET_9482.onnx"),
        "https://huggingface.co/k2-fsa/sherpa-onnx-models/resolve/bee11a0c31f3054fa7e73752e59f50d1e05340af/source-separation-models/UVR_MDXNET_9482.onnx"
    );
}

#[test]
#[ignore = "downloads the optional 29.7 MB public vocal separation package"]
fn public_vocal_separation_package_installs_with_progress() {
    let retained_root = std::env::var_os("XRTRANSLATE_ASSET_TEST_ROOT");
    let root = retained_root.as_ref().map_or_else(
        || std::env::temp_dir().join(format!("xrtranslate-assets-{}", uuid::Uuid::new_v4())),
        std::path::PathBuf::from,
    );
    let assets = ModelAssetsConfig::default().resolve_selected(&root);
    let id = ModelAssetId::UvrMdxnet9482Onnx;
    let asset = assets.asset(id);
    let already_installed = asset.check().is_empty();
    let installer = NativeModelInstaller::new(assets.clone()).unwrap();
    let mut last_downloaded = 0;
    let mut updates = 0;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let installed = runtime
        .block_on(installer.install(id, |progress| {
            assert_eq!(progress.asset_id, id);
            assert_eq!(progress.total_bytes, asset.manifest().download_bytes());
            assert!(progress.downloaded_bytes >= last_downloaded);
            last_downloaded = progress.downloaded_bytes;
            updates += 1;
        }))
        .unwrap();
    assert_eq!(installed, asset.directory());
    assert!(asset.check().is_empty());
    if !already_installed {
        assert!(updates > 1);
        assert_eq!(last_downloaded, asset.manifest().download_bytes());
    }
    assert!(assets.active_assets().next().is_none());
    eprintln!("Verified vocal model: {}", installed.display());
    if retained_root.is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "downloads the optional 461 MiB public OpenVoice Chinese package"]
fn public_openvoice_chinese_package_installs_anonymously() {
    let root = std::env::temp_dir().join(format!("xrtranslate-assets-{}", uuid::Uuid::new_v4()));
    let mut config = ModelAssetsConfig::default();
    config.select_asset(ModelAssetId::OpenVoiceV2ZhOnnxFp16);
    let assets = config.resolve_selected(&root);
    let installer = NativeModelInstaller::new(assets.clone()).unwrap();
    let mut last_downloaded = 0_u64;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let installed = runtime
        .block_on(
            installer.install(ModelAssetId::OpenVoiceV2ZhOnnxFp16, |progress| {
                last_downloaded = progress.downloaded_bytes;
            }),
        )
        .unwrap();
    let asset = assets.asset(ModelAssetId::OpenVoiceV2ZhOnnxFp16);
    assert_eq!(installed, asset.directory());
    assert_eq!(last_downloaded, asset.manifest().download_bytes());
    assert!(asset.check().is_empty());
    fs::remove_dir_all(root).unwrap();
}
