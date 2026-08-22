use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{Duration, MissedTickBehavior, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
    },
};
use uuid::Uuid;

use crate::{AsrTranscript, AsrVocabularyBias, InferenceError, TransportError};

const PCM_FRAME_BYTES: usize = 3_200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TASK_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_CONTEXT_CHARS: usize = 400;
const MAX_VOCABULARY_ENTRIES: usize = 2_000;

/// Options supported by Qwen Audio's streaming recognition transport.
///
/// `context_bias` is recognition context, not an instruction prompt. Callers
/// must route instruction prompts only to providers that advertise instruction
/// support.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QwenAudioStreamingOptions {
    pub language: Option<String>,
    pub context_bias: Option<String>,
    pub vocabulary_bias: Vec<AsrVocabularyBias>,
}

/// Qwen Audio streaming ASR over DashScope's native WebSocket protocol.
#[derive(Clone, Debug)]
pub struct QwenAudioStreamingAdapter {
    endpoint: String,
    model: String,
    api_key: String,
}

impl QwenAudioStreamingAdapter {
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let endpoint = required_value("endpoint", endpoint.into())?;
        let request = endpoint.clone().into_client_request().map_err(|error| {
            InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: error.to_string(),
            }
        })?;
        if !matches!(request.uri().scheme_str(), Some("ws" | "wss")) {
            return Err(InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: "must use the ws or wss scheme".into(),
            });
        }
        let is_loopback = matches!(
            request.uri().host(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        );
        if request.uri().scheme_str() == Some("ws") && !is_loopback {
            return Err(InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: "non-loopback endpoints must use wss".into(),
            });
        }

        Ok(Self {
            endpoint,
            model: required_value("model", model.into())?,
            api_key: required_value("api_key", api_key.into())?,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Transcribes one complete PCM16, 16 kHz, mono turn.
    ///
    /// Audio is sent in 100 ms (3,200-byte) binary frames only after the
    /// provider acknowledges `task-started`. Final, non-heartbeat sentences are
    /// aggregated until `task-finished`.
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: QwenAudioStreamingOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        if pcm.len() % 2 != 0 {
            return Err(InferenceError::InvalidAudio {
                message: "PCM16 input length must be even".into(),
            });
        }

        let task_id = Uuid::new_v4().to_string();
        let (run_task, requested_language) = build_run_task(&task_id, &self.model, &options)?;
        let mut request = self
            .endpoint
            .clone()
            .into_client_request()
            .map_err(|error| InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: error.to_string(),
            })?;
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|error| {
                InferenceError::InvalidConfiguration {
                    field: "api_key",
                    message: error.to_string(),
                }
            })?;
        request.headers_mut().insert(AUTHORIZATION, authorization);
        request.headers_mut().insert(
            "user-agent",
            HeaderValue::from_static("XRTranslate/qwen-audio-streaming"),
        );

        let (mut socket, _) = timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| timeout_error(&self.endpoint, "connect"))?
            .map_err(|error| websocket_error(&self.endpoint, "connect", error))?;
        timeout(CONNECT_TIMEOUT, async {
            socket
                .send(Message::Text(run_task.to_string().into()))
                .await
                .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            wait_for_task_started(&mut socket, &self.endpoint, &task_id).await
        })
        .await
        .map_err(|_| timeout_error(&self.endpoint, "task-started"))??;

        let (mut writer, mut reader) = socket.split();
        let send_audio = async {
            let mut frame_interval = tokio::time::interval(Duration::from_millis(100));
            frame_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            for frame in pcm.chunks(PCM_FRAME_BYTES) {
                frame_interval.tick().await;
                writer
                    .send(Message::Binary(frame.to_vec().into()))
                    .await
                    .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            }
            writer
                .send(Message::Text(finish_task(&task_id).to_string().into()))
                .await
                .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            Result::<(), InferenceError>::Ok(())
        };
        let receive_results = collect_final_sentences(&mut reader, &self.endpoint, &task_id);
        let ((), text) = timeout(TASK_TIMEOUT, async {
            tokio::try_join!(send_audio, receive_results)
        })
        .await
        .map_err(|_| timeout_error(&self.endpoint, "recognition"))??;
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(InferenceError::EmptyOutput {
                operation: "Qwen Audio ASR",
            });
        }

        Ok(AsrTranscript {
            language: requested_language,
            text,
        })
    }
}

fn required_value(field: &'static str, value: String) -> Result<String, InferenceError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(InferenceError::InvalidConfiguration {
            field,
            message: "must not be empty".into(),
        });
    }
    Ok(value.to_owned())
}

fn build_run_task(
    task_id: &str,
    model: &str,
    options: &QwenAudioStreamingOptions,
) -> Result<(Value, Option<String>), InferenceError> {
    let (language_code, requested_language) = map_language(options.language.as_deref())?;
    let mut parameters = serde_json::Map::from_iter([
        ("format".into(), json!("pcm")),
        ("sample_rate".into(), json!(16_000)),
    ]);
    if let Some(code) = language_code {
        parameters.insert("language_hints".into(), json!([code]));
    }

    let vocabulary = validated_vocabulary(&options.vocabulary_bias)?;
    if !vocabulary.is_empty() {
        parameters.insert("vocabulary".into(), json!(vocabulary));
    }

    let context_bias = options
        .context_bias
        .as_deref()
        .map(str::trim)
        .filter(|context| !context.is_empty());
    if context_bias.is_some_and(|context| context.chars().count() > MAX_CONTEXT_CHARS) {
        return Err(InferenceError::InvalidConfiguration {
            field: "context_bias",
            message: format!("must contain at most {MAX_CONTEXT_CHARS} characters"),
        });
    }
    let input = context_bias.as_deref().map_or_else(
        || json!({}),
        |context| {
            json!({
                "context": [{
                    "role": "user",
                    "content": [{"type": "input_text", "text": context}]
                }]
            })
        },
    );

    Ok((
        json!({
            "header": {
                "action": "run-task",
                "task_id": task_id,
                "streaming": "duplex"
            },
            "payload": {
                "task_group": "audio",
                "task": "asr",
                "function": "recognition",
                "model": model,
                "parameters": parameters,
                "input": input
            }
        }),
        requested_language,
    ))
}

fn validated_vocabulary(
    vocabulary: &[AsrVocabularyBias],
) -> Result<BTreeMap<String, u8>, InferenceError> {
    let mut result = BTreeMap::new();
    let mut discarded_entries = 0usize;
    for item in vocabulary {
        let text = item.text.trim();
        if text.is_empty() {
            discarded_entries += 1;
            continue;
        }
        if !matches!(item.weight, 1..=5 | 50) {
            return Err(InferenceError::InvalidConfiguration {
                field: "vocabulary_bias.weight",
                message: "must be between 1 and 5, or exactly 50".into(),
            });
        }
        if text.is_ascii() {
            if text.split_ascii_whitespace().count() > 7 {
                discarded_entries += 1;
                continue;
            }
        } else if text.chars().count() > 15 {
            discarded_entries += 1;
            continue;
        }
        if !result.contains_key(text) && result.len() >= MAX_VOCABULARY_ENTRIES {
            discarded_entries += 1;
            continue;
        }
        result.insert(text.to_owned(), item.weight);
    }
    let mut super_hot_words = 0usize;
    result.retain(|_, weight| {
        if *weight != 50 {
            return true;
        }
        super_hot_words += 1;
        let keep = super_hot_words <= 50;
        discarded_entries += usize::from(!keep);
        keep
    });
    if discarded_entries > 0 {
        tracing::warn!(
            discarded_entries,
            "discarded ASR vocabulary entries outside the provider contract"
        );
    }
    Ok(result)
}

fn map_language(
    language: Option<&str>,
) -> Result<(Option<&'static str>, Option<String>), InferenceError> {
    let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None));
    };
    if language.eq_ignore_ascii_case("auto") {
        return Ok((None, None));
    }

    let normalized = language.to_ascii_lowercase().replace('_', "-");
    let primary = normalized.split('-').next().unwrap_or_default();
    let code = match normalized.as_str() {
        "chinese" | "traditional chinese" | "simplified chinese" => "zh",
        "english" => "en",
        "japanese" => "ja",
        "korean" => "ko",
        "vietnamese" => "vi",
        "thai" => "th",
        "indonesian" => "id",
        "malay" => "ms",
        "filipino" | "tagalog" => "tl",
        "hindi" => "hi",
        "arabic" => "ar",
        "french" => "fr",
        "german" => "de",
        "spanish" => "es",
        "portuguese" => "pt",
        "russian" => "ru",
        "italian" => "it",
        "dutch" => "nl",
        "swedish" => "sv",
        "danish" => "da",
        "finnish" => "fi",
        "norwegian" => "no",
        "greek" => "el",
        "polish" => "pl",
        "czech" => "cs",
        "hungarian" => "hu",
        "romanian" => "ro",
        "bulgarian" => "bg",
        "croatian" => "hr",
        "slovak" => "sk",
        _ => match primary {
            "zh" => "zh",
            "en" => "en",
            "ja" => "ja",
            "ko" => "ko",
            "vi" => "vi",
            "th" => "th",
            "id" => "id",
            "ms" => "ms",
            "tl" => "tl",
            "hi" => "hi",
            "ar" => "ar",
            "fr" => "fr",
            "de" => "de",
            "es" => "es",
            "pt" => "pt",
            "ru" => "ru",
            "it" => "it",
            "nl" => "nl",
            "sv" => "sv",
            "da" => "da",
            "fi" => "fi",
            "no" => "no",
            "el" => "el",
            "pl" => "pl",
            "cs" => "cs",
            "hu" => "hu",
            "ro" => "ro",
            "bg" => "bg",
            "hr" => "hr",
            "sk" => "sk",
            _ => {
                return Err(InferenceError::InvalidConfiguration {
                    field: "language",
                    message: format!("{language:?} is not supported by Qwen Audio streaming ASR"),
                });
            }
        },
    };
    Ok((Some(code), Some(language.to_owned())))
}

fn finish_task(task_id: &str) -> Value {
    json!({
        "header": {
            "action": "finish-task",
            "task_id": task_id,
            "streaming": "duplex"
        },
        "payload": {"input": {}}
    })
}

async fn wait_for_task_started<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    endpoint: &str,
    task_id: &str,
) -> Result<(), InferenceError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(message) = socket.next().await {
        match message.map_err(|error| websocket_error(endpoint, "receive", error))? {
            Message::Text(text) => {
                let event = parse_event(endpoint, &text)?;
                ensure_task_id(endpoint, &event, task_id)?;
                match event["header"]["event"].as_str() {
                    Some("task-started") => return Ok(()),
                    Some("task-failed") => return Err(task_failed(endpoint, &event)),
                    _ => {}
                }
            }
            Message::Close(_) => break,
            Message::Binary(_) => {
                return Err(invalid_event(
                    endpoint,
                    "received binary data before task-started",
                ));
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Err(invalid_event(
        endpoint,
        "connection closed before task-started",
    ))
}

async fn collect_final_sentences<S>(
    reader: &mut futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<S>>,
    endpoint: &str,
    task_id: &str,
) -> Result<String, InferenceError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut transcript = String::new();
    while let Some(message) = reader.next().await {
        match message.map_err(|error| websocket_error(endpoint, "receive", error))? {
            Message::Text(text) => {
                let event = parse_event(endpoint, &text)?;
                ensure_task_id(endpoint, &event, task_id)?;
                match event["header"]["event"].as_str() {
                    Some("result-generated") => {
                        let sentence = &event["payload"]["output"]["sentence"];
                        if sentence["sentence_end"].as_bool() == Some(true)
                            && sentence["heartbeat"].as_bool() != Some(true)
                        {
                            if let Some(text) = sentence["text"].as_str() {
                                transcript.push_str(text);
                            }
                        }
                    }
                    Some("task-finished") => return Ok(transcript),
                    Some("task-failed") => return Err(task_failed(endpoint, &event)),
                    _ => {}
                }
            }
            Message::Close(_) => {
                return Err(invalid_event(
                    endpoint,
                    "connection closed before task-finished",
                ));
            }
            Message::Binary(_) => {
                return Err(invalid_event(
                    endpoint,
                    "received unexpected binary response",
                ));
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Err(invalid_event(
        endpoint,
        "connection ended before task-finished",
    ))
}

fn parse_event(endpoint: &str, text: &str) -> Result<Value, InferenceError> {
    serde_json::from_str(text).map_err(|error| InferenceError::InvalidResponse {
        endpoint: endpoint.to_owned(),
        message: format!("invalid WebSocket event JSON: {error}"),
        body_preview: crate::error::preview(text),
    })
}

fn ensure_task_id(endpoint: &str, event: &Value, expected: &str) -> Result<(), InferenceError> {
    match event["header"]["task_id"].as_str() {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(invalid_event(
            endpoint,
            format!("received event for task {actual}, expected {expected}"),
        )),
        None => Err(invalid_event(endpoint, "event is missing header.task_id")),
    }
}

fn task_failed(endpoint: &str, event: &Value) -> InferenceError {
    let code = event["header"]["error_code"]
        .as_str()
        .unwrap_or("UNKNOWN_ERROR");
    let message = event["header"]["error_message"]
        .as_str()
        .unwrap_or("the provider did not include an error message");
    InferenceError::InvalidResponse {
        endpoint: endpoint.to_owned(),
        message: format!("Qwen Audio task failed ({code}): {message}"),
        body_preview: crate::error::preview(&event.to_string()),
    }
}

fn invalid_event(endpoint: &str, message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidResponse {
        endpoint: endpoint.to_owned(),
        message: message.into(),
        body_preview: String::new(),
    }
}

fn websocket_error(
    endpoint: &str,
    kind: &'static str,
    error: tokio_tungstenite::tungstenite::Error,
) -> InferenceError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
        let body_preview = response.body().as_ref().map_or_else(String::new, |body| {
            crate::error::preview(&String::from_utf8_lossy(body))
        });
        return InferenceError::HttpStatus {
            endpoint: endpoint.to_owned(),
            status: response.status().as_u16(),
            body_preview,
        };
    }
    InferenceError::Transport {
        endpoint: endpoint.to_owned(),
        source: TransportError::new(kind, error.to_string()),
    }
}

fn timeout_error(endpoint: &str, operation: &str) -> InferenceError {
    InferenceError::Transport {
        endpoint: endpoint.to_owned(),
        source: TransportError::new(
            "timeout",
            format!("Qwen Audio WebSocket {operation} exceeded its deadline"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{Message, handshake::server::Request},
    };

    use super::*;

    #[test]
    fn run_task_maps_language_context_and_vocabulary_without_instruction_semantics() {
        let options = QwenAudioStreamingOptions {
            language: Some("Japanese".into()),
            context_bias: Some("  XRTranslate、VRChat  ".into()),
            vocabulary_bias: vec![
                AsrVocabularyBias {
                    text: "XRTranslate".into(),
                    weight: 5,
                },
                AsrVocabularyBias {
                    text: "VRChat".into(),
                    weight: 50,
                },
            ],
        };
        let (request, requested_language) =
            build_run_task("task-1", "qwen-model", &options).unwrap();

        assert_eq!(requested_language.as_deref(), Some("Japanese"));
        assert_eq!(
            request["payload"]["parameters"]["language_hints"],
            json!(["ja"])
        );
        assert_eq!(
            request["payload"]["parameters"]["vocabulary"]["XRTranslate"],
            5
        );
        assert_eq!(request["payload"]["parameters"]["vocabulary"]["VRChat"], 50);
        assert_eq!(
            request["payload"]["input"]["context"][0]["content"][0],
            json!({"type": "input_text", "text": "XRTranslate、VRChat"})
        );
        assert!(!request.to_string().contains("instruction"));
    }

    #[test]
    fn language_names_and_bcp_47_codes_map_to_official_codes() {
        assert_eq!(map_language(Some("Chinese")).unwrap().0, Some("zh"));
        assert_eq!(map_language(Some("zh-TW")).unwrap().0, Some("zh"));
        assert_eq!(map_language(Some("Portuguese")).unwrap().0, Some("pt"));
        assert_eq!(map_language(Some("auto")).unwrap(), (None, None));
        assert!(map_language(Some("Klingon")).is_err());
    }

    #[test]
    fn vocabulary_rejects_weights_outside_the_provider_contract() {
        let error = validated_vocabulary(&[AsrVocabularyBias {
            text: "XRTranslate".into(),
            weight: 6,
        }])
        .unwrap_err();
        assert!(matches!(
            error,
            InferenceError::InvalidConfiguration {
                field: "vocabulary_bias.weight",
                ..
            }
        ));
    }

    #[test]
    fn adapter_rejects_cleartext_non_loopback_websocket() {
        let error = QwenAudioStreamingAdapter::new(
            "ws://example.com/api-ws/v1/inference",
            "qwen-model",
            "test-key",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            InferenceError::InvalidConfiguration {
                field: "endpoint",
                ..
            }
        ));
    }

    #[test]
    fn vocabulary_filters_provider_term_length_limits_without_failing_asr() {
        let non_ascii = validated_vocabulary(&[AsrVocabularyBias {
            text: "词".repeat(16),
            weight: 1,
        }])
        .unwrap();
        assert!(non_ascii.is_empty());

        let ascii = validated_vocabulary(&[AsrVocabularyBias {
            text: "one two three four five six seven eight".into(),
            weight: 1,
        }])
        .unwrap();
        assert!(ascii.is_empty());
    }

    #[test]
    fn duplicate_super_hot_words_count_once_after_normalization() {
        let vocabulary = (0..51)
            .map(|_| AsrVocabularyBias {
                text: "XRTranslate".into(),
                weight: 50,
            })
            .collect::<Vec<_>>();
        assert_eq!(validated_vocabulary(&vocabulary).unwrap().len(), 1);
    }

    #[test]
    fn extra_super_hot_words_are_filtered_without_failing_asr() {
        let vocabulary = (0..51)
            .map(|index| AsrVocabularyBias {
                text: format!("term-{index}"),
                weight: 50,
            })
            .collect::<Vec<_>>();
        assert_eq!(validated_vocabulary(&vocabulary).unwrap().len(), 50);
    }

    #[test]
    fn context_and_vocabulary_respect_provider_size_limits() {
        let options = QwenAudioStreamingOptions {
            context_bias: Some("词".repeat(MAX_CONTEXT_CHARS + 10)),
            ..QwenAudioStreamingOptions::default()
        };
        assert!(build_run_task("task-1", "qwen-model", &options).is_err());

        let options = QwenAudioStreamingOptions {
            context_bias: Some("词".repeat(MAX_CONTEXT_CHARS)),
            ..QwenAudioStreamingOptions::default()
        };
        let (request, _) = build_run_task("task-1", "qwen-model", &options).unwrap();
        assert_eq!(
            request["payload"]["input"]["context"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            MAX_CONTEXT_CHARS
        );

        let vocabulary = (0..=MAX_VOCABULARY_ENTRIES)
            .map(|index| AsrVocabularyBias {
                text: format!("term-{index}"),
                weight: 1,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validated_vocabulary(&vocabulary).unwrap().len(),
            MAX_VOCABULARY_ENTRIES
        );
    }

    #[tokio::test]
    async fn websocket_sends_authorized_ordered_frames_and_aggregates_final_sentences() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, |request: &Request, response| {
                assert_eq!(request.headers()[AUTHORIZATION], "Bearer test-secret");
                assert_eq!(
                    request.headers()["user-agent"],
                    "XRTranslate/qwen-audio-streaming"
                );
                Ok(response)
            })
            .await
            .unwrap();

            let run_task: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .as_str(),
            )
            .unwrap();
            assert_eq!(run_task["header"]["action"], "run-task");
            let task_id = run_task["header"]["task_id"].as_str().unwrap().to_owned();
            socket
                .send(Message::Text(
                    json!({"header":{"event":"task-started","task_id":task_id}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();

            assert_eq!(
                socket.next().await.unwrap().unwrap().into_data().len(),
                PCM_FRAME_BYTES
            );
            assert_eq!(socket.next().await.unwrap().unwrap().into_data().len(), 4);
            let finish: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .as_str(),
            )
            .unwrap();
            assert_eq!(finish["header"]["action"], "finish-task");
            assert_eq!(finish["header"]["task_id"], task_id);

            for event in [
                json!({"header":{"event":"result-generated","task_id":task_id},"payload":{"output":{"sentence":{"text":"ignored","heartbeat":true,"sentence_end":true}}}}),
                json!({"header":{"event":"result-generated","task_id":task_id},"payload":{"output":{"sentence":{"text":"partial","heartbeat":false,"sentence_end":false}}}}),
                json!({"header":{"event":"result-generated","task_id":task_id},"payload":{"output":{"sentence":{"text":"Hello, ","heartbeat":false,"sentence_end":true}}}}),
                json!({"header":{"event":"result-generated","task_id":task_id},"payload":{"output":{"sentence":{"text":"world.","sentence_end":true}}}}),
                json!({"header":{"event":"task-finished","task_id":task_id}}),
            ] {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
        });

        let adapter = QwenAudioStreamingAdapter::new(
            format!("ws://{address}"),
            "qwen-audio-3.0-asr-flash-streaming",
            "test-secret",
        )
        .unwrap();
        let transcript = adapter
            .transcribe_pcm16(
                &vec![0; PCM_FRAME_BYTES + 4],
                QwenAudioStreamingOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(transcript.text, "Hello, world.");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn task_failed_event_becomes_an_inference_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let run_task: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .as_str(),
            )
            .unwrap();
            let task_id = run_task["header"]["task_id"].as_str().unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "header": {
                            "event": "task-failed",
                            "task_id": task_id,
                            "error_code": "CLIENT_ERROR",
                            "error_message": "bad audio"
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let adapter = QwenAudioStreamingAdapter::new(
            format!("ws://{address}"),
            "qwen-audio-3.0-asr-flash-streaming",
            "test-secret",
        )
        .unwrap();
        let error = adapter
            .transcribe_pcm16(&[0, 0], QwenAudioStreamingOptions::default())
            .await
            .unwrap_err();

        assert!(matches!(error, InferenceError::InvalidResponse { .. }));
        assert!(error.to_string().contains("CLIENT_ERROR"));
        assert!(error.to_string().contains("bad audio"));
        server.await.unwrap();
    }
}
