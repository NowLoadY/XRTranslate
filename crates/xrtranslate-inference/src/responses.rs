//! ChatGPT plan transport. Keeps the shared prompt graph and translation output
//! checks intact while speaking the official Responses API contract.
mod stream;
use std::ops::ControlFlow;

use crate::{AsyncHttpClient, ChatCompletion, HttpRequest, InferenceError, ReqwestClient};
use serde_json::{Value, json};
use xrtranslate_connections::{
    AccountStore, ConnectionError,
    chatgpt::{self, RESPONSES_ENDPOINT},
};

#[derive(Debug, Clone)]
pub(crate) struct ChatGptClient {
    accounts: AccountStore,
    http: ReqwestClient,
}

impl ChatGptClient {
    pub fn new(accounts: AccountStore, endpoint: &str) -> Result<Self, InferenceError> {
        if endpoint != RESPONSES_ENDPOINT {
            return Err(InferenceError::InvalidConfiguration {
                field: "translation.url",
                message: "ChatGPT login requires its official Responses endpoint".into(),
            });
        }
        let http = ReqwestClient::for_account().map_err(|source| InferenceError::Transport {
            endpoint: RESPONSES_ENDPOINT.into(),
            source,
        })?;
        Ok(Self { accounts, http })
    }

    pub async fn complete<F, Fut>(
        &self,
        chat_payload: Value,
        mut on_update: F,
    ) -> Result<ChatCompletion, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let payload = responses_payload(chat_payload)?;
        let grant = chatgpt::access(&self.accounts, payload["model"].as_str())
            .await
            .map_err(account_error)?;
        let mut request = HttpRequest::post_json(RESPONSES_ENDPOINT, payload);
        request.headers.extend([
            ("authorization".into(), grant.authorization()),
            ("accept".into(), "text/event-stream".into()),
        ]);
        let mut stream = stream::ResponseStream::default();
        let response = self
            .http
            .execute_streaming(request, |chunk| {
                let update = stream.push(&chunk);
                let completed = stream.is_completed();
                let callback = update
                    .as_ref()
                    .ok()
                    .and_then(Clone::clone)
                    .map(&mut on_update);
                async move {
                    update?;
                    if let Some(callback) = callback {
                        callback.await;
                    }
                    Ok(if completed {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    })
                }
            })
            .await
            .map_err(|source| InferenceError::Transport {
                endpoint: RESPONSES_ENDPOINT.into(),
                source,
            })?;
        if response.status == 401 {
            let _ = chatgpt::invalidate(&self.accounts, &grant).await;
            return Err(InferenceError::AccountLoginRequired {
                provider: "ChatGPT",
            });
        }
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: RESPONSES_ENDPOINT.into(), status: response.status,
                retry_after: response.retry_after,
                body_preview: match response.status {
                    403 => "ChatGPT plan access is unavailable. Check this app's permissions in ChatGPT Settings.",
                    429 => "ChatGPT usage limit reached. Check your plan usage and try again later.",
                    400 => "ChatGPT rejected this model or request. Refresh the account's model list.",
                    _ => "ChatGPT could not complete the request. Please try again later.",
                }.into(),
            });
        }
        let completion = stream
            .finish()
            .map_err(|source| InferenceError::Transport {
                endpoint: RESPONSES_ENDPOINT.into(),
                source,
            })?;
        on_update(completion.text.clone()).await;
        Ok(completion)
    }
}

fn account_error(error: ConnectionError) -> InferenceError {
    if error == ConnectionError::LoginRequired {
        InferenceError::AccountLoginRequired {
            provider: "ChatGPT",
        }
    } else {
        InferenceError::InvalidConfiguration {
            field: "translation.account",
            message: error.to_string(),
        }
    }
}

fn responses_payload(chat: Value) -> Result<Value, InferenceError> {
    let invalid = || InferenceError::InvalidConfiguration {
        field: "translation.messages",
        message: "ChatGPT requires ordered text chat messages".into(),
    };
    let model = chat["model"]
        .as_str()
        .filter(|model| !model.is_empty())
        .ok_or_else(invalid)?;
    let messages = chat["messages"].as_array().ok_or_else(invalid)?;
    let input = messages
        .iter()
        .map(|message| {
            let role = match message["role"].as_str() {
                Some("system" | "developer") => "developer",
                Some("user") => "user",
                Some("assistant") => "assistant",
                _ => return Err(invalid()),
            };
            let text = message["content"].as_str().ok_or_else(invalid)?;
            let mut item = json!({"role": role, "content": text});
            // This adapter exposes final answers only, never provider commentary.
            // Preserve that phase when callers replay the returned answer.
            if role == "assistant" {
                item["phase"] = json!("final_answer");
            }
            Ok(item)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if input.is_empty() {
        return Err(invalid());
    }
    // This authorization route rejects sampling/output-limit fields and requires
    // streaming, including callers that only want the final translation/Ask result.
    let mut payload = json!({"model": model, "input": input, "store": false, "stream": true});
    if let Some(format) = chat.get("response_format") {
        let format = if format["type"] == "json_schema" {
            let mut schema = format["json_schema"].clone();
            schema["type"] = json!("json_schema");
            schema
        } else {
            format.clone()
        };
        payload["text"] = json!({"format": format});
    }
    if let Some(effort) = chat.get("reasoning_effort").and_then(Value::as_str) {
        payload["reasoning"] = json!({"effort": effort});
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_graph_messages_without_unsupported_chat_parameters() {
        let result = responses_payload(json!({"model":"account-model", "messages":[
            {"role":"system", "content":"Translate faithfully."},
            {"role":"user", "content":"你好"},
            {"role":"assistant", "content":"Hello"},
            {"role":"user", "content":"再见"}
        ], "temperature":0.7,"max_tokens":256,"stream":false,"reasoning_effort":"low","response_format":{"type":"json_schema","json_schema":{"name":"response","strict":false,"schema":{"type":"object"}}}}))
        .unwrap();
        assert_eq!(
            result["input"][0],
            json!({"role":"developer","content":"Translate faithfully."})
        );
        assert_eq!(result["input"][2]["role"], "assistant");
        assert_eq!(result["input"][2]["phase"], "final_answer");
        assert!(result["input"][3].get("phase").is_none());
        assert_eq!(result["input"][3]["content"], "再见");
        assert_eq!(result["stream"], true);
        assert_eq!(result["store"], false);
        assert_eq!(
            result["text"]["format"],
            json!({"type":"json_schema","name":"response","strict":false,"schema":{"type":"object"}})
        );
        assert!(result.get("conversation").is_none());
        assert!(result.get("previous_response_id").is_none());
        assert_eq!(result["reasoning"], json!({"effort":"low"}));
        assert!(result.get("reasoning_effort").is_none());
        assert!(result.get("temperature").is_none() && result.get("max_tokens").is_none());
        assert!(
            ChatGptClient::new(
                AccountStore::for_project_root("unused"),
                "https://example.org/v1/responses"
            )
            .is_err()
        );
    }
}
