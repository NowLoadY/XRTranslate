use crate::{
    AsyncHttpClient, InferenceError, OpenAiCompatibleClient, openai::non_streaming_chat_payload,
};

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
        })
    }

    pub fn provider(&self) -> TranslationProvider {
        self.provider
    }
}

impl<C: AsyncHttpClient> TranslationAdapter<C> {
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

        let completion = if streaming {
            // These older Qwen-MT families stream complete snapshots, whereas
            // Flash/Lite, llama.cpp and OpenAI stream appended deltas.
            let cumulative = self.provider == TranslationProvider::Qwen
                && ["qwen-mt-plus", "qwen-mt-turbo"]
                    .iter()
                    .any(|family| self.model.starts_with(family));
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
