use serde_json::json;
use xrtranslate_config::{AppConfig, TextModelSelection};

fn config() -> AppConfig {
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("../../../config.json")).unwrap();
    raw["translation"]["provider"] = json!("qwen");
    raw["translation"]["providers"]["qwen"]["api_key"] = json!("test-only");
    AppConfig::from_value(raw).unwrap()
}

fn selection(provider: &str, model: &str) -> TextModelSelection {
    TextModelSelection {
        provider: provider.into(),
        model: model.into(),
    }
}

#[test]
fn online_context_can_exceed_local_limits_without_expanding_translation_history() {
    let base = config();
    let mut raw = base.raw.clone();
    raw["translation"]["provider"] = json!("ChatGPT Account");
    raw["translation"]["providers"]["ChatGPT Account"]["model"] = json!("account-model");
    let configured = AppConfig::from_value(raw.clone()).unwrap();
    assert_eq!(
        configured
            .agent_provider()
            .unwrap()
            .runtime
            .context_window_tokens,
        131_072
    );
    assert!(
        configured.for_agent().unwrap().is_none(),
        "following shares the translation runtime"
    );
    assert_eq!(configured.raw["prompt_context"], base.raw["prompt_context"]);

    for window in [8_192, 262_144, 1_048_576] {
        raw["translation"]["providers"]["ChatGPT Account"]["context_window_tokens"] = json!(window);
        let configured = AppConfig::from_value(raw.clone()).unwrap();
        assert_eq!(
            configured
                .agent_provider()
                .unwrap()
                .runtime
                .context_window_tokens,
            window
        );
    }
    raw["translation"]["providers"]["ChatGPT Account"]["context_window_tokens"] = json!(1_048_577);
    assert!(
        AppConfig::from_value(raw)
            .unwrap()
            .native_model_route()
            .is_err()
    );

    let mut local = base.raw.clone();
    local["translation"]["provider"] = json!("hunyuan");
    local["translation"]["providers"]["hunyuan"]["context_window_tokens"] = json!(131_072);
    assert!(
        AppConfig::from_value(local)
            .unwrap()
            .native_model_route()
            .is_err()
    );
}

#[test]
fn independent_agent_reuses_credentials_without_changing_translation() {
    let base = config();
    assert!(base.for_agent().unwrap().is_none());
    assert!(!base.agent_provider().unwrap().supports_chat().unwrap());
    let original = base.native_model_route().unwrap().translation;
    let mut raw = base.raw.clone();
    raw["agent"] = json!({"model":selection("qwen", "qwen-plus")});
    let config = AppConfig::from_value(raw).unwrap();
    let agent = config.agent_provider().unwrap();
    assert_eq!(config.native_model_route().unwrap().translation, original);
    assert_eq!(agent.model, "qwen-plus");
    assert_eq!(agent.api_key, original.api_key);
    assert!(agent.supports_chat().unwrap());
    assert_eq!(agent.runtime.context_window_tokens, 1_000_000);
    assert!(
        config
            .for_agent()
            .unwrap()
            .unwrap()
            .for_agent()
            .unwrap()
            .is_none()
    );

    let same = config
        .with_text_model(&selection("qwen", "qwen-plus"))
        .unwrap();
    assert!(
        same.for_agent().unwrap().is_none(),
        "identical route should share its runtime"
    );
    let mut invalid = config.raw.clone();
    invalid["agent"]["model"] = json!(selection("qwen", "qwen-mt-flash"));
    assert!(AppConfig::from_value(invalid).unwrap().for_agent().is_err());

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, serde_json::to_vec(&base.raw).unwrap()).unwrap();
    xrtranslate_config::save_user_config_document(&path, directory.path(), &config.raw).unwrap();
    let saved = AppConfig::from_path_with_user_config(&path, directory.path()).unwrap();
    assert_eq!(saved.agent_provider().unwrap(), agent);
    let mut follow = saved.raw;
    follow["agent"]["model"] = serde_json::Value::Null;
    xrtranslate_config::save_user_config_document(&path, directory.path(), &follow).unwrap();
    let restored = AppConfig::from_path_with_user_config(&path, directory.path()).unwrap();
    assert!(restored.for_agent().unwrap().is_none());
    assert_eq!(restored.native_model_route().unwrap().translation, original);
}

#[test]
fn openai_agent_prefers_responses_and_honors_per_model_window_overrides() {
    use xrtranslate_assets::remote::{RemoteConnector, openai_agent_limits};
    let mut raw = config().raw;
    raw["translation"]["providers"]["openai"]["api_key"] = json!("fixture");
    raw["agent"]["model"] = json!(selection("openai", "gpt-6.2-sol"));
    let configured = AppConfig::from_value(raw.clone()).unwrap();
    let choices = configured.chat_model_selections();
    assert_eq!(
        choices.iter().find(|item| item.provider == "openai"),
        Some(&selection("openai", "gpt-6.2-sol"))
    );
    let agent = configured.agent_provider().unwrap();
    assert_eq!(agent.connector, Some(RemoteConnector::OpenaiResponses));
    assert_eq!(agent.url, "https://api.openai.com/v1/responses");
    assert_eq!(agent.api_key.as_deref(), Some("fixture"));
    assert_eq!(agent.runtime.context_window_tokens, 1_050_000);
    assert_eq!(openai_agent_limits(1_050_000), (922_000, 25_000));
    assert_eq!(
        configured
            .native_model_route()
            .unwrap()
            .translation
            .provider,
        "qwen"
    );

    raw["translation"]["providers"]["openai"]["context_windows"]["gpt-6.2-sol"] = json!(131_072);
    let adjusted = AppConfig::from_value(raw.clone())
        .unwrap()
        .agent_provider()
        .unwrap();
    assert_eq!(adjusted.runtime.context_window_tokens, 131_072);
    assert_eq!(openai_agent_limits(131_072), (105_816, 25_000));

    raw["agent"]["model"] = json!(selection("openai", "gpt-4o-mini"));
    let mini = AppConfig::from_value(raw)
        .unwrap()
        .agent_provider()
        .unwrap();
    assert_eq!(mini.connector, Some(RemoteConnector::OpenaiChat));
    assert_eq!(mini.runtime.context_window_tokens, 128_000);
}

#[test]
fn qwen_agent_uses_the_selected_regional_service_and_respects_custom_gateways() {
    use xrtranslate_assets::remote::AgentApi;
    let mut raw = config().raw;
    for (provider, region) in [("qwen", "cn-beijing"), ("qwen-intl", "ap-southeast-1")] {
        raw["translation"]["providers"][provider]["api_key"] = json!("fixture");
        raw["translation"]["providers"][provider]["workspace_id"] = json!("test-workspace");
        raw["agent"]["model"] = json!(selection(provider, "qwen3.8-flash"));
        let configured = AppConfig::from_value(raw.clone()).unwrap();
        let agent = configured.agent_provider().unwrap();
        let expected = format!(
            "https://test-workspace.{region}.maas.aliyuncs.com/compatible-mode/v1/responses"
        );
        assert_eq!(
            agent.agent_endpoint,
            Some((AgentApi::DashscopeResponses, expected.clone()))
        );
        assert!(agent.url.ends_with("/chat/completions"));
        assert_eq!(agent.runtime.context_window_tokens, 1_000_000);
        assert_eq!(
            configured
                .chat_model_selections()
                .iter()
                .find(|s| s.provider == provider),
            Some(&selection(provider, "qwen3.8-flash"))
        );
        let mut custom = raw.clone();
        custom["translation"]["providers"][provider]["url"] =
            json!(expected.replace("/responses", "/chat/completions"));
        assert_eq!(
            AppConfig::from_value(custom.clone())
                .unwrap()
                .agent_provider()
                .unwrap()
                .agent_endpoint,
            agent.agent_endpoint
        );
        custom["translation"]["providers"][provider]["url"] =
            json!("https://private.invalid/v1/chat/completions");
        assert!(
            AppConfig::from_value(custom)
                .unwrap()
                .agent_provider()
                .unwrap()
                .agent_endpoint
                .is_none()
        );
    }
}

#[test]
fn chat_choices_use_capability_records_not_transport_or_credentials() {
    let mut raw = config().raw;
    raw["translation"]["providers"]["qwen"]["api_key"] = json!("");
    raw["translation"]["providers"]["custom"] = json!({
        "connector":"openai-chat", "url":"https://example.invalid/v1/chat/completions", "model":"unverified"
    });
    let config = AppConfig::from_value(raw.clone()).unwrap();
    let choices = config.chat_model_selections();
    assert!(choices.contains(&selection("qwen", "qwen-plus")));
    assert!(!choices.contains(&selection("qwen", "qwen-mt-plus")));
    assert!(!choices.contains(&selection("custom", "unverified")));
    raw["translation"]["providers"]["custom"]["capabilities"] = json!({"supports_chat":true});
    let config = AppConfig::from_value(raw).unwrap();
    assert!(
        config
            .chat_model_selections()
            .contains(&selection("custom", "unverified"))
    );
}

#[test]
fn local_translation_capabilities_remain_explicit_without_loading_models() {
    let config = config();
    for (provider, model, ask) in [
        ("hunyuan", "hy-mt2", true),
        ("hunyuan", "hy-mt2-1.8b-q2-k", true),
        ("hunyuan", "hy-mt2-big", true),
        ("haidass", "haidass-translate-143m-q8-gguf", false),
    ] {
        // Resolve from the catalogue, independent of local package installation.
        let id = xrtranslate_assets::ModelAssetId::from_config_key(model).unwrap();
        let manifest = xrtranslate_assets::manifest_for(id);
        let selected = config.with_model_assets(&[id]).unwrap();
        let route = selected.native_model_route().unwrap().translation;
        assert_eq!(route.provider, manifest.provider, "{provider}");
        assert_eq!(route.supports_ask().unwrap(), ask, "{model}");
        assert!(!route.supports_chat().unwrap(), "{model}");
    }
}
