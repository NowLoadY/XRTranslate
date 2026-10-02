use serde_json::{Value, json};

mod stream;

use crate::{AsyncHttpClient, HttpRequest, InferenceError, MultipartBody, error::preview};

/// Text extracted from the first OpenAI chat-completions choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCompletion {
    pub text: String,
}

/// Shared authenticated OpenAI HTTP adapter.
///
/// `endpoint` must be the full chat-completions or audio-transcriptions URL. Requiring the
/// exact endpoint avoids silently choosing a wrong API version when external
/// providers are configured.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleClient<C> {
    http: C,
    endpoint: String,
    headers: Vec<(String, String)>,
}

impl<C> OpenAiCompatibleClient<C> {
    pub fn new(http: C, endpoint: impl Into<String>) -> Result<Self, InferenceError> {
        Self::with_headers(http, endpoint, Vec::new())
    }

    /// Creates a client with additional headers, for example a remote
    /// provider's `authorization: Bearer …` credential. Local llama-server
    /// callers should use [`Self::new`] and therefore send no credential.
    pub fn with_headers(
        http: C,
        endpoint: impl Into<String>,
        headers: Vec<(String, String)>,
    ) -> Result<Self, InferenceError> {
        let endpoint = endpoint.into().trim().to_owned();
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err(InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: "must start with http:// or https://".into(),
            });
        }
        if !endpoint.contains("/chat/completions") && !endpoint.contains("/audio/transcriptions") {
            return Err(InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: "must point to /chat/completions or /audio/transcriptions".into(),
            });
        }
        Ok(Self {
            http,
            endpoint,
            headers,
        })
    }

    /// Adds an OpenAI-style bearer token without exposing it in payload JSON.
    pub fn with_bearer_token(
        http: C,
        endpoint: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let token = token.into().trim().to_owned();
        if token.is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "api_key",
                message: "must not be empty when bearer authentication is configured".into(),
            });
        }
        Self::with_headers(
            http,
            endpoint,
            vec![("authorization".into(), format!("Bearer {token}"))],
        )
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn into_inner(self) -> C {
        self.http
    }
}

impl<C: AsyncHttpClient> OpenAiCompatibleClient<C> {
    /// Posts a complete, non-streaming chat-completions JSON payload.
    pub async fn chat_completion(&self, payload: Value) -> Result<ChatCompletion, InferenceError> {
        let body = self
            .execute(HttpRequest {
                headers: self
                    .headers
                    .iter()
                    .cloned()
                    .chain(std::iter::once((
                        "content-type".into(),
                        "application/json".into(),
                    )))
                    .collect(),
                ..HttpRequest::post_json(self.endpoint.clone(), payload)
            })
            .await?;
        parse_chat_completion(&self.endpoint, &body)
    }

    pub(crate) async fn chat_completion_streaming<F, Fut>(
        &self,
        mut payload: Value,
        cumulative: bool,
        mut on_update: F,
    ) -> Result<ChatCompletion, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        payload["stream"] = Value::Bool(true);
        let mut request = HttpRequest::post_json(self.endpoint.clone(), payload);
        request.headers.extend(self.headers.iter().cloned());
        request
            .headers
            .push(("accept".into(), "text/event-stream".into()));
        let mut stream = stream::ChatStream::new(cumulative);
        let response = self
            .http
            .execute_streaming(request, |chunk| {
                let update = stream.push(&chunk);
                let callback = update
                    .as_ref()
                    .ok()
                    .and_then(|value| value.clone())
                    .map(&mut on_update);
                async move {
                    update?;
                    if let Some(callback) = callback {
                        callback.await;
                    }
                    Ok(())
                }
            })
            .await
            .map_err(|source| InferenceError::Transport {
                endpoint: self.endpoint.clone(),
                source,
            })?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: preview(&response.body),
            });
        }
        let completion = stream.finish(&self.endpoint)?;
        on_update(completion.text.clone()).await;
        Ok(completion)
    }

    pub async fn audio_transcription(
        &self,
        fields: Vec<(String, String)>,
        wav: Vec<u8>,
    ) -> Result<String, InferenceError> {
        let body = self
            .execute(HttpRequest {
                method: "POST".into(),
                url: self.endpoint.clone(),
                headers: self.headers.clone(),
                body: Value::Null,
                multipart: Some(MultipartBody {
                    fields,
                    file_name: "speech.wav".into(),
                    file: wav,
                    mime_type: "audio/wav".into(),
                }),
            })
            .await?;
        let value: Value =
            serde_json::from_str(&body).map_err(|error| InferenceError::InvalidResponse {
                endpoint: self.endpoint.clone(),
                message: error.to_string(),
                body_preview: preview(&body),
            })?;
        value["text"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| InferenceError::InvalidResponse {
                endpoint: self.endpoint.clone(),
                message: "missing transcription text".into(),
                body_preview: preview(&body),
            })
    }

    async fn execute(&self, request: HttpRequest) -> Result<String, InferenceError> {
        let response =
            self.http
                .execute(request)
                .await
                .map_err(|source| InferenceError::Transport {
                    endpoint: self.endpoint.clone(),
                    source,
                })?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: preview(&response.body),
            });
        }
        Ok(response.body)
    }
}

pub(crate) fn parse_chat_completion(
    endpoint: &str,
    body: &str,
) -> Result<ChatCompletion, InferenceError> {
    let json: Value =
        serde_json::from_str(body).map_err(|error| InferenceError::InvalidResponse {
            endpoint: endpoint.to_owned(),
            message: format!("body is not JSON: {error}"),
            body_preview: preview(body),
        })?;
    let Some(choice) = json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return invalid_response(endpoint, body, "choices[0] is missing");
    };
    let Some(content) = choice.pointer("/message/content") else {
        return invalid_response(endpoint, body, "choices[0].message.content is missing");
    };
    let text = content_to_text(content).ok_or_else(|| InferenceError::InvalidResponse {
        endpoint: endpoint.to_owned(),
        message: "choices[0].message.content must be text or text content parts".into(),
        body_preview: preview(body),
    })?;
    Ok(ChatCompletion { text })
}

fn invalid_response<T>(endpoint: &str, body: &str, message: &str) -> Result<T, InferenceError> {
    Err(InferenceError::InvalidResponse {
        endpoint: endpoint.to_owned(),
        message: message.into(),
        body_preview: preview(body),
    })
}

fn content_to_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let parts = content.as_array()?;
    let mut text = String::new();
    for part in parts {
        match part {
            Value::String(value) => text.push_str(value),
            Value::Object(_) => {
                let value = part.get("text").and_then(Value::as_str)?;
                text.push_str(value);
            }
            _ => return None,
        }
    }
    Some(text)
}

pub(crate) fn non_streaming_chat_payload(
    model: &str,
    messages: Value,
    temperature: f64,
    max_tokens: u32,
) -> Value {
    json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false,
    })
}

/// Removes explicit completion markers that llama.cpp may surface as text.
pub(crate) fn remove_completion_markers(text: &str) -> String {
    text.trim()
        .replace("<|endoftext|>", "")
        .replace("<|im_end|>", "")
        .replace("</s>", "")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_or_content_parts() {
        let string = parse_chat_completion(
            "http://test/v1/chat/completions",
            r#"{"choices":[{"message":{"content":"hello"}}]}"#,
        )
        .unwrap();
        assert_eq!(string.text, "hello");
        let parts = parse_chat_completion("http://test/v1/chat/completions", r#"{"choices":[{"message":{"content":[{"type":"text","text":"he"},{"type":"text","text":"llo"}]}}]}"#)
            .unwrap();
        assert_eq!(parts.text, "hello");

        let wire = concat!(
            ": keepalive\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"好。\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let mut stream = stream::ChatStream::new(false);
        let mut updates = Vec::new();
        for byte in wire.bytes() {
            if let Some(text) = stream.push(&[byte]).unwrap() {
                updates.push(text);
            }
        }
        assert_eq!(updates, ["你", "你好。"]);
        assert_eq!(stream.finish("http://test").unwrap().text, "你好。");

        let mut cumulative = stream::ChatStream::new(true);
        cumulative
            .push(wire.replace("好。", "你好。").as_bytes())
            .unwrap();
        assert_eq!(cumulative.finish("http://test").unwrap().text, "你好。");
        let mut unfinished = stream::ChatStream::new(false);
        unfinished
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n")
            .unwrap();
        assert!(unfinished.finish("http://test").is_err());
        let mut truncated = stream::ChatStream::new(false);
        assert!(
            truncated
                .push(b"data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\n")
                .is_err()
        );
    }
}
