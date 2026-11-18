//! DashScope Responses uses stored response IDs over SSE and model-specific caching.
use super::{agent::Previous, key::KeyResponses};
use crate::InferenceError;
use serde_json::{Value, json};

pub(super) fn configure(payload: &mut Value, model: &str, effort: Option<&str>) {
    payload["store"] = json!(true);
    if let Some(effort) = effort {
        payload["reasoning"] = json!({"effort": effort});
    }
    if model == "qwen4-max" {
        payload["prompt_cache_options"] = json!({"mode":"implicit", "ttl":"5m"});
        payload["reasoning"]["summary"] = json!("auto");
        payload["include"] = json!(["reasoning.encrypted_content"]);
    }
}

pub(super) fn replayable(item: &Value) -> bool {
    matches!(item["type"].as_str(), Some("function_call" | "message"))
        || (item["type"] == "reasoning" && item["encrypted_content"].is_string())
}

fn missing_previous(error: &InferenceError) -> bool {
    fn matches(code: &str, message: &str) -> bool {
        if matches!(
            code,
            "previous_response_not_found" | "previous_response_id_not_found"
        ) {
            return true;
        }
        let message = message.to_ascii_lowercase();
        message.contains("previous_response_id")
            && (message.contains("not found") || message.contains("expired"))
            && matches!(
                code,
                "InvalidParameter" | "InvalidRequest" | "invalid_request_error" | "NotFound"
            )
    }
    match error {
        InferenceError::HttpStatus {
            status: 400 | 404,
            body_preview,
            ..
        } => serde_json::from_str::<Value>(body_preview).is_ok_and(|body| {
            let err = body.get("error").unwrap_or(&body);
            matches(
                err["code"].as_str().unwrap_or(""),
                err["message"].as_str().unwrap_or(""),
            )
        }),
        InferenceError::Transport { source, .. } => matches(&source.kind, &source.message),
        _ => false,
    }
}

impl KeyResponses {
    pub(super) async fn dashscope_request(
        &self,
        mut payload: Value,
        input: &[Value],
        previous: Option<&Previous>,
    ) -> Result<(Value, bool), InferenceError> {
        if let Some(previous) = previous {
            let mut delta = payload.clone();
            delta["previous_response_id"] = json!(previous.id);
            match self.native_sse(delta).await {
                Ok(response) => return Ok((response, true)),
                Err(error) if missing_previous(&error) => {
                    tracing::debug!(
                        "DashScope response ID expired, replaying the accepted local history"
                    );
                    payload["input"] =
                        Value::Array(previous.replay.iter().chain(input).cloned().collect());
                }
                Err(error) => return Err(error),
            }
        }
        self.native_sse(payload)
            .await
            .map(|response| (response, false))
    }
}
