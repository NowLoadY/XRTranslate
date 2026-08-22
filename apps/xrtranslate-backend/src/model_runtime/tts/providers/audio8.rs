//! Audio8-specific configuration mapping.

use std::path::Path;

use xrtranslate_config::AppConfig;
use xrtranslate_inference::{Audio8OnnxAdapter, Audio8SynthesisOptions, OnnxExecutionDevice};

use super::{provider_object, supported_languages, threads};
use crate::model_runtime::tts::NativeTtsAdapter;

pub(in crate::model_runtime::tts) fn build(
    config: &AppConfig,
    model_directory: &Path,
) -> Result<NativeTtsAdapter, String> {
    let provider = provider_object(config, "audio8")?;
    let string = |key: &str, fallback: &str| {
        provider
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback)
            .to_owned()
    };
    let defaults = Audio8SynthesisOptions::default();
    let integer = |key: &str, fallback: usize| {
        provider
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(fallback)
    };
    let decimal = |key: &str, fallback: f64| {
        provider
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(fallback)
    };
    let adapter = Audio8OnnxAdapter::with_synthesis_options(
        model_directory,
        OnnxExecutionDevice::from_config(&string("device", "auto")),
        threads(provider),
        Audio8SynthesisOptions {
            max_new_tokens: integer("max_new_tokens", defaults.max_new_tokens),
            temperature: decimal("temperature", defaults.temperature),
            top_p: decimal("top_p", defaults.top_p),
            top_k: integer("top_k", defaults.top_k),
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(NativeTtsAdapter::audio8(
        adapter,
        supported_languages(provider, "audio8")?,
    ))
}
