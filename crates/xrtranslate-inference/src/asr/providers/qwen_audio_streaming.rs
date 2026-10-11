use futures_util::{SinkExt, Stream, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::time::{Duration, MissedTickBehavior, timeout};
use tokio::{net::TcpStream, sync::Mutex};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
    },
};
use uuid::Uuid;

use super::qwen_audio::{MAX_CONTEXT_CHARS, map_language, validated_vocabulary};
use crate::{AsrOptions, AsrTranscript, InferenceError, TransportError};
use xrtranslate_assets::remote::RemoteConnector;

const PCM_FRAME_BYTES: usize = 3_200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TASK_TIMEOUT: Duration = Duration::from_secs(90);
const IDLE_CONNECTION_TTL: Duration = Duration::from_secs(60);
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Qwen Audio streaming ASR over DashScope's native WebSocket protocol.
#[derive(Clone, Debug)]
pub struct QwenAudioStreamingAdapter {
    endpoint: String,
    model: String,
    api_key: String,
    idle_socket: Arc<Mutex<Option<(Socket, tokio::time::Instant)>>>,
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
            idle_socket: Arc::new(Mutex::new(None)),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Completed windows (including pseudo-streaming) use the same duplex
    /// transport, with pacing only because their audio is already recorded.
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let audio =
            futures_util::stream::unfold((pcm, interval), |(remaining, mut interval)| async move {
                if remaining.is_empty() {
                    return None;
                }
                interval.tick().await;
                let count = remaining.len().min(PCM_FRAME_BYTES);
                Some((remaining[..count].to_vec(), (&remaining[count..], interval)))
            });
        self.transcribe_stream(audio, options, |_| async {}).await
    }

    /// Audio arrives while the user speaks. Sender and receiver run concurrently;
    /// only EOF sends finish-task. Dropping this future closes the active socket.
    pub async fn transcribe_stream<S, F, Fut>(
        &self,
        audio: S,
        options: AsrOptions,
        on_update: F,
    ) -> Result<AsrTranscript, InferenceError>
    where
        S: Stream<Item = Vec<u8>> + Send,
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let task_id = Uuid::new_v4().to_string();
        let (run_task, requested_language) = build_run_task(&task_id, &self.model, &options)?;
        let cached = self
            .idle_socket
            .lock()
            .await
            .take()
            .filter(|(_, idle_since)| idle_since.elapsed() < IDLE_CONNECTION_TTL);
        let reused = cached.is_some();
        let mut socket = match cached {
            Some((socket, _)) => socket,
            None => self.connect().await?,
        };
        if let Err(error) = self.start_task(&mut socket, &task_id, &run_task).await {
            if !reused {
                return Err(error);
            }
            // An idle peer may close the connection. No audio was submitted,
            // so replace it once before beginning this turn, without a probe.
            socket = self.connect().await?;
            self.start_task(&mut socket, &task_id, &run_task).await?;
        }
        let (mut writer, mut reader) = socket.split();
        let send_audio = async {
            futures_util::pin_mut!(audio);
            let mut pending = Vec::with_capacity(PCM_FRAME_BYTES * 2);
            while let Some(frame) = audio.next().await {
                if !frame.len().is_multiple_of(2) {
                    return Err(InferenceError::InvalidAudio {
                        message: "PCM16 input length must be even".into(),
                    });
                }
                pending.extend_from_slice(&frame);
                while pending.len() >= PCM_FRAME_BYTES {
                    let frame = pending.drain(..PCM_FRAME_BYTES).collect::<Vec<_>>();
                    writer
                        .send(Message::Binary(frame.into()))
                        .await
                        .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
                }
            }
            if !pending.is_empty() {
                writer
                    .send(Message::Binary(pending.into()))
                    .await
                    .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            }
            writer
                .send(Message::Text(finish_task(&task_id).to_string().into()))
                .await
                .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            Result::<(), InferenceError>::Ok(())
        };
        let receive_results = collect_sentences(&mut reader, &self.endpoint, &task_id, on_update);
        let ((), text) = timeout(TASK_TIMEOUT, async {
            tokio::try_join!(send_audio, receive_results)
        })
        .await
        .map_err(|_| timeout_error(&self.endpoint, "recognition"))??;
        // Only a normally finished task returns its connection to this session's
        // one-slot cache. Failed/cancelled tasks never contaminate the next turn.
        if let Ok(socket) = writer.reunite(reader) {
            *self.idle_socket.lock().await = Some((socket, tokio::time::Instant::now()));
        }
        Ok(AsrTranscript {
            language: requested_language,
            text,
        })
    }

    async fn connect(&self) -> Result<Socket, InferenceError> {
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
        request
            .headers_mut()
            .insert("user-agent", HeaderValue::from_static("XRTranslate/asr"));
        timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| timeout_error(&self.endpoint, "connect"))?
            .map(|(socket, _)| socket)
            .map_err(|error| websocket_error(&self.endpoint, "connect", error))
    }

    async fn start_task(
        &self,
        socket: &mut Socket,
        task_id: &str,
        run_task: &Value,
    ) -> Result<(), InferenceError> {
        timeout(CONNECT_TIMEOUT, async {
            socket
                .send(Message::Text(run_task.to_string().into()))
                .await
                .map_err(|error| websocket_error(&self.endpoint, "send", error))?;
            wait_for_task_started(socket, &self.endpoint, task_id).await
        })
        .await
        .map_err(|_| timeout_error(&self.endpoint, "task-started"))?
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
    options: &AsrOptions,
) -> Result<(Value, Option<String>), InferenceError> {
    let (language_code, requested_language) = map_language(
        RemoteConnector::DashscopeAsrWebsocket,
        model,
        options.language.as_deref(),
    )?;
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

async fn collect_sentences<S, F, Fut>(
    reader: &mut futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<S>>,
    endpoint: &str,
    task_id: &str,
    mut on_update: F,
) -> Result<String, InferenceError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut committed = String::new();
    let mut last_final_id = None;
    let mut active_id = None;
    let mut last_preview = String::new();
    while let Some(message) = reader.next().await {
        match message.map_err(|error| websocket_error(endpoint, "receive", error))? {
            Message::Text(text) => {
                let event = parse_event(endpoint, &text)?;
                ensure_task_id(endpoint, &event, task_id)?;
                match event["header"]["event"].as_str() {
                    Some("result-generated") => {
                        let sentence = &event["payload"]["output"]["sentence"];
                        if sentence["heartbeat"].as_bool() == Some(true) {
                            continue;
                        }
                        let Some(text) = sentence["text"].as_str().map(str::trim) else {
                            continue;
                        };
                        if text.is_empty() && sentence["sentence_end"].as_bool() != Some(true) {
                            continue;
                        }
                        let id = sentence["sentence_id"]
                            .as_u64()
                            .or_else(|| sentence["begin_time"].as_u64())
                            .ok_or_else(|| {
                                invalid_event(endpoint, "sentence identity is missing")
                            })?;
                        if last_final_id.is_some_and(|last| id <= last)
                            || active_id.is_some_and(|active| id < active)
                        {
                            continue;
                        }
                        active_id = Some(id);
                        let mut snapshot = committed.clone();
                        append_sentence(&mut snapshot, text);
                        if sentence["sentence_end"].as_bool() == Some(true) {
                            committed = snapshot.clone();
                            last_final_id = Some(id);
                            active_id = None;
                        }
                        if snapshot != last_preview {
                            last_preview = snapshot.clone();
                            on_update(snapshot).await;
                        }
                    }
                    Some("task-finished") => {
                        if active_id.is_some() {
                            return Err(invalid_event(
                                endpoint,
                                "task finished without finalizing its last sentence",
                            ));
                        }
                        return Ok(committed);
                    }
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

fn append_sentence(output: &mut String, next: &str) {
    if output
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && next
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        output.push(' ');
    }
    output.push_str(next);
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
