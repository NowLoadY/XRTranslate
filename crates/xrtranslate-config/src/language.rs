use crate::NativeModelRouteConfig;
use xrtranslate_assets::{ModelCapability, language::provider_model};
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
                    if let Some(card) = xrtranslate_assets::remote::remote_asr_model(
                        &provider.provider,
                        &provider.transport,
                        &provider.model,
                    ) {
                        let codes = card
                            .languages
                            .iter()
                            .copied()
                            .chain(card.language_aliases.iter().map(|(alias, _)| *alias))
                            .collect::<Vec<_>>();
                        return Ok(Some(LanguageSet::from_codes(&codes, true)));
                    }
                    None
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
