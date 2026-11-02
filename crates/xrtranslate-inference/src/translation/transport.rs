use crate::{
    AsyncHttpClient, ChatCompletion, InferenceError, OpenAiCompatibleClient,
    responses::ChatGptClient,
};
use serde_json::Value;

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
