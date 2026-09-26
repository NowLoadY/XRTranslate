//! Language capability metadata shared by configuration and provider adapters.

use crate::{
    ModelAssetId, ModelAssetManifest, ModelCapability, ModelLevel, manifest_for,
    tier_default_manifest,
};

pub const QWEN_AUDIO_STREAMING_LANGUAGES: &[&str] = &[
    "zh", "en", "ja", "ko", "vi", "th", "id", "ms", "fil", "tl", "hi", "ar", "fr", "de", "es",
    "pt", "ru", "it", "nl", "sv", "da", "fi", "no", "el", "pl", "cs", "hu", "ro", "bg", "hr", "sk",
];

pub fn provider_model(
    provider: &str,
    asset: Option<&str>,
    capability: ModelCapability,
) -> Result<&'static ModelAssetManifest, String> {
    let model = match asset {
        Some(key) => ModelAssetId::from_config_key(key).map(manifest_for),
        None => tier_default_manifest(provider, capability, ModelLevel::Normal),
    }
    .ok_or_else(|| format!("No model card for {provider:?} {capability:?} ({asset:?})"))?;
    if model.provider != provider || model.capability != capability {
        return Err(format!(
            "Model {} does not belong to {provider:?} for {capability:?}",
            model.id.as_str()
        ));
    }
    Ok(model)
}

/// Generic remote endpoints do not declare a finite language set.
pub fn remote_asr_languages(provider: &str, transport: &str) -> Option<&'static [&'static str]> {
    match (provider, transport) {
        ("qwen-audio-streaming", "websocket") => Some(QWEN_AUDIO_STREAMING_LANGUAGES),
        ("qwen" | "qwen-intl", _) => Some(crate::QWEN3_ASR_GGUF.languages),
        _ => None,
    }
}
