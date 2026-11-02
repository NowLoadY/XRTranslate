use crate::{
    AsyncHttpClient, ChatCompletion, InferenceError, OpenAiCompatibleClient,
    responses::ChatGptClient,
};
use serde_json::{Value, json};
use xrtranslate_assets::remote::{RemoteCapabilities, RemoteConnector};
use xrtranslate_engine::language::SupportedLanguage;
use xrtranslate_prompt::PromptMessage;

/// Encodes the rendered graph without authoring or reordering its content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RequestFormat {
    Messages,
    SingleUser,
    TranslationOptions,
}

impl RequestFormat {
    pub fn from_remote(
        connector: Option<RemoteConnector>,
        capabilities: &RemoteCapabilities,
    ) -> Self {
        if connector == Some(RemoteConnector::DashscopeTranslation) {
            Self::TranslationOptions
        } else if capabilities.single_user_message {
            Self::SingleUser
        } else {
            Self::Messages
        }
    }

    pub fn apply(
        self,
        payload: &mut Value,
        messages: &[PromptMessage],
        source_text: &str,
        source_language: &str,
        target_language: &str,
    ) {
        if self == Self::Messages {
            return;
        }
        let content = messages
            .iter()
            .map(|message| message.content.trim())
            .filter(|content| !content.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if self == Self::TranslationOptions {
            payload["messages"] = json!([{"role": "user", "content": source_text}]);
            payload["translation_options"] = json!({
                "source_lang": SupportedLanguage::parse(source_language)
                    .map_or(source_language, |language| language.name()),
                "target_lang": SupportedLanguage::parse(target_language)
                    .map_or(target_language, |language| language.name()),
                "domains": content,
            });
        } else {
            payload["messages"] = json!([{"role": "user", "content": content}]);
        }
    }
}

/// The prompt/translation layer is shared; only wire delivery differs.
#[derive(Debug, Clone)]
pub(super) enum ChatTransport<C> {
    Completions(OpenAiCompatibleClient<C>),
    Responses(ChatGptClient),
}

impl<C: AsyncHttpClient> ChatTransport<C> {
    pub async fn complete(&self, payload: Value) -> Result<ChatCompletion, InferenceError> {
        match self {
            Self::Completions(client) => client.chat_completion(payload).await,
            Self::Responses(client) => client.complete(payload, |_| std::future::ready(())).await,
        }
    }

    pub async fn stream<F, Fut>(
        &self,
        payload: Value,
        cumulative: bool,
        on_update: F,
    ) -> Result<ChatCompletion, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        match self {
            Self::Completions(client) => {
                client
                    .chat_completion_streaming(payload, cumulative, on_update)
                    .await
            }
            Self::Responses(client) => client.complete(payload, on_update).await,
        }
    }
}
