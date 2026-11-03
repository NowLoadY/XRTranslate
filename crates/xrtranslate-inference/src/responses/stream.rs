use crate::{ChatCompletion, TransportError};
use serde_json::Value;

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct ResponseStream {
    pending: Vec<u8>,
    data: String,
    text: String,
    completed: bool,
}

impl ResponseStream {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<String>, TransportError> {
        if self.pending.len() + bytes.len() > MAX_BYTES {
            return Err(error("ChatGPT response frame exceeded its size limit"));
        }
        self.pending.extend_from_slice(bytes);
        let mut changed = false;
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|_| error("Invalid UTF-8 in ChatGPT response"))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                changed |= self.dispatch()?;
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            }
            if self.data.len() > MAX_BYTES || self.text.len() > MAX_BYTES {
                return Err(error("ChatGPT response exceeded its size limit"));
            }
        }
        Ok(changed.then(|| self.text.clone()))
    }

    fn dispatch(&mut self) -> Result<bool, TransportError> {
        let data = std::mem::take(&mut self.data);
        if data.trim().is_empty() || self.completed {
            return Ok(false);
        }
        if data.trim() == "[DONE]" {
            return Ok(false);
        } // Not proof of completed inference.
        let value: Value =
            serde_json::from_str(&data).map_err(|_| error("Invalid ChatGPT response event"))?;
        match value["type"].as_str() {
            Some("response.output_text.delta") => {
                self.text.push_str(
                    value["delta"]
                        .as_str()
                        .ok_or_else(|| error("Missing ChatGPT text delta"))?,
                );
                Ok(true)
            }
            Some("response.completed") => {
                let response = &value["response"];
                if response["status"] != "completed" {
                    return Err(error("ChatGPT did not complete the response"));
                }
                let output = response["output"]
                    .as_array()
                    .ok_or_else(|| error("Missing ChatGPT final output"))?;
                let mut text = String::new();
                for item in output {
                    if item["type"] != "message" || item["role"] != "assistant" {
                        continue;
                    }
                    if let Some(parts) = item["content"].as_array() {
                        for part in parts {
                            if part["type"] == "output_text" {
                                text.push_str(
                                    part["text"]
                                        .as_str()
                                        .ok_or_else(|| error("Invalid ChatGPT final text"))?,
                                );
                            }
                        }
                    }
                }
                self.text = text;
                self.completed = true;
                Ok(true)
            }
            Some("response.failed" | "error") => {
                let code = value
                    .pointer("/response/error/code")
                    .or_else(|| value.get("code"))
                    .and_then(Value::as_str);
                Err(error(match code {
                    Some(
                        "subscription_sharing_usage_limit_exceeded"
                        | "subscription_sharing_usage_unavailable",
                    ) => {
                        "ChatGPT plan usage is unavailable or its limit has been reached. Check usage in ChatGPT Settings."
                    }
                    Some(
                        "subscription_sharing_user_not_eligible"
                        | "chatpass_v2_scope_not_authorized",
                    ) => {
                        "This ChatGPT account has not authorized eligible plan usage for XRTranslate."
                    }
                    _ => "ChatGPT failed to complete the request. Please try again later.",
                }))
            }
            Some("response.incomplete") => Err(error(
                "ChatGPT returned an incomplete response; partial text was not accepted",
            )),
            _ => Ok(false), // Lifecycle, reasoning and usage events are not translated text.
        }
    }

    pub(super) fn is_completed(&self) -> bool {
        self.completed
    }

    pub fn finish(mut self) -> Result<ChatCompletion, TransportError> {
        self.push(b"\n\n")?;
        if !self.completed {
            return Err(error("ChatGPT stream ended before response.completed"));
        }
        Ok(ChatCompletion { text: self.text })
    }
}

fn error(message: &str) -> TransportError {
    TransportError::new("responses_stream", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_explicit_completed_output_across_utf8_chunk_boundaries() {
        let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\r\n\r\n";
        let final_event = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好。\"}]}]}}\n\n";
        let mut stream = ResponseStream::default();
        for b in delta.as_bytes() {
            stream.push(&[*b]).unwrap();
        }
        stream.push(final_event.as_bytes()).unwrap();
        assert_eq!(stream.finish().unwrap().text, "你好。");
        for terminal in [
            "",
            "data: [DONE]\n\n",
            "data: {\"type\":\"response.incomplete\"}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n\n",
        ] {
            let mut stream = ResponseStream::default();
            stream.push(delta.as_bytes()).unwrap();
            assert!(
                stream
                    .push(terminal.as_bytes())
                    .and_then(|_| stream.finish())
                    .is_err()
            );
        }
    }
}
