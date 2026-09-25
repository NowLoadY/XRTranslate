//! Translation provider profiles and adapter construction.

use xrtranslate_assets::{ModelCapability, manifests_for_capability};
use xrtranslate_inference::{
    InferenceError, ReqwestClient, TranslationAdapter, TranslationProvider,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationProfile {
    LocalCatalog,
    OpenAiCompatible,
    QwenRemote,
}

impl TranslationProfile {
    pub(super) fn registered(provider: &str, transport: &str) -> Option<Self> {
        if transport == "local" {
            return manifests_for_capability(ModelCapability::Translation)
                .any(|manifest| manifest.provider == provider)
                .then_some(Self::LocalCatalog);
        }
        if provider == "qwen" || provider == "qwen-intl" {
            return Some(Self::QwenRemote);
        }
        if transport == "openai" {
            return Some(Self::OpenAiCompatible);
        }
        None
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<TranslationAdapter<ReqwestClient>, InferenceError> {
        match self {
            Self::LocalCatalog => Err(InferenceError::InvalidConfiguration {
                field: "translation.model_asset.runtime",
                message: "local translation adapters must be constructed from the model card"
                    .into(),
            }),
            Self::OpenAiCompatible => {
                if let Some(token) = api_key {
                    TranslationAdapter::with_bearer_token(
                        http,
                        endpoint,
                        model,
                        TranslationProvider::OpenAiCompatible,
                        token,
                    )
                } else {
                    TranslationAdapter::new(
                        http,
                        endpoint,
                        model,
                        TranslationProvider::OpenAiCompatible,
                    )
                }
            }
            Self::QwenRemote => {
                if let Some(token) = api_key {
                    TranslationAdapter::with_bearer_token(
                        http,
                        endpoint,
                        model,
                        TranslationProvider::Qwen,
                        token,
                    )
                } else {
                    TranslationAdapter::new(http, endpoint, model, TranslationProvider::Qwen)
                }
            }
        }
    }
}
