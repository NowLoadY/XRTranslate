//! Upgrade shipped Qwen presets without rewriting custom remote services.

use serde_json::Value;

const OLD_MODEL: &str = "qwen3-asr-flash";
const NEW_MODEL: &str = "qwen-audio-3.0-asr-flash";

/// Run on both the base document and each minimal user override before merge.
/// A key-only override inherits new defaults. Custom old routes remain explicit
/// and receive validation errors instead of silently using a different API.
pub(crate) fn migrate_qwen_asr_defaults(document: &mut Value) {
    for (provider, host) in [
        ("qwen", "dashscope.aliyuncs.com"),
        ("qwen-intl", "dashscope-intl.aliyuncs.com"),
    ] {
        let Some(settings) = document
            .pointer_mut(&format!("/asr/providers/{provider}"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if !["model", "url", "transport"]
            .iter()
            .any(|key| settings.contains_key(*key))
        {
            continue;
        }
        let old_url = format!("https://{host}/compatible-mode/v1/chat/completions");
        let new_url =
            format!("https://{host}/api/v1/services/aigc/multimodal-generation/generation");
        let model = settings.get("model").and_then(Value::as_str).map(str::trim);
        let url = settings
            .get("url")
            .and_then(Value::as_str)
            .map(|url| url.trim().trim_end_matches('/'));
        let transport = settings
            .get("transport")
            .and_then(Value::as_str)
            .map(str::trim);
        let native_model = model.is_some_and(|model| {
            xrtranslate_assets::remote::remote_asr_model(provider, "dashscope", model).is_some()
        });
        let old_model = model.is_none_or(|model| {
            matches!(
                model,
                OLD_MODEL | "qwen3-asr-flash-2025-09-08" | "qwen3-asr-flash-2026-02-10"
            )
        });
        let standard_url = url.is_none_or(|url| url == old_url || url == new_url);
        let standard_transport =
            transport.is_none_or(|value| matches!(value, "openai" | "dashscope"));
        if (old_model || native_model) && standard_url && standard_transport {
            if !native_model {
                settings.insert("model".into(), Value::from(NEW_MODEL));
            }
            settings.insert("url".into(), Value::from(new_url));
            settings.insert("transport".into(), Value::from("dashscope"));
            settings.insert("asr_prompt_mode".into(), Value::from("context_bias"));
            settings.insert("supports_prompt_context".into(), Value::from(true));
            settings.insert("asr_context_max_chars".into(), Value::from(400));
            settings.insert("supports_vocabulary_bias".into(), Value::from(true));
        } else if !native_model && transport != Some("dashscope") && url != Some(new_url.as_str()) {
            // Materialize the previous inherited fields before the new base is
            // merged. Keep credentials, custom model and custom URL untouched.
            settings
                .entry("model")
                .or_insert_with(|| Value::from(OLD_MODEL));
            settings
                .entry("url")
                .or_insert_with(|| Value::from(old_url));
            settings
                .entry("transport")
                .or_insert_with(|| Value::from("openai"));
        }
    }
}
