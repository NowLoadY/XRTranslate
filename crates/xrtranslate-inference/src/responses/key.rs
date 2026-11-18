use super::{
    agent::{Catalogue, Previous, input_item},
    responses_payload,
    socket::Socket,
    stream::ResponseStream,
};
use crate::{ChatCompletion, InferenceError, ReqwestClient};
use reqwest::header::HeaderValue;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use xrtranslate_prompt::{PromptMessage, PromptMessageRole};
use xrtranslate_protocol::AgentTurn;

#[derive(Default)]
struct Connection {
    socket: Option<Socket>,
    retry_upgrade_at: Option<Instant>,
    previous: Option<Previous>,
}

#[derive(Clone)]
pub(crate) struct KeyResponses {
    pub(super) endpoint: String,
    pub(super) authorization: HeaderValue,
    pub(super) http: ReqwestClient,
    connection: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for KeyResponses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyResponses")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl KeyResponses {
    pub fn new(endpoint: &str, key: &str) -> Result<Self, InferenceError> {
        let error = |field, message: &str| InferenceError::InvalidConfiguration {
            field,
            message: message.into(),
        };
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| error("endpoint", "Expected a Responses URL"))?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            return Err(error("endpoint", "Expected an HTTP(S) Responses URL"));
        }
        if key.trim().is_empty() {
            return Err(error("api_key", "OpenAI API key is missing"));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", key.trim()))
            .map_err(|_| error("api_key", "Invalid API key"))?;
        authorization.set_sensitive(true);
        let http = ReqwestClient::for_responses().map_err(|source| InferenceError::Transport {
            endpoint: endpoint.into(),
            source,
        })?;
        Ok(Self {
            endpoint: endpoint.into(),
            authorization,
            http,
            connection: Default::default(),
        })
    }

    async fn sse<F, Fut>(
        &self,
        mut payload: Value,
        stream: &mut ResponseStream,
        mut update: F,
    ) -> Result<(), InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        payload["stream"] = json!(true);
        let authorization = self.authorization.to_str().expect("validated bearer");
        let response = super::sse(
            &self.http,
            &self.endpoint,
            authorization.to_owned(),
            payload,
            stream,
            &mut update,
        )
        .await?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: crate::error::preview(
                    &response.body.replace(
                        authorization
                            .strip_prefix("Bearer ")
                            .expect("validated bearer"),
                        "[redacted]",
                    ),
                ),
                retry_after: response.retry_after,
            });
        }
        Ok(())
    }

    pub async fn complete<F, Fut>(
        &self,
        chat: Value,
        mut update: F,
    ) -> Result<ChatCompletion, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let max_tokens = chat["max_tokens"].clone();
        let mut payload = responses_payload(chat)?;
        if max_tokens.is_number() {
            payload["max_output_tokens"] = max_tokens;
        }
        let mut stream = ResponseStream::default();
        self.sse(payload, &mut stream, &mut update).await?;
        let completion = stream
            .finish()
            .map_err(|source| InferenceError::Transport {
                endpoint: self.endpoint.clone(),
                source,
            })?;
        update(completion.text.clone()).await;
        Ok(completion)
    }

    pub async fn agent(
        &self,
        model: &str,
        effort: &str,
        messages: &[PromptMessage],
        turn: &AgentTurn,
        window: u32,
    ) -> Result<ChatCompletion, InferenceError> {
        if turn.session.is_empty() || messages.is_empty() || turn.tools.is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "agent_turn",
                message: "A session, transcript and scoped tool catalogue are required".into(),
            });
        }
        let catalogue = Catalogue::new(turn);
        let mut state = self.connection.lock().await;
        let delta = state
            .previous
            .as_ref()
            .and_then(|p| p.delta(messages, turn, &catalogue));
        let continuation = delta.is_some();
        let (input_limit, output_limit) = xrtranslate_assets::remote::openai_agent_limits(window);
        let input = delta.unwrap_or_else(|| messages.iter().map(input_item).collect());
        let mut payload = json!({
            "model": model,
            "input": input,
            "tools": catalogue.definitions,
            "parallel_tool_calls": false,
            "reasoning": {"effort": effort},
            "store": false,
            "max_output_tokens": output_limit,
            "prompt_cache_options": {"ttl": "30m"},
            "include": ["reasoning.encrypted_content"]
        });
        // OpenAI's auto-compaction example uses 200K; always leave headroom
        // inside a smaller user-selected window. API minimum is 1,000 tokens.
        let threshold = 200_000.min(input_limit.saturating_mul(3) / 4);
        if threshold >= 1000 {
            payload["context_management"] =
                json!([{"type":"compaction","compact_threshold":threshold}]);
        }
        let started = Instant::now();
        let mut record = crate::diagnostics::RequestRecord::start("Agent", &payload, Vec::new());
        let exchange = tokio::time::timeout(
            Duration::from_secs(150),
            state.request(self, payload, &input, continuation),
        )
        .await
        .unwrap_or_else(|_| Err(self.transport("timeout", "Responses turn deadline reached")));
        let result = exchange.and_then(|reply| {
            let (completion, call_id) = catalogue.decision(&reply.response)?;
            let output = reply.response["output"]
                .as_array()
                .expect("validated response output");
            let compaction = output.iter().rposition(|item| item["type"] == "compaction");
            let id = reply.response["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| self.transport("responses", "Missing response ID"))?
                .to_owned();
            let transport = if reply.socket.is_some() {
                "websocket"
            } else {
                "https"
            };
            tracing::info!(
                model, transport, incremental = reply.incremental, compacted = compaction.is_some(),
                elapsed_ms = started.elapsed().as_millis(), usage = ?completion.usage,
                "OpenAI Agent turn completed"
            );
            // Only stateless recovery uses this copy. A live response ID retains its chain.
            let replay = if let Some(index) = compaction {
                output[index..].to_vec()
            } else {
                let mut retained = state
                    .previous
                    .take()
                    .filter(|_| continuation)
                    .map_or_else(Vec::new, |previous| previous.replay);
                retained.extend(input);
                retained.extend(output.iter().cloned());
                retained
            };
            let mut expected = messages.to_vec();
            expected.push(PromptMessage {
                role: PromptMessageRole::Assistant,
                content: completion.text.clone(),
            });
            state.previous = Some(Previous {
                session: turn.session.clone(),
                scope: turn.scope.clone(),
                definitions: catalogue.definitions,
                expected,
                id,
                call_id,
                replay,
            });
            state.socket = reply.socket;
            Ok(completion)
        });
        if let Ok(completion) = &result {
            record.capture_output(&completion.text);
        }
        record.finish(result.as_ref().map(|c| c.text.as_str()));
        result
    }
}

struct Reply {
    response: Value,
    socket: Option<Socket>,
    incremental: bool,
}

impl Connection {
    async fn open(&mut self, client: &KeyResponses) -> Option<Socket> {
        if self.retry_upgrade_at.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        match client.connect().await {
            Ok(socket) => {
                self.retry_upgrade_at = None;
                Some(socket)
            }
            Err(error) => {
                tracing::info!(%error, "Responses WebSocket upgrade unavailable; using HTTPS streaming");
                self.retry_upgrade_at = Some(Instant::now() + Duration::from_secs(60));
                None
            }
        }
    }

    async fn request(
        &mut self,
        client: &KeyResponses,
        mut payload: Value,
        input: &[Value],
        continuation: bool,
    ) -> Result<Reply, InferenceError> {
        // Taking ownership before I/O also drops an in-flight socket on cancellation.
        let mut socket = self.socket.take();
        let previous = self.previous.as_ref().filter(|_| continuation);
        if let (Some(active), Some(previous)) = (&mut socket, previous) {
            let mut delta = payload.clone();
            delta["previous_response_id"] = json!(previous.id);
            match client.exchange(active, delta).await {
                Ok(response) => {
                    return Ok(Reply {
                        response,
                        socket,
                        incremental: true,
                    });
                }
                Err(InferenceError::Transport { source, .. }) => match source.kind.as_str() {
                    "previous_response_not_found" | "previous_response_id_not_found" => {
                        tracing::debug!("Previous Responses ID expired, replaying on this socket");
                    }
                    "connect" | "response_body" | "websocket_connection_limit_reached" => {
                        tracing::debug!(kind = source.kind, "Reconnecting the Responses socket");
                        socket = None;
                    }
                    _ => {
                        return Err(InferenceError::Transport {
                            endpoint: client.endpoint.clone(),
                            source,
                        });
                    }
                },
                Err(error) => return Err(error),
            }
        }
        if let Some(previous) = previous {
            payload["input"] = Value::Array(previous.replay.iter().chain(input).cloned().collect());
        }
        if socket.is_none() {
            socket = self.open(client).await;
        }
        let response = if let Some(active) = &mut socket {
            client.exchange(active, payload).await?
        } else {
            let mut stream = ResponseStream::native();
            client
                .sse(payload, &mut stream, |_| std::future::ready(()))
                .await?;
            stream.response.take().ok_or_else(|| {
                client.transport("response_body", "Stream ended before response.completed")
            })?
        };
        Ok(Reply {
            response,
            socket,
            incremental: false,
        })
    }
}
