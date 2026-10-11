use crate::{
    AsyncHttpClient, InferenceError, OpenAiCompatibleClient, openai::non_streaming_chat_payload,
};
use xrtranslate_assets::remote::{RemoteCapabilities, RemoteStreaming};

use super::{
    TranslationOptions, TranslationProvider, TranslationResult,
    profile::{registered, translation_output_rejection},
};

/// Reusable MT adapter for Hy-MT2 GGUF, Qwen-MT, and remote OpenAI-compatible services.
///
/// This type owns endpoint transport and authentication. Prompt construction,
/// sampling parameters, and output cleanup are selected by the registered
/// translation profile.
#[derive(Debug, Clone)]
pub struct TranslationAdapter<C> {
    chat: OpenAiCompatibleClient<C>,
    model: String,
    provider: TranslationProvider,
    streaming: RemoteStreaming,
    single_user_message: bool,
}

impl<C> TranslationAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        provider: TranslationProvider,
    ) -> Result<Self, InferenceError> {
        let model = validated_model(model)?;
        Ok(Self {
            chat: OpenAiCompatibleClient::new(http, endpoint)?,
            model,
            provider,
            streaming: RemoteStreaming::Delta,
            single_user_message: false,
        })
    }

    /// Creates a translation adapter for an OpenAI-compatible endpoint that
    /// requires an `Authorization: Bearer …` header (for example Groq).
    pub fn with_bearer_token(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        provider: TranslationProvider,
        token: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let model = validated_model(model)?;
        Ok(Self {
            chat: OpenAiCompatibleClient::with_bearer_token(http, endpoint, token)?,
            model,
            provider,
            streaming: RemoteStreaming::Delta,
            single_user_message: false,
        })
    }

    pub fn with_remote_capabilities(mut self, capabilities: &RemoteCapabilities) -> Self {
        self.streaming = capabilities.streaming;
        self.single_user_message = capabilities.single_user_message;
        self
    }

    pub fn provider(&self) -> TranslationProvider {
        self.provider
    }
}

impl<C: AsyncHttpClient> TranslationAdapter<C> {
    /// Ask is one user instruction. It does not render a translation graph,
    /// retrieve reference material, or read/write conversation history.
    pub async fn ask(
        &self,
        prompt: &str,
        context_window_tokens: u32,
        max_tokens: u32,
    ) -> Result<String, InferenceError> {
        if prompt.trim().is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "ask.prompt",
                message: "must not be empty".into(),
            });
        }
        let profile = registered(self.provider);
        let mut payload = non_streaming_chat_payload(
            &self.model,
            serde_json::json!([{"role": "user", "content": prompt}]),
            profile.temperature(),
            max_tokens,
        );
        let mut options = TranslationOptions::new("", "");
        options.context_window_tokens = context_window_tokens;
        options.max_tokens = max_tokens;
        profile.apply_sampling(&mut payload, &options);
        let completion = self.chat.chat_completion(payload).await?;
        let text = crate::openai::remove_completion_markers(&completion.text);
        if text.is_empty() {
            return Err(InferenceError::EmptyOutput { operation: "ask" });
        }
        Ok(text)
    }

    pub async fn translate(
        &self,
        source_text: &str,
        options: TranslationOptions,
    ) -> Result<TranslationResult, InferenceError> {
        self.translate_request(source_text, options, false, |_| std::future::ready(()))
            .await
    }

    /// Publishes cleaned cumulative text while keeping final validation shared
    /// with ordinary requests. The callback is awaited for bounded consumers.
    pub async fn translate_streaming<F, Fut>(
        &self,
        source_text: &str,
        options: TranslationOptions,
        on_update: F,
    ) -> Result<TranslationResult, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        self.translate_request(source_text, options, true, on_update)
            .await
    }

    async fn translate_request<F, Fut>(
        &self,
        source_text: &str,
        options: TranslationOptions,
        streaming: bool,
        mut on_update: F,
    ) -> Result<TranslationResult, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let profile = registered(self.provider);
        let prompt = profile.build_prompt(source_text, &options)?;
        let mut payload = non_streaming_chat_payload(
            &self.model,
            prompt.messages_json(),
            profile.temperature(),
            options.max_tokens,
        );
        profile.apply_sampling(&mut payload, &options);
        if self.single_user_message {
            let ordered = [
                xrtranslate_prompt::PromptMessageRole::System,
                xrtranslate_prompt::PromptMessageRole::User,
            ];
            let content = ordered
                .iter()
                .flat_map(|role| {
                    prompt
                        .messages
                        .iter()
                        .filter(move |message| message.role == *role)
                })
                .filter(|message| !message.content.trim().is_empty())
                .map(|message| message.content.trim())
                .collect::<Vec<_>>()
                .join("\n\n");
            payload["messages"] = serde_json::json!([{"role": "user", "content": content}]);
        }

        let completion = if streaming && self.streaming != RemoteStreaming::None {
            let cumulative = self.streaming == RemoteStreaming::Cumulative;
            let mut previous = String::new();
            self.chat
                .chat_completion_streaming(payload, cumulative, |text| {
                    let text = profile.clean_output(&text);
                    let safe = translation_output_rejection(
                        source_text,
                        &text,
                        &prompt.messages,
                        &options.prompt_context,
                    )
                    .is_none();
                    let callback = if safe && !text.is_empty() && text != previous {
                        previous.clone_from(&text);
                        Some(on_update(text))
                    } else {
                        None
                    };
                    async move {
                        if let Some(callback) = callback {
                            callback.await;
                        }
                    }
                })
                .await?
        } else {
            self.chat.chat_completion(payload).await?
        };
        let text = profile.clean_output(&completion.text);
        if text.is_empty() {
            return Err(InferenceError::EmptyOutput {
                operation: "translation",
            });
        }
        if let Some(reason) = translation_output_rejection(
            source_text,
            &text,
            &prompt.messages,
            &options.prompt_context,
        ) {
            return Err(InferenceError::RejectedOutput {
                operation: "translation",
                reason,
            });
        }
        Ok(TranslationResult {
            text,
            prompt_trace: prompt.trace,
        })
    }
}

fn validated_model(model: impl Into<String>) -> Result<String, InferenceError> {
    let model = model.into().trim().to_owned();
    if model.is_empty() {
        return Err(InferenceError::InvalidConfiguration {
            field: "model",
            message: "must not be empty".into(),
        });
    }
    Ok(model)
}
