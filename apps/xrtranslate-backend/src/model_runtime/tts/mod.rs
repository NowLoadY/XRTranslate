//! Native TTS provider registry and adapter factory.
//!
//! This entry module is the composition map: provider-specific configuration
//! lives below `providers/`, while session code consumes only the erased
//! [`NativeTtsAdapter`].

mod adapter;
mod providers;

use xrtranslate_assets::{ModelAssetId, ModelCapability, ResolvedModelAsset};
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

    pub(super) fn configured_assets(self, config: &AppConfig) -> Result<Vec<ModelAssetId>, String> {
        let provider = config
            .tts
            .provider_config(self.provider)
            .and_then(serde_json::Value::as_object)
            .expect("selected TTS profile has a provider object");
        let keys = provider
            .get("model_assets")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| {
                provider
                    .get("model_asset")
                    .and_then(serde_json::Value::as_str)
                    .into_iter()
                    .collect()
            });
        if keys.is_empty() {
            return Ok(vec![self.default_asset()]);
        }
        keys.into_iter()
            .map(|key| {
                let id = ModelAssetId::from_config_key(key).ok_or_else(|| {
                    format!(
                        "unknown model asset {key:?} for TTS provider {:?}",
                        self.provider
                    )
                })?;
                let manifest = xrtranslate_assets::manifest_for(id);
                if manifest.provider != self.provider || manifest.capability != ModelCapability::Tts
                {
                    return Err(format!(
                        "model asset {key:?} does not belong to TTS provider {:?}",
                        self.provider
                    ));
                }
                Ok(id)
            })
            .collect()
    }

    pub(super) fn adapter(
        self,
        config: &AppConfig,
        assets: &[&ResolvedModelAsset],
    ) -> Result<NativeTtsAdapter, String> {
        let adapters = assets
            .iter()
            .map(|asset| {
                let languages = asset
                    .manifest()
                    .languages
                    .iter()
                    .map(|language| (*language).to_owned())
                    .collect();
                match self.provider {
                    "openvoice" => providers::openvoice::build(
                        config,
                        asset.manifest().id,
                        asset.directory(),
                        languages,
                    ),
                    "audio8" => providers::audio8::build(config, asset.directory(), languages),
                    _ => unreachable!("only registered profiles reach the TTS factory"),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        NativeTtsAdapter::combine(adapters)
    }
}
