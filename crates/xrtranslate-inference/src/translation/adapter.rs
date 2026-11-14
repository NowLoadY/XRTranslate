use crate::{
    AsyncHttpClient, InferenceError, OpenAiCompatibleClient, openai::non_streaming_chat_payload,
};
use xrtranslate_assets::remote::{RemoteCapabilities, RemoteConnector, RemoteStreaming};
use xrtranslate_prompt::{PromptExecutionTrace, PromptMessage};

use super::{
    TranslationOptions, TranslationPromptProfile, TranslationResult,
    history::{HistoryBudget, history_tokens},
    profile::{registered, translation_output_rejection},
    transport::{ChatTransport, RequestFormat},
};

/// Reusable MT adapter for Hy-MT2 GGUF, Qwen-MT, and remote OpenAI-compatible services.
///
/// This type owns endpoint transport and authentication. Prompt construction,
/// sampling parameters, and output cleanup are selected by the registered
/// translation profile.
#[derive(Debug, Clone)]
pub struct TranslationAdapter<C> {
    chat: ChatTransport<C>,
    model: String,
    profile: TranslationPromptProfile,
    streaming: RemoteStreaming,
    request_format: RequestFormat,
    reasoning_effort: Option<String>,
    supports_json_output: bool,
    history: HistoryBudget,
}

impl<C> TranslationAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        profile: TranslationPromptProfile,
    ) -> Result<Self, InferenceError> {
        let model = validated_model(model)?;
        Ok(Self {
            chat: ChatTransport::Completions(OpenAiCompatibleClient::new(http, endpoint)?),
            model,
            profile,
            streaming: RemoteStreaming::Delta,
            request_format: RequestFormat::Messages,
            reasoning_effort: None,
            supports_json_output: false,
            history: HistoryBudget::default(),
        })
    }

    /// Creates a translation adapter for an OpenAI-compatible endpoint that
    /// requires an `Authorization: Bearer …` header (for example Groq).
    pub fn with_bearer_token(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        profile: TranslationPromptProfile,
        token: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let model = validated_model(model)?;
        Ok(Self {
            chat: ChatTransport::Completions(OpenAiCompatibleClient::with_bearer_token(
                http, endpoint, token,
            )?),
            model,
            profile,
            streaming: RemoteStreaming::Delta,
            request_format: RequestFormat::Messages,
            reasoning_effort: None,
            supports_json_output: false,
            history: HistoryBudget::default(),
        })
    }

    pub fn with_remote_config(
        mut self,
        connector: Option<RemoteConnector>,
        capabilities: &RemoteCapabilities,
    ) -> Self {
        self.streaming = capabilities.streaming;
        self.request_format = RequestFormat::from_remote(connector, capabilities);
        self.reasoning_effort
            .clone_from(&capabilities.reasoning_effort);
        self.supports_json_output = capabilities.supports_json_output;
        self
    }

    pub fn with_chatgpt_account(
        store: xrtranslate_connections::AccountStore,
        endpoint: &str,
        model: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        Ok(Self {
            chat: ChatTransport::Responses(crate::responses::ChatGptClient::new(store, endpoint)?),
            model: validated_model(model)?,
            profile: TranslationPromptProfile::ChatMessages,
            streaming: RemoteStreaming::Delta,
            request_format: RequestFormat::Messages,
            reasoning_effort: None,
            supports_json_output: false,
            history: HistoryBudget::default(),
        })
    }

    fn apply_request_options(&self, payload: &mut serde_json::Value) {
        if let Some(effort) = &self.reasoning_effort {
            payload["reasoning_effort"] = serde_json::Value::from(effort.as_str());
        }
    }

    pub fn prompt_profile(&self) -> TranslationPromptProfile {
        self.profile
    }
}

impl<C: AsyncHttpClient> TranslationAdapter<C> {
    /// One stateless request with caller-owned context. It does not render a
    /// translation graph, retrieve references, or read/write conversation history.
    pub async fn ask(
        &self,
        messages: &[PromptMessage],
        context_window_tokens: u32,
        max_tokens: u32,
        response_schema: Option<&serde_json::Value>,
    ) -> Result<crate::ChatCompletion, InferenceError> {
        if messages.is_empty()
            || messages
                .iter()
                .any(|message| message.content.trim().is_empty())
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "ask.messages",
                message: "requires nonempty messages".into(),
            });
        }
        let profile = registered(self.profile);
        let mut payload = non_streaming_chat_payload(
            &self.model,
            self.request_format
                .instruction_messages(messages, self.profile),
            profile.temperature(),
            max_tokens,
        );
        let mut options = TranslationOptions::new("", "");
        options.context_window_tokens = context_window_tokens;
        options.max_tokens = max_tokens;
        profile.apply_sampling(&mut payload, &options);
        self.apply_request_options(&mut payload);
        if let Some(schema) = response_schema.filter(|_| self.supports_json_output) {
            payload["response_format"] = serde_json::json!({"type":"json_schema","json_schema":{"name":"response","strict":false,"schema":schema}});
        }
        let started = std::time::Instant::now();
        let mut record =
            crate::diagnostics::RequestRecord::start("Instruction", &payload, Vec::new());
        let result = async {
            let mut completion = self.chat.complete(payload).await.map_err(|error| {
                tracing::warn!(operation = "ask", model = %self.model, elapsed_ms = started.elapsed().as_millis(), error = %error, "Model inference failed");
                error
            })?;
            tracing::info!(operation = "ask", model = %self.model, messages = messages.len(), elapsed_ms = started.elapsed().as_millis(), usage = ?completion.usage, "Model inference completed");
            record.capture_output(&completion.text);
            completion.text = crate::openai::remove_completion_markers(&completion.text);
            if completion.text.is_empty() {
                return Err(InferenceError::EmptyOutput { operation: "ask" });
            }
            Ok(completion)
        }
        .await;
        record.finish(result.as_ref().map(|completion| completion.text.as_str()));
        result
    }

    pub async fn translate(
        &self,
        source_text: &str,
        options: TranslationOptions,
    ) -> Result<TranslationResult, InferenceError> {
        self.translate_request(
            source_text,
            options,
            false,
            |_| std::future::ready(()),
            |_| {},
        )
        .await
    }

    /// Publishes cleaned cumulative text while keeping final validation shared
    /// with ordinary requests. Text updates are awaited for bounded consumers;
    /// the rendered trace is published once before the provider request.
    pub async fn translate_streaming<F, Fut, T>(
        &self,
        source_text: &str,
        options: TranslationOptions,
        on_update: F,
        on_trace: T,
    ) -> Result<TranslationResult, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
        T: Fn(&PromptExecutionTrace) + Send + Sync,
    {
        self.translate_request(source_text, options, true, on_update, on_trace)
            .await
    }

    async fn translate_request<F, Fut, T>(
        &self,
        source_text: &str,
        mut options: TranslationOptions,
        streaming: bool,
        mut on_update: F,
        on_trace: T,
    ) -> Result<TranslationResult, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
        T: Fn(&PromptExecutionTrace) + Send + Sync,
    {
        let profile = registered(self.profile);
        let requested_history = history_tokens(&options.prompt_context);
        let history_limit = self.history.apply(&mut options.prompt_context);
        let prompt = profile.build_prompt(source_text, &options)?;
        on_trace(&prompt.trace);
        let mut payload = non_streaming_chat_payload(
            &self.model,
            prompt.messages_json(),
            profile.temperature(),
            options.max_tokens,
        );
        profile.apply_sampling(&mut payload, &options);
        self.apply_request_options(&mut payload);
        self.request_format.apply(
            &mut payload,
            &prompt.messages,
            source_text,
            &options.source_language,
            &options.target_language,
        );

        if streaming && self.streaming != RemoteStreaming::None {
            payload["stream"] = serde_json::Value::Bool(true);
        }
        let mut record = crate::diagnostics::RequestRecord::start(
            "Translation",
            &payload,
            prompt.trace.prompt_parts(&options.prompt_graph),
        );
        let result = async {
            let mut stream_rejection = None;
            let completion = if streaming && self.streaming != RemoteStreaming::None {
                let cumulative = self.streaming == RemoteStreaming::Cumulative;
                let mut previous = String::new();
                self.chat
                    .stream(payload, cumulative, |text| {
                        let text = profile.clean_output(&text);
                        stream_rejection = translation_output_rejection(
                            source_text,
                            &text,
                            &prompt.messages,
                            &prompt.context,
                        );
                        if stream_rejection.is_some() {
                            record.capture_output(&text);
                        }
                        let callback =
                            if stream_rejection.is_none() && !text.is_empty() && text != previous {
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
                    .await
            } else {
                self.chat.complete(payload).await
            }
            .map_err(|error| {
                // An expanding response can hit the output limit before completion.
                if error.is_output_limit()
                    && let Some(reason) = stream_rejection
                {
                    self.history.reject(&prompt.context);
                    InferenceError::RejectedOutput {
                        operation: "translation",
                        reason,
                    }
                } else {
                    error
                }
            })?;
            record.capture_output(&completion.text);
            let text = profile.clean_output(&completion.text);
            if text.is_empty() {
                return Err(InferenceError::EmptyOutput {
                    operation: "translation",
                });
            }
            if let Some(reason) =
                translation_output_rejection(source_text, &text, &prompt.messages, &prompt.context)
            {
                self.history.reject(&prompt.context);
                return Err(InferenceError::RejectedOutput {
                    operation: "translation",
                    reason,
                });
            }
            self.history.recover(history_limit, requested_history);
            Ok(TranslationResult {
                text,
                prompt_trace: prompt.trace,
            })
        }
        .await;
        record.finish(result.as_ref().map(|translation| translation.text.as_str()));
        result
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
    use super::*;
    use crate::{HttpRequest, HttpResponse, TransportError};
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingHttp(
        Arc<Mutex<Vec<Value>>>,
        Arc<Mutex<VecDeque<Result<HttpResponse, TransportError>>>>,
    );

    impl AsyncHttpClient for RecordingHttp {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.0.lock().unwrap().push(request.body);
            if let Some(response) = self.1.lock().unwrap().pop_front() {
                return response;
            }
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: json!({"choices":[{"message":{"content":"answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2048,"prompt_tokens_details":{"cached_tokens":1024}}})
                    .to_string(),
            })
        }
    }

    #[tokio::test]
    async fn diagnostics_keeps_the_submitted_prompt_and_rejected_output() {
        let http = RecordingHttp::default();
        let output = "仅翻译当前输入，不要重复提示词。".repeat(15);
        http.1.lock().unwrap().push_back(Ok(HttpResponse {
            status: 200,
            retry_after: None,
            body: json!({"choices":[{"message":{"content":output},"finish_reason":"stop"}]})
                .to_string(),
        }));
        let adapter = TranslationAdapter::new(
            http.clone(),
            "http://localhost/v1/chat/completions",
            "diagnostics-adapter-fixture",
            TranslationPromptProfile::Contextual,
        )
        .unwrap();
        let mut options = TranslationOptions::new("English", "Chinese");
        let style = "Keep every detail of this style instruction. Use natural language.";
        assert!(options.prompt_graph.set_style_text(style));
        assert!(
            adapter
                .translate("glimmer", options)
                .await
                .unwrap_err()
                .is_rejected_output()
        );
        let records = crate::diagnostics::requests();
        let record = records
            .iter()
            .find(|record| record.model == "diagnostics-adapter-fixture")
            .unwrap();
        assert_eq!(record.request, http.0.lock().unwrap()[0]);
        assert!(record.prompt_text().contains(style));
        assert!(
            record
                .parts
                .iter()
                .any(|part| part.kind == xrtranslate_prompt::diagnostics::PromptPartKind::Style)
        );
        assert!(record.error.is_some());
        assert_eq!(record.output, output);
    }

    #[tokio::test]
    async fn rejected_output_reduces_only_history_and_preserves_full_style_and_layout() {
        use xrtranslate_prompt::{PromptTurn, SurroundingSource};
        let http = RecordingHttp::default();
        let adapter = TranslationAdapter::new(
            http.clone(),
            "http://localhost/v1/chat/completions",
            "fixture",
            TranslationPromptProfile::Contextual,
        )
        .unwrap();
        let mut options = TranslationOptions::new("English", "Chinese");
        options.context_window_tokens = 8192;
        let style = "Use warm and natural wording. Preserve the original meaning and tone.";
        assert!(options.prompt_graph.set_style_text(style));
        options.prompt_context.terminology_rows = vec!["glimmer,微光".into()];
        options.prompt_context.surrounding_source = Some(SurroundingSource {
            speaker_id: String::new(),
            source_language: "en".into(),
            before: "It was dark outside.".into(),
            after: "The sun was rising.".into(),
        });
        options.prompt_context.recent_turns = (0..4)
            .map(|index| PromptTurn {
                turn_id: Some(index.to_string()),
                speaker_id: String::new(),
                source_language: "en".into(),
                target_language: "zh".into(),
                source_text: format!("Earlier {index}: {}", "conversation ".repeat(4)),
                translated_text: "之前的对话".repeat(4),
            })
            .collect();
        let original = options.clone();
        http.1.lock().unwrap().push_back(Ok(HttpResponse {
            status: 200, retry_after: None,
            body: json!({"choices":[{"message":{"content":"仅翻译当前输入并且不要重复提示词。".repeat(10)},"finish_reason":"stop"}]}).to_string(),
        }));
        let error = adapter
            .translate("glimmer", options.clone())
            .await
            .unwrap_err();
        assert!(error.is_rejected_output());
        // A transport failure must not reduce history a second time.
        http.1
            .lock()
            .unwrap()
            .push_back(Err(TransportError::new("timeout", "fixture")));
        assert!(
            adapter
                .translate("glimmer", options.clone())
                .await
                .unwrap_err()
                .is_retryable()
        );
        adapter
            .clone()
            .translate("glimmer", options.clone())
            .await
            .unwrap();
        let mut expected = options.clone();
        expected.prompt_context.recent_turns.drain(..2);
        let expected_messages = super::super::build_translation_messages(
            TranslationPromptProfile::Contextual,
            "glimmer",
            &expected,
        )
        .unwrap();
        let requests = http.0.lock().unwrap();
        assert_eq!(requests[1]["messages"], expected_messages);
        assert_eq!(requests[2]["messages"], expected_messages);
        assert!(requests[2]["messages"].to_string().contains(style));
        assert_eq!(options, original);
    }

    #[tokio::test]
    async fn expanding_stream_reduces_history_even_when_generation_hits_the_output_limit() {
        use xrtranslate_prompt::PromptTurn;
        for finish_reason in ["stop", "length"] {
            let http = RecordingHttp::default();
            let adapter = TranslationAdapter::new(
                http.clone(),
                "http://localhost/v1/chat/completions",
                "fixture",
                TranslationPromptProfile::Contextual,
            )
            .unwrap();
            let mut options = TranslationOptions::new("English", "Chinese");
            let style = "Keep the tone gentle.";
            options.prompt_graph.set_style_text(style);
            options.prompt_context.recent_turns.push(PromptTurn {
                turn_id: None,
                speaker_id: String::new(),
                source_language: "en".into(),
                target_language: "zh".into(),
                source_text: "previous exchange".into(),
                translated_text: "之前的对话".into(),
            });
            let delta = json!({"choices":[{"delta":{"content":"只翻译当前输入并保持原意。".repeat(12)},"finish_reason":null}]});
            let end = json!({"choices":[{"delta":{},"finish_reason":finish_reason}]});
            http.1.lock().unwrap().push_back(Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: format!("data: {delta}\n\ndata: {end}\n\ndata: [DONE]\n\n"),
            }));
            let mut previews = Vec::new();
            let error = adapter
                .translate_streaming(
                    "hi",
                    options.clone(),
                    |text| {
                        previews.push(text);
                        std::future::ready(())
                    },
                    |_| {},
                )
                .await
                .unwrap_err();
            assert!(error.is_rejected_output(), "{error}");
            assert!(previews.is_empty());
            adapter.translate("hi", options.clone()).await.unwrap();
            let mut expected = options;
            expected.prompt_context.recent_turns.clear();
            let messages = super::super::build_translation_messages(
                TranslationPromptProfile::Contextual,
                "hi",
                &expected,
            )
            .unwrap();
            assert_eq!(http.0.lock().unwrap()[1]["messages"], messages);
            assert!(messages.to_string().contains(style));
        }
    }

    #[tokio::test]
    async fn ask_preserves_caller_history_and_adapts_single_instruction_models() {
        let http = RecordingHttp::default();
        let adapter = TranslationAdapter::new(
            http.clone(),
            "http://localhost/v1/chat/completions",
            "fixture",
            TranslationPromptProfile::ChatMessages,
        )
        .unwrap();
        let mut messages: Vec<PromptMessage> = serde_json::from_value(json!([
            {"role":"system","content":"rules and tools"},
            {"role":"user","content":"goal"}
        ]))
        .unwrap();
        let completion = adapter.ask(&messages, 8192, 512, None).await.unwrap();
        assert_eq!(completion.text, "answer");
        assert_eq!(completion.usage.unwrap().cached_input_tokens, Some(1024));
        messages.extend(
            serde_json::from_value::<Vec<PromptMessage>>(json!([
                {"role":"assistant","content":"action"},
                {"role":"user","content":"result"}
            ]))
            .unwrap(),
        );
        adapter.ask(&messages, 8192, 512, None).await.unwrap();
        // A new caller supplies its own context, even on the same adapter.
        adapter.ask(&messages[..2], 8192, 512, None).await.unwrap();
        {
            let requests = http.0.lock().unwrap();
            assert_eq!(requests[0]["messages"], json!(&messages[..2]));
            assert_eq!(requests[1]["messages"], json!(messages));
            assert_eq!(requests[2]["messages"], requests[0]["messages"]);
            for request in requests.iter() {
                assert!(request.get("response_format").is_none());
                assert!(request.get("previous_response_id").is_none());
                assert!(request.get("conversation").is_none());
            }
        }
        adapter
            .clone()
            .with_remote_config(
                None,
                &RemoteCapabilities {
                    supports_json_output: true,
                    ..Default::default()
                },
            )
            .ask(&messages, 8192, 512, Some(&json!({"type":"object"})))
            .await
            .unwrap();
        assert_eq!(
            http.0.lock().unwrap().last().unwrap()["response_format"],
            json!({"type":"json_schema","json_schema":{"name":"response","strict":false,"schema":{"type":"object"}}})
        );
        let single = TranslationAdapter::new(
            http.clone(),
            "http://localhost/v1/chat/completions",
            "fixture",
            TranslationPromptProfile::Contextual,
        )
        .unwrap();
        single
            .ask(&messages, 8192, 512, Some(&json!({"type":"object"})))
            .await
            .unwrap();
        let requests = http.0.lock().unwrap();
        assert!(requests.last().unwrap().get("response_format").is_none());
        let wire = &requests.last().unwrap()["messages"];
        assert_eq!(wire.as_array().unwrap().len(), 1);
        assert_eq!(wire[0]["role"], "user");
        let (_, transcript) = wire[0]["content"]
            .as_str()
            .unwrap()
            .split_once('\n')
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<PromptMessage>>(transcript).unwrap(),
            messages
        );
    }
}
