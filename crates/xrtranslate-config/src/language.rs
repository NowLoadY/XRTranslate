use crate::NativeModelRouteConfig;
use xrtranslate_assets::{
    ModelCapability,
    language::{provider_model, remote_asr_languages},
};
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSet};

impl NativeModelRouteConfig {
    /// Resolve model metadata once when service configuration changes. Core and
    /// plugins consume only the resulting model-independent constraints.
    pub fn language_capabilities(&self) -> Result<LanguageCapabilities, String> {
        let support =
            |provider: &crate::NativeProviderConfig, capability| {
                let codes = if provider.uses_local_runtime() {
                    Some(
                        provider_model(
                            &provider.provider,
                            provider.model_asset.as_deref(),
                            capability,
                        )?
                        .languages,
                    )
                } else if capability == ModelCapability::Asr {
                    remote_asr_languages(&provider.provider, &provider.transport)
                } else {
                    None
                };
                Ok::<_, String>(codes.map(|codes| {
                    LanguageSet::from_codes(codes, capability == ModelCapability::Asr)
                }))
            };
        Ok(LanguageCapabilities {
            recognition: support(&self.asr, ModelCapability::Asr)?,
            translation: support(&self.translation, ModelCapability::Translation)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_and_backend_share_local_and_remote_capability_resolution() {
        let config = crate::AppConfig::from_value(
            serde_json::from_str(include_str!("../../../config.json")).unwrap(),
        )
        .unwrap();
        let mut route = config.native_model_route().unwrap();
        route.asr.model_asset = Some("sensevoice-small-int8-onnx".into());
        route.translation.model_asset = Some(
            xrtranslate_assets::HAIDASS_TRANSLATE_143M_Q8_GGUF
                .id
                .as_str()
                .into(),
        );
        route.translation.provider = xrtranslate_assets::HAIDASS_TRANSLATE_143M_Q8_GGUF
            .provider
            .into();
        let languages = route.language_capabilities().unwrap();
        assert!(languages.select("auto", "zh,en").is_ok());
        assert!(languages.select("ja", "en").is_err());
        assert!(languages.select("zh", "zh-TW").is_err());
        route.asr.transport = "websocket".into();
        route.asr.provider = "qwen-audio-streaming".into();
        route.translation.transport = "openai".into();
        let languages = route.language_capabilities().unwrap();
        assert!(languages.select("no", "fr").is_ok());
        assert!(languages.select("yue", "fr").is_err());
        route.asr.provider = "custom-openai".into();
        route.asr.transport = "openai".into();
        assert_eq!(
            route.language_capabilities().unwrap(),
            LanguageCapabilities::default()
        );
    }

    #[test]
    fn every_declared_model_language_uses_the_shared_catalogue() {
        for model in xrtranslate_assets::MODEL_ASSET_CATALOG {
            for code in model.languages {
                assert!(
                    xrtranslate_engine::language::SupportedLanguage::parse(code).is_some(),
                    "{}: {code}",
                    model.label
                );
            }
        }
    }
}
