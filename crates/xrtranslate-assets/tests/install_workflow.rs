use std::fs;

use xrtranslate_assets::{ModelAssetId, ModelAssetsConfig, NativeModelInstaller};

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
