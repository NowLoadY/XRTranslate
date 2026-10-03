use serde_json::Value;

use super::{ChatCompletion, content_to_text, parse_chat_completion, validate_finish_reason};
use crate::{InferenceError, TransportError};

/// SSE framing is byte based, so UTF-8 characters may span HTTP chunks safely.
pub(super) struct ChatStream {
    pending: Vec<u8>,
    data: String,
    text: String,
    cumulative: bool,
    finished: bool,
    finish_reason: Option<String>,
    json: bool,
}

impl ChatStream {
    pub(super) fn new(cumulative: bool) -> Self {
        Self {
            pending: Vec::new(),
            data: String::new(),
            text: String::new(),
            cumulative,
            finished: false,
            finish_reason: None,
            json: false,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Option<String>, TransportError> {
        const MAX_BYTES: usize = 1_048_576;
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_BYTES {
            return Err(stream_error("completion frame exceeded its size limit"));
        }
        // Some compatible servers ignore stream=true and return ordinary JSON.
        self.json |= self.pending.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{');
        if self.json {
            return Ok(None);
        }
        let mut changed = false;
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|_| stream_error("completion stream contained invalid UTF-8"))?
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
                return Err(stream_error("completion text exceeded its size limit"));
            }
        }
        Ok(changed.then(|| self.text.clone()))
    }

    fn dispatch(&mut self) -> Result<bool, TransportError> {
        let data = std::mem::take(&mut self.data);
        let data = data.trim();
        if data.is_empty() || self.finished {
            return Ok(false);
        }
        if data == "[DONE]" {
            self.finished = true;
            return Ok(false);
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|_| stream_error("completion event was not valid JSON"))?;
        if value.get("error").is_some() {
            return Err(stream_error("provider reported a completion stream error"));
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        else {
            return Ok(false); // A usage-only chunk has no choices.
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.into());
            self.finished = true;
        }
        let content = choice.pointer("/delta/content");
        let Some(text) = content
            .and_then(content_to_text)
            .filter(|text| !text.is_empty())
        else {
            return Ok(false);
        };
        if self.cumulative {
            if self.text == text {
                return Ok(false);
            }
            self.text = text;
        } else {
            self.text.push_str(&text);
        }
        Ok(true)
    }

    pub(super) fn finish(mut self, endpoint: &str) -> Result<ChatCompletion, InferenceError> {
        if self.json {
            return parse_chat_completion(endpoint, &String::from_utf8_lossy(&self.pending));
        }
        self.push(b"\n\n")
            .map_err(|source| InferenceError::Transport {
                endpoint: endpoint.into(),
                source,
            })?;
        if !self.finished {
            return Err(InferenceError::Transport {
                endpoint: endpoint.into(),
                source: stream_error("completion stream ended before its final event"),
            });
        }
        validate_finish_reason(endpoint, self.finish_reason.as_deref())?;
        Ok(ChatCompletion { text: self.text })
    }
}

fn stream_error(message: &str) -> TransportError {
    TransportError::new("completion_stream", message)
}
