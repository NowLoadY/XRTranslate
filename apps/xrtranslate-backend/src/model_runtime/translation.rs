//! Translation registration depends on wire format, never service identity.
use xrtranslate_assets::{ModelCapability, manifests_for_capability, remote::RemoteConnector};
use xrtranslate_config::NativeProviderConfig;
use xrtranslate_inference::{InferenceError, ReqwestClient, TranslationAdapter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationProfile {
    LocalCatalog,
    OpenAiCompatible,
}

impl TranslationProfile {
    pub(super) fn registered(config: &NativeProviderConfig) -> Option<Self> {
        if config.uses_local_runtime() {
            return manifests_for_capability(ModelCapability::Translation)
                .any(|manifest| manifest.provider == config.provider)
                .then_some(Self::LocalCatalog);
        }
        (config.connector == Some(RemoteConnector::OpenaiChat)).then_some(Self::OpenAiCompatible)
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        config: &NativeProviderConfig,
    ) -> Result<TranslationAdapter<ReqwestClient>, InferenceError> {
        if self == Self::LocalCatalog {
            return Err(InferenceError::InvalidConfiguration {
                field: "translation.model_asset.runtime",
                message: "local translation adapters must be constructed from the model card"
                    .into(),
            });
        }
        let adapter = TranslationAdapter::with_bearer_token(
            http,
            &config.url,
            &config.model,
            config.translation_prompt_profile().map_err(|message| {
                InferenceError::InvalidConfiguration {
                    field: "translation.capabilities",
                    message,
                }
            })?,
            config.api_key.as_deref().unwrap_or_default(),
        )?;
        Ok(adapter.with_remote_capabilities(&config.capabilities))
    }
}
