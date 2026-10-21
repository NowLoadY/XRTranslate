use xrtranslate_config::{BundledModelAsset, DenoiseConfig, ModelManagerConfig, RuntimeLayout};

#[test]
fn noise_suppression_is_enabled_by_default_and_respects_explicit_opt_out() {
    assert!(DenoiseConfig::default().enabled);
    assert!(serde_json::from_str::<DenoiseConfig>("{}").unwrap().enabled);
    assert!(
        !serde_json::from_str::<DenoiseConfig>(r#"{"enabled":false}"#)
            .unwrap()
            .enabled
    );
}

#[test]
fn legacy_required_resources_no_longer_install_gtcrn() {
    let expected = ModelManagerConfig::default_bundled_models();
    assert!(
        expected
            .iter()
            .all(|asset| asset.relative_path != RuntimeLayout::DENOISE_MODEL_PATH)
    );
    let mut manager = ModelManagerConfig {
        bundled_models: expected.clone(),
        ..Default::default()
    };
    manager.bundled_models.push(BundledModelAsset {
        name: "gtcrn_simple.onnx".into(),
        relative_path: RuntimeLayout::DENOISE_MODEL_PATH.replace('/', "\\"),
        bytes: RuntimeLayout::DENOISE_MODEL_BYTES,
        ..Default::default()
    });
    assert_eq!(manager.resolved_bundled_models(), expected);
}
