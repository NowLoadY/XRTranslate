//! Translation provider profiles and adapter construction.

use xrtranslate_assets::ModelAssetId;
use xrtranslate_inference::{
    InferenceError, ReqwestClient, TranslationAdapter, TranslationProvider,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationProfile {
    HunyuanLocal,
    OpenAiCompatible,
    QwenRemote,
}

impl TranslationProfile {
    pub(super) fn registered(provider: &str, transport: &str) -> Option<Self> {
        if provider == "qwen" || provider == "qwen-intl" {
            return Some(Self::QwenRemote);
        }
        if transport == "openai" {
            return Some(Self::OpenAiCompatible);
        }
        match provider {
            "hunyuan" => Some(Self::HunyuanLocal),
            _ => None,
        }
    }

    pub(super) const fn default_asset(self) -> ModelAssetId {
        match self {
            Self::HunyuanLocal => ModelAssetId::HunyuanMtGguf,
            Self::OpenAiCompatible | Self::QwenRemote => ModelAssetId::HunyuanMtGguf,
        }
    }

    pub(super) fn model_alias<'a>(self, configured: &'a str) -> &'a str {
        match self {
            Self::HunyuanLocal => "hy-mt2",
            Self::OpenAiCompatible | Self::QwenRemote => configured,
        }
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<TranslationAdapter<ReqwestClient>, InferenceError> {
        match self {
            Self::HunyuanLocal => {
                TranslationAdapter::new(http, endpoint, model, TranslationProvider::Hunyuan)
            }
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
                    TranslationAdapter::new(
                        http,
                        endpoint,
                        model,
                        TranslationProvider::Qwen,
                    )
                }
            }
        }
    }
}
