use serde_json::{Value, json};
use xrtranslate_config::{
    AppConfig, TextModelSelection, default_context_window, model_context_window,
};

fn document() -> Value {
    let mut raw: Value = serde_json::from_str(include_str!("../../../config.json")).unwrap();
    raw["translation"]["provider"] = json!("qwen");
    raw["translation"]["providers"]["qwen"]["api_key"] = json!("test-only");
    raw
}

fn selected(config: &AppConfig, provider: &str, model: &str) -> AppConfig {
    config
        .with_text_model(&TextModelSelection {
            provider: provider.into(),
            model: model.into(),
        })
        .unwrap()
}

fn context(config: &AppConfig) -> u32 {
    config
        .native_model_route()
        .unwrap()
        .translation
        .runtime
        .context_window_tokens
}

#[test]
fn each_model_remembers_its_own_value_and_default() {
    let mut raw = document();
    raw["translation"]["providers"]["qwen"]["context_windows"] = json!({
        "qwen-mt-plus": 32768, "qwen-plus": 24576
    });
    let config = AppConfig::from_value(raw).unwrap();
    let mt = selected(&config, "qwen", "qwen-mt-plus");
    let chat = selected(&mt, "qwen", "qwen-plus");
    assert_eq!(context(&mt), 32768);
    assert_eq!(context(&chat), 24576);
    assert_eq!(context(&selected(&chat, "qwen", "qwen-mt-plus")), 32768);
    assert_eq!(
        default_context_window(
            "translation",
            "qwen",
            &mt.raw["translation"]["providers"]["qwen"]
        ),
        8192
    );
    assert_eq!(
        default_context_window(
            "translation",
            "qwen",
            &chat.raw["translation"]["providers"]["qwen"]
        ),
        32768
    );
    assert_eq!(context(&selected(&chat, "qwen", "qwen-mt-lite")), 8192);
}

#[test]
fn local_packages_and_agent_share_the_same_model_key() {
    let mut raw = document();
    raw["asr"]["providers"]["qwen3-gguf"]["context_windows"] = json!({"qwen3-asr-gguf":8192});
    raw["translation"]["providers"]["hunyuan"]["context_windows"] =
        json!({"hy-mt2":4096,"hy-mt2-big":8192});
    raw["translation"]["providers"]["qwen"]["context_windows"] = json!({"qwen-plus":24576});
    raw["agent"]["model"] = json!({"provider":"qwen","model":"qwen-plus"});
    let config = AppConfig::from_value(raw).unwrap();
    assert_eq!(
        config
            .native_model_route()
            .unwrap()
            .asr
            .runtime
            .context_window_tokens,
        8192
    );
    let smaller_asr = config
        .with_model_assets(&[xrtranslate_assets::ModelAssetId::Qwen3Asr06bQ8Gguf])
        .unwrap();
    assert_eq!(
        smaller_asr
            .native_model_route()
            .unwrap()
            .asr
            .runtime
            .context_window_tokens,
        4800
    );
    assert_eq!(context(&selected(&config, "hunyuan", "hy-mt2")), 4096);
    assert_eq!(context(&selected(&config, "hunyuan", "hy-mt2-big")), 8192);
    assert_eq!(
        context(&selected(&config, "hunyuan", "hy-mt2-1.8b-q2-k")),
        2048
    );
    assert_eq!(
        config
            .agent_provider()
            .unwrap()
            .runtime
            .context_window_tokens,
        24576
    );
}

#[test]
fn unknown_models_have_a_common_baseline_and_separate_memories() {
    let mut raw = document();
    raw["translation"]["providers"]["custom"] = json!({
        "connector":"openai-chat", "url":"https://example.invalid/v1/chat/completions",
        "api_key":"test-only", "model":"future-one", "max_tokens":1024,
        "context_windows":{"future-one":16384}
    });
    let config = AppConfig::from_value(raw).unwrap();
    assert_eq!(context(&selected(&config, "custom", "future-one")), 16384);
    assert_eq!(context(&selected(&config, "custom", "future-two")), 8192);
    assert_eq!(context(&selected(&config, "qwen", "future-one")), 8192);
}

#[test]
fn saving_and_resetting_never_changes_shipped_defaults_or_other_models() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let base = serde_json::to_string_pretty(&document()).unwrap();
    std::fs::write(&path, &base).unwrap();
    let mut config = AppConfig::from_value(document()).unwrap();
    config.raw["translation"]["providers"]["qwen"]["context_windows"] =
        json!({"qwen-mt-plus":16384,"qwen-plus":24576});
    xrtranslate_config::save_user_config_document(&path, directory.path(), &config.raw).unwrap();
    let restored = AppConfig::from_path_with_user_config(&path, directory.path()).unwrap();
    assert_eq!(context(&selected(&restored, "qwen", "qwen-mt-plus")), 16384);
    let mut reset = restored.raw;
    reset["translation"]["providers"]["qwen"]["context_windows"]
        .as_object_mut()
        .unwrap()
        .remove("qwen-mt-plus");
    xrtranslate_config::save_user_config_document(&path, directory.path(), &reset).unwrap();
    let restored = AppConfig::from_path_with_user_config(&path, directory.path()).unwrap();
    let mt = selected(&restored, "qwen", "qwen-mt-plus");
    assert_eq!(context(&mt), 8192);
    assert!(
        !model_context_window(
            "translation",
            "qwen",
            &mt.raw["translation"]["providers"]["qwen"]
        )
        .unwrap()
        .customized
    );
    assert_eq!(context(&selected(&restored, "qwen", "qwen-plus")), 24576);
    assert_eq!(std::fs::read_to_string(path).unwrap(), base);
}

#[test]
fn legacy_value_belongs_only_to_the_previously_selected_model() {
    let mut raw = document();
    raw["translation"]["providers"]["qwen"]["model"] = json!("qwen-mt-plus");
    raw["translation"]["providers"]["qwen"]["context_window_tokens"] = json!(12345);
    let config = AppConfig::from_value(raw).unwrap();
    assert_eq!(context(&config), 12345);
    assert_eq!(context(&selected(&config, "qwen", "qwen-plus")), 32768);
    assert_eq!(
        context(&selected(
            &selected(&config, "qwen", "qwen-plus"),
            "qwen",
            "qwen-mt-plus"
        )),
        12345
    );
    assert!(
        config.raw["translation"]["providers"]["qwen"]
            .get("context_window_tokens")
            .is_none()
    );
}

#[test]
fn an_old_base_value_cannot_override_a_different_model_selected_by_the_user() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let mut base = document();
    base["translation"]["providers"]["qwen"]["model"] = json!("qwen-mt-plus");
    base["translation"]["providers"]["qwen"]["context_window_tokens"] = json!(8192);
    std::fs::write(&path, serde_json::to_vec(&base).unwrap()).unwrap();
    let override_path = xrtranslate_config::RuntimeLayout::user_config_path(directory.path());
    std::fs::create_dir_all(override_path.parent().unwrap()).unwrap();
    std::fs::write(
        override_path,
        serde_json::to_vec(&json!({"translation":{"providers":{"qwen":{"model":"qwen-plus"}}}}))
            .unwrap(),
    )
    .unwrap();
    let config = AppConfig::from_path_with_user_config(path, directory.path()).unwrap();
    assert_eq!(context(&config), 32768);
}
