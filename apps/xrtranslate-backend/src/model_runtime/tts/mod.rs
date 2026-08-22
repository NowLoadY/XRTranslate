//! Native TTS provider registry and adapter factory.
//!
//! This entry module is the composition map: provider-specific configuration
//! lives below `providers/`, while session code consumes only the erased
//! [`NativeTtsAdapter`].

mod adapter;
mod providers;

use std::path::Path;

use xrtranslate_assets::{ModelAssetId, ModelCapability};
use xrtranslate_config::AppConfig;

pub(crate) use adapter::NativeTtsAdapter;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TtsProfile {
    provider: &'static str,
    transport: &'static str,
    default_asset: ModelAssetId,
}

const TTS_PROFILES: &[TtsProfile] = &[
    TtsProfile {
        provider: "openvoice",
        transport: "onnx",
        default_asset: ModelAssetId::OpenVoiceV3OnnxFp16,
    },
    TtsProfile {
        provider: "audio8",
        transport: "onnx",
        default_asset: ModelAssetId::Audio8TtsOnnxFp16,
    },
];

impl TtsProfile {
    pub(super) fn selected(config: &AppConfig) -> Result<Option<Self>, String> {
        let provider = config.tts.provider.trim();
        if provider.is_empty() || provider.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let values = config
            .tts
            .provider_config(provider)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| format!("tts.providers.{provider} must be an object"))?;
        let transport = values
            .get("transport")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        Self::registered(provider, transport)
            .map(Some)
            .ok_or_else(|| format!("unsupported TTS provider {provider:?} over {transport:?}"))
    }

    pub(super) fn registered(provider: &str, transport: &str) -> Option<Self> {
        TTS_PROFILES
            .iter()
            .copied()
            .find(|profile| profile.provider == provider && profile.transport == transport)
    }

    pub(super) const fn default_asset(self) -> ModelAssetId {
        self.default_asset
    }

    pub(super) fn configured_asset(self, config: &AppConfig) -> Result<ModelAssetId, String> {
        let provider = config
            .tts
            .provider_config(self.provider)
            .and_then(serde_json::Value::as_object)
            .expect("selected TTS profile has a provider object");
        let Some(key) = provider
            .get("model_asset")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(self.default_asset());
        };
        let id = ModelAssetId::from_config_key(key).ok_or_else(|| {
            format!(
                "unknown model asset {key:?} for TTS provider {:?}",
                self.provider
            )
        })?;
        let manifest = xrtranslate_assets::manifest_for(id);
        if manifest.provider != self.provider || manifest.capability != ModelCapability::Tts {
            return Err(format!(
                "model asset {key:?} does not belong to TTS provider {:?}",
                self.provider
            ));
        }
        Ok(id)
    }

    pub(super) fn adapter(
        self,
        config: &AppConfig,
        model_directory: &Path,
    ) -> Result<NativeTtsAdapter, String> {
        match self.provider {
            "openvoice" => providers::openvoice::build(config, model_directory),
            "audio8" => providers::audio8::build(config, model_directory),
            _ => unreachable!("only registered profiles reach the TTS factory"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_tts(provider: &str) -> AppConfig {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../config.json")).unwrap();
        document["tts"]["provider"] = serde_json::Value::from(provider);
        AppConfig::from_value(document).unwrap()
    }

    #[test]
    fn none_selects_no_native_tts_profile() {
        assert_eq!(
            TtsProfile::selected(&config_with_tts("none")).unwrap(),
            None
        );
    }

    #[test]
    fn audio8_profile_owns_its_default_asset() {
        let profile = TtsProfile::selected(&config_with_tts("audio8"))
            .unwrap()
            .unwrap();
        assert_eq!(profile.default_asset(), ModelAssetId::Audio8TtsOnnxFp16);
    }

    #[test]
    fn openvoice_profile_owns_the_ngc_v3_asset() {
        let profile = TtsProfile::selected(&config_with_tts("openvoice"))
            .unwrap()
            .unwrap();
        assert_eq!(profile.default_asset(), ModelAssetId::OpenVoiceV3OnnxFp16);
    }

    #[test]
    fn unknown_provider_is_rejected_at_the_factory_boundary() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../config.json")).unwrap();
        document["tts"]["provider"] = serde_json::Value::from("future");
        document["tts"]["providers"]["future"] = serde_json::json!({ "transport": "onnx" });
        let config = AppConfig::from_value(document).unwrap();
        assert!(
            TtsProfile::selected(&config)
                .unwrap_err()
                .contains("unsupported TTS provider")
        );
    }
}
