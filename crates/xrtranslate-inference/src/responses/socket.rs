//! Upgrade through the same TLS and system-proxy stack used by HTTPS.
use super::{key::KeyResponses, stream::ResponseStream};
use crate::{InferenceError, TransportError};
use futures_util::{SinkExt, StreamExt};
use reqwest::header;
use serde_json::Value;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::{client::generate_key, derive_accept_key},
        protocol::Role,
    },
};

pub(super) type Socket = WebSocketStream<reqwest::Upgraded>;

impl KeyResponses {
    pub(super) async fn connect(&self) -> Result<Socket, InferenceError> {
        let key = generate_key();
        // HTTP/2 ALPN strips the HTTP/1.1 Upgrade headers even when the
        // individual request asks for HTTP/1.1. Negotiate H1 on this socket.
        let client = xrtranslate_http::client_builder()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| self.transport("connect", e))?;
        let response = client
            .get(&self.endpoint)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, &key)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .map_err(|e| self.transport("connect", e))?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status().as_u16(),
                retry_after: None,
                body_preview: "Responses WebSocket upgrade unavailable".into(),
            });
        }
        let accept = derive_accept_key(key.as_bytes());
        if response
            .headers()
            .get(header::SEC_WEBSOCKET_ACCEPT)
            .and_then(|v| v.to_str().ok())
            != Some(&accept)
        {
            return Err(self.transport("connect", "Invalid WebSocket handshake"));
        }
        let io = response
            .upgrade()
            .await
            .map_err(|e| self.transport("connect", e))?;
        Ok(WebSocketStream::from_raw_socket(io, Role::Client, None).await)
    }

    pub(super) async fn exchange(
        &self,
        socket: &mut Socket,
        mut payload: Value,
    ) -> Result<Value, InferenceError> {
        payload["type"] = Value::from("response.create");
        socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .map_err(|e| self.transport("connect", e))?;
        let mut stream = ResponseStream::native();
        while let Some(frame) = socket.next().await {
            match frame.map_err(|e| self.transport("response_body", e))? {
                Message::Text(text) => {
                    let event =
                        serde_json::from_str(&text).map_err(|e| self.transport("responses", e))?;
                    stream
                        .event(event)
                        .map_err(|source| InferenceError::Transport {
                            endpoint: self.endpoint.clone(),
                            source,
                        })?;
                    if let Some(response) = stream.response.take() {
                        return Ok(response);
                    }
                }
                Message::Ping(data) => socket
                    .send(Message::Pong(data))
                    .await
                    .map_err(|e| self.transport("response_body", e))?,
                Message::Close(_) => break,
                _ => {}
            }
        }
        Err(self.transport(
            "response_body",
            "Responses WebSocket closed before completion",
        ))
    }

    pub(super) fn transport(&self, kind: &str, error: impl std::fmt::Display) -> InferenceError {
        InferenceError::Transport {
            endpoint: self.endpoint.clone(),
            source: TransportError::new(kind, error.to_string()),
        }
    }
}
