//! Provider-specific configuration parsing and adapter construction.

use serde_json::Map;
use xrtranslate_config::AppConfig;

pub(super) mod audio8;
pub(super) mod openvoice;

fn provider_object<'a>(
    config: &'a AppConfig,
    provider: &str,
) -> Result<&'a Map<String, serde_json::Value>, String> {
    config
        .tts
        .provider_config(provider)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("tts.providers.{provider} must be an object"))
}

fn threads(values: &Map<String, serde_json::Value>) -> usize {
    values
        .get("threads")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(5)
}

fn supported_languages(
    values: &Map<String, serde_json::Value>,
    provider: &str,
) -> Result<Vec<String>, String> {
    let Some(languages) = values.get("supported_languages") else {
        return Ok(Vec::new());
    };
    let languages = languages
        .as_array()
        .ok_or_else(|| format!("tts.providers.{provider}.supported_languages must be an array"))?;
    languages
        .iter()
        .map(|language| {
            language
                .as_str()
                .map(normalized_language)
                .filter(|language| !language.is_empty())
                .ok_or_else(|| {
                    format!(
                        "tts.providers.{provider}.supported_languages must contain language tags"
                    )
                })
        })
        .collect()
}

fn normalized_language(language: &str) -> String {
    language.trim().replace('_', "-").to_ascii_lowercase()
}
