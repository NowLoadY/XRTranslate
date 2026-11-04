use crate::{ChatCompletion, TokenUsage, TransportError};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct ResponseStream {
    pending: Vec<u8>,
    data: String,
    text: String,
    outputs: BTreeMap<u64, String>,
    commentary: BTreeSet<u64>,
    completed: bool,
    usage: Option<TokenUsage>,
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
            Some("response.output_item.added") => {
                if value["item"]["phase"] == "commentary" {
                    let index = value["output_index"]
                        .as_u64()
                        .ok_or_else(|| error("Missing ChatGPT output index"))?;
                    self.commentary.insert(index);
                }
                Ok(false)
            }
            Some("response.output_text.delta") => {
                if value["output_index"]
                    .as_u64()
                    .is_some_and(|index| self.commentary.contains(&index))
                {
                    return Ok(false);
                }
                self.text.push_str(
                    value["delta"]
                        .as_str()
                        .ok_or_else(|| error("Missing ChatGPT text delta"))?,
                );
                Ok(true)
            }
            Some("response.output_item.done") => {
                if let Some(text) = assistant_text(&value["item"])? {
                    let index = value["output_index"]
                        .as_u64()
                        .ok_or_else(|| error("Missing ChatGPT output index"))?;
                    self.outputs.insert(index, text);
                }
                Ok(false)
            }
            Some("response.completed") => {
                self.usage = TokenUsage::from_json(value["response"].get("usage"));
                let response = &value["response"];
                if response["status"] != "completed" {
                    return Err(error("ChatGPT did not complete the response"));
                }
                let output = response["output"]
                    .as_array()
                    .ok_or_else(|| error("Missing ChatGPT final output"))?;
                let mut final_outputs = BTreeMap::new();
                for (index, item) in output.iter().enumerate() {
                    if let Some(text) = assistant_text(item)? {
                        final_outputs.insert(index as u64, text);
                    }
                }
                // Some account responses send completed items separately and
                // leave this final snapshot's output array empty. Otherwise the
                // final snapshot is authoritative; its indices may differ from
                // the streamed indices when reasoning items are omitted.
                if !final_outputs.is_empty() {
                    self.outputs = final_outputs;
                }
                self.text = self.outputs.values().map(String::as_str).collect();
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
        Ok(ChatCompletion {
            text: self.text,
            usage: self.usage,
        })
    }
}

fn assistant_text(item: &Value) -> Result<Option<String>, TransportError> {
    if item["type"] != "message" || item["role"] != "assistant" || item["phase"] == "commentary" {
        return Ok(None);
    }
    if item
        .get("status")
        .is_some_and(|status| status != "completed")
    {
        return Err(error("ChatGPT output message was not completed"));
    }
    let parts = item["content"]
        .as_array()
        .ok_or_else(|| error("Missing ChatGPT output content"))?;
    let mut text = String::new();
    for part in parts {
        if part["type"] == "output_text" {
            text.push_str(
                part["text"]
                    .as_str()
                    .ok_or_else(|| error("Invalid ChatGPT final text"))?,
            );
        }
    }
    Ok(Some(text))
}

fn error(message: &str) -> TransportError {
    TransportError::new("responses_stream", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commentary_is_not_an_answer_or_a_streaming_update() {
        let mut stream = ResponseStream::default();
        let mut push = |value: Value| {
            stream
                .push(format!("data: {value}\n\n").as_bytes())
                .unwrap()
        };
        let note = serde_json::json!({"type":"message","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"preliminary action"}]});
        assert!(push(serde_json::json!({"type":"response.output_item.added","output_index":1,"item":note})).is_none());
        assert!(push(serde_json::json!({"type":"response.output_text.delta","output_index":1,"delta":"preliminary action"})).is_none());
        push(serde_json::json!({"type":"response.output_item.done","output_index":1,"item":note}));
        let answer = serde_json::json!({"type":"message","role":"assistant","phase":"final_answer","status":"completed","content":[{"type":"output_text","text":"final decision"}]});
        push(
            serde_json::json!({"type":"response.output_item.added","output_index":3,"item":answer}),
        );
        assert_eq!(push(serde_json::json!({"type":"response.output_text.delta","output_index":3,"delta":"final decision"})).as_deref(), Some("final decision"));
        push(
            serde_json::json!({"type":"response.output_item.done","output_index":3,"item":answer}),
        );
        push(
            serde_json::json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        );
        assert_eq!(stream.finish().unwrap().text, "final decision");
    }

    #[test]
    fn accepts_only_explicit_completed_output_across_utf8_chunk_boundaries() {
        let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\r\n\r\n";
        let final_event = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":2048,\"input_tokens_details\":{\"cached_tokens\":1024}},\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好。\"}]}]}}\n\n";
        let mut stream = ResponseStream::default();
        for b in delta.as_bytes() {
            stream.push(&[*b]).unwrap();
        }
        stream.push(final_event.as_bytes()).unwrap();
        let completion = stream.finish().unwrap();
        assert_eq!(completion.text, "你好。");
        assert_eq!(completion.usage.unwrap().cached_input_tokens, Some(1024));
        // A streamed message can be index 1 after reasoning, while the final
        // snapshot contains only that message at index 0. Never concatenate it twice.
        let item = "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好。\"}]}}\n\n";
        for final_event in [
            final_event,
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n",
        ] {
            let mut stream = ResponseStream::default();
            stream.push(item.as_bytes()).unwrap();
            stream.push(final_event.as_bytes()).unwrap();
            assert_eq!(stream.finish().unwrap().text, "你好。");
        }
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
