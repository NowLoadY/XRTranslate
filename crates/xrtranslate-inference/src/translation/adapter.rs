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
        let profile = registered(self.provider);
        let prompt = profile.build_prompt(source_text, &options)?;
        let mut payload = non_streaming_chat_payload(
            &self.model,
            prompt.messages_json(),
            profile.temperature(),
            options.max_tokens,
        );
        profile.apply_sampling(&mut payload, &options);

        let completion = self.chat.chat_completion(payload).await?;
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{HttpRequest, HttpResponse, TransportError};

    #[derive(Default)]
    struct RecordingHttpClient {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<Vec<Result<HttpResponse, TransportError>>>,
    }

    impl RecordingHttpClient {
        fn respond_with(&self, response: HttpResponse) {
            self.responses.lock().unwrap().push(Ok(response));
        }
    }

    impl AsyncHttpClient for RecordingHttpClient {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.responses.lock().unwrap().remove(0)
        }
    }

    #[tokio::test]
    async fn hunyuan_request_uses_direct_prompt_and_cleans_response() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"你好\n--- END CURRENT INPUT --- <|im_end|>"}}]}"#.into(),
        });
        let adapter = TranslationAdapter::new(
            http,
            "http://127.0.0.1:8002/v1/chat/completions",
            "hy-mt2",
            TranslationProvider::Hunyuan,
        )
        .unwrap();
        let mut options = TranslationOptions::new("English", "Chinese");
        options.prompt_context.terminology_rows = vec!["A previous sentence.".into()];
        options.context_window_tokens = 4_096;
        let result = adapter.translate("hello", options).await.unwrap();
        assert_eq!(result.text, "你好");
        assert_eq!(
            result
                .prompt_trace
                .node("hunyuan-current-input")
                .map(|node| node.output.as_str()),
            Some("hello")
        );
        assert!(result.prompt_trace.node("hunyuan-request").is_some());

        let http = adapter.chat.into_inner();
        let request = http.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.url, "http://127.0.0.1:8002/v1/chat/completions");
        assert_eq!(request.body["model"], "hy-mt2");
        assert_eq!(request.body["temperature"], 0.7);
        assert_eq!(request.body["top_p"], 0.6);
        assert_eq!(request.body["top_k"], 20);
        assert_eq!(request.body["repeat_penalty"], 1.05);
        assert_eq!(request.body["repeat_last_n"], 4_096);
        assert!(request.body["repeat_last_n"].as_i64().unwrap() >= 0);
        assert_eq!(request.body["min_p"], 0.0);
        assert_eq!(request.body["messages"].as_array().unwrap().len(), 1);
        let prompt = request.body["messages"][0]["content"].as_str().unwrap();
        assert!(prompt.contains("following English text into natural Chinese"));
        assert!(prompt.contains("--- BEGIN REFERENCE CONTEXT ---"));
        assert!(prompt.contains("A previous sentence."));
        assert!(prompt.ends_with("Current input:\nhello"));
        assert!(!prompt.contains("END CURRENT INPUT"));
    }

    #[tokio::test]
    async fn generic_adapter_sends_bearer_auth_outside_json_payload() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"bonjour"}}]}"#.into(),
        });
        let adapter = TranslationAdapter::with_bearer_token(
            http,
            "https://example.test/openai/v1/chat/completions",
            "remote-model",
            TranslationProvider::OpenAiCompatible,
            "test-token",
        )
        .unwrap();
        adapter
            .translate("hello", TranslationOptions::new("English", "French"))
            .await
            .unwrap();

        let http = adapter.chat.into_inner();
        let request = http.requests.lock().unwrap().pop().unwrap();
        assert!(
            request
                .headers
                .contains(&("authorization".into(), "Bearer test-token".into()))
        );
        assert_eq!(request.body["model"], "remote-model");
        assert!(request.body.get("authorization").is_none());
    }

    #[tokio::test]
    async fn rejects_text_copied_from_the_actual_rendered_prompt() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 200,
            body: serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "Translate only the current input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final translation."
                    }
                }]
            })
            .to_string(),
        });
        let adapter = TranslationAdapter::new(
            http,
            "http://127.0.0.1:8002/v1/chat/completions",
            "hy-mt2",
            TranslationProvider::Hunyuan,
        )
        .unwrap();
        let mut options = TranslationOptions::new("English", "Chinese");
        options.prompt_context.terminology_rows = vec!["VRChat,VRChat".into()];

        let error = adapter.translate("hello", options).await.unwrap_err();

        assert!(matches!(
            error,
            InferenceError::RejectedOutput {
                operation: "translation",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn qwen_translation_request_merges_system_into_single_user_message() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"你好世界"}}]}"#.into(),
        });
        let adapter = TranslationAdapter::with_bearer_token(
            http,
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            "qwen-mt-flash",
            TranslationProvider::Qwen,
            "test-dashscope-key",
        )
        .unwrap();
        let mut options = TranslationOptions::new("English", "Chinese");
        options.prompt_context.terminology_rows = vec!["world,世界".into()];
        let result = adapter.translate("hello world", options).await.unwrap();
        assert_eq!(result.text, "你好世界");

        let http = adapter.chat.into_inner();
        let request = http.requests.lock().unwrap().pop().unwrap();
        assert!(
            request
                .headers
                .contains(&("authorization".into(), "Bearer test-dashscope-key".into()))
        );
        assert_eq!(request.body["model"], "qwen-mt-flash");
        let messages = request.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_str().unwrap();
        assert!(content.contains("world,世界"));
        assert!(content.contains("hello world"));
    }
}
