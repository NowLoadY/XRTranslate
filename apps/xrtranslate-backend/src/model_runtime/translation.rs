//! Translation registration depends on wire format, never service identity.
use xrtranslate_assets::{ModelCapability, manifests_for_capability, remote::RemoteConnector};
use xrtranslate_config::NativeProviderConfig;
use xrtranslate_inference::{InferenceError, ReqwestClient, TranslationAdapter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationProfile {
    LocalCatalog,
    OpenAiCompatible,
    ChatGptAccount,
    OpenAiResponses,
}

impl TranslationProfile {
    pub(super) fn registered(config: &NativeProviderConfig) -> Option<Self> {
        if config.uses_local_runtime() {
            return manifests_for_capability(ModelCapability::Translation)
                .any(|manifest| manifest.provider == config.provider)
                .then_some(Self::LocalCatalog);
        }
        match config.connector {
            Some(RemoteConnector::OpenaiChat | RemoteConnector::DashscopeTranslation) => {
                Some(Self::OpenAiCompatible)
            }
            Some(RemoteConnector::OpenaiResponses) => Some(Self::OpenAiResponses),
            Some(RemoteConnector::ChatgptResponses) => Some(Self::ChatGptAccount),
            _ => None,
        }
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        config: &NativeProviderConfig,
        accounts: &xrtranslate_connections::AccountStore,
    ) -> Result<TranslationAdapter<ReqwestClient>, InferenceError> {
        let adapter = match self {
            Self::OpenAiResponses => TranslationAdapter::with_openai_responses(
                &config.url,
                &config.model,
                config.api_key.as_deref().unwrap_or_default(),
            ),
            Self::ChatGptAccount => TranslationAdapter::with_chatgpt_account(
                accounts.clone(),
                &config.url,
                &config.model,
            ),
            Self::LocalCatalog => Err(InferenceError::InvalidConfiguration {
                field: "translation.model_asset.runtime",
                message: "local translation adapters must be constructed from the model card"
                    .into(),
            }),
            Self::OpenAiCompatible => TranslationAdapter::with_bearer_token(
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
            ),
        }?;
        let adapter = match &config.agent_endpoint {
            Some((xrtranslate_assets::remote::AgentApi::DashscopeResponses, url)) => {
                adapter.with_dashscope_agent(url, config.api_key.as_deref().unwrap_or_default())?
            }
            _ => adapter,
        };
        Ok(adapter.with_remote_config(config.connector, &config.capabilities))
    }
}
