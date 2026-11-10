#![cfg(windows)]

use xrtranslate_config::{Distribution, RuntimeLayout, data_root};

#[test]
fn installed_channels_share_writable_data_without_redirecting_bundled_files() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let data =
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("XRTranslate");
    for channel in ["installer", "store", "steam"] {
        std::fs::write(root.join("distribution.json"), format!("\"{channel}\"\n")).unwrap();
        assert_ne!(Distribution::read(root).unwrap(), Distribution::Portable);
        assert_eq!(data_root(root), data);
        let layout = RuntimeLayout::for_project_root(root);
        assert_eq!(layout.runtime_root(), data.join("runtime"));
        assert_eq!(
            RuntimeLayout::new(root, Some("custom-runtime")).runtime_root(),
            data.join("custom-runtime")
        );
        let external_runtime = root.join("external-runtime");
        assert_eq!(
            RuntimeLayout::new(root, Some(&external_runtime)).runtime_root(),
            external_runtime
        );
        assert_eq!(
            RuntimeLayout::user_config_path(root),
            data.join("runtime/user-config.json")
        );
        assert_eq!(
            layout.resolve_configured_path("models/example/model.onnx"),
            data.join("models/example/model.onnx")
        );
        assert_eq!(
            layout.resolve_configured_path("plugins/example"),
            data.join("plugins/example")
        );
        assert_eq!(
            layout.resolve_configured_path("corpora/default.sqlite"),
            root.join("corpora/default.sqlite")
        );
        assert_eq!(
            layout.silero_vad_model_path(),
            root.join(RuntimeLayout::VAD_MODEL_PATH)
        );
        let external = root.join("external/model.gguf");
        assert_eq!(layout.resolve_configured_path(&external), external);
    }
}

#[test]
fn portable_paths_stay_local_and_invalid_distribution_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    assert_eq!(Distribution::read(root).unwrap(), Distribution::Portable);
    assert_eq!(data_root(root), root);
    assert_eq!(
        RuntimeLayout::for_project_root(root).runtime_root(),
        root.join("runtime")
    );
    std::fs::write(root.join("distribution.json"), b"\"portable\"").unwrap();
    assert_eq!(data_root(root), root);
    std::fs::write(root.join("distribution.json"), b"broken").unwrap();
    assert!(Distribution::read(root).is_err());
}
