//! One bounded in-memory journal for requests, including failures and cancellation.
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use xrtranslate_prompt::diagnostics::{ModelRequestRecord, PromptPart, PromptPartKind};

const MAX_REQUESTS: usize = 64;
const MAX_BYTES: usize = 8 * 1024 * 1024;
static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    next_id: 0,
    entries: VecDeque::new(),
    bytes: 0,
});

struct Journal {
    next_id: u64,
    entries: VecDeque<(ModelRequestRecord, usize)>,
    bytes: usize,
}

impl Journal {
    fn trim(&mut self) {
        // Preserve the latest complete request even when it alone exceeds the budget.
        while self.entries.len() > 1
            && (self.entries.len() > MAX_REQUESTS || self.bytes > MAX_BYTES)
        {
            self.bytes = self
                .bytes
                .saturating_sub(self.entries.pop_front().unwrap().1);
        }
    }
}

fn record_bytes(record: &ModelRequestRecord) -> usize {
    record.request.to_string().len()
        + record
            .parts
            .iter()
            .map(|part| part.text.len())
            .sum::<usize>()
        + record.output.len()
        + record.error.as_ref().map_or(0, String::len)
}

pub fn requests() -> Vec<ModelRequestRecord> {
    JOURNAL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .iter()
        .map(|(record, _)| record.clone())
        .collect()
}

pub fn clear_requests() {
    let mut journal = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    journal.entries.clear();
    journal.bytes = 0;
}

pub struct RequestRecord {
    id: u64,
    started: Instant,
    finished: bool,
    output: String,
}

impl RequestRecord {
    pub fn start(operation: &str, request: &Value, mut parts: Vec<PromptPart>) -> Self {
        if parts.is_empty()
            && let Some(messages) = request.get("messages").and_then(Value::as_array)
        {
            let current = messages
                .iter()
                .rposition(|message| message["role"] == "user");
            for (index, message) in messages.iter().enumerate() {
                if let Some(text) = message["content"].as_str() {
                    let kind = if message["role"] == "system" {
                        PromptPartKind::Instruction
                    } else if Some(index) == current {
                        PromptPartKind::Input
                    } else {
                        PromptPartKind::History
                    };
                    parts.push(PromptPart {
                        kind,
                        text: text.into(),
                    });
                }
            }
        }
        let mut journal = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
        journal.next_id += 1;
        let id = journal.next_id;
        let record = ModelRequestRecord {
            id,
            started_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            operation: operation.into(),
            model: request["model"].as_str().unwrap_or_default().into(),
            request: inspectable(request),
            parts,
            elapsed_ms: None,
            error: None,
            output: String::new(),
        };
        let bytes = record_bytes(&record);
        journal.entries.push_back((record, bytes));
        journal.bytes += bytes;
        journal.trim();
        Self {
            id,
            started: Instant::now(),
            finished: false,
            output: String::new(),
        }
    }

    pub fn capture_output(&mut self, output: &str) {
        self.output = excerpt(output);
    }

    pub fn finish<E: std::fmt::Display>(mut self, result: Result<&str, E>) {
        self.update(result.map(str::to_owned).map_err(|error| error.to_string()));
        self.finished = true;
    }

    fn update(&self, result: Result<String, String>) {
        let mut journal = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((record, bytes)) = journal
            .entries
            .iter_mut()
            .find(|(entry, _)| entry.id == self.id)
        {
            let previous = record.output.len() + record.error.as_ref().map_or(0, String::len);
            record.elapsed_ms = Some(self.started.elapsed().as_millis() as u64);
            match result {
                Ok(output) => record.output = excerpt(&output),
                Err(error) => {
                    record.error = Some(excerpt(&error));
                    record.output.clone_from(&self.output);
                }
            }
            let added = record.output.len() + record.error.as_ref().map_or(0, String::len);
            *bytes = bytes.saturating_sub(previous) + added;
            journal.bytes = journal.bytes.saturating_sub(previous) + added;
        }
        journal.trim();
    }
}

impl Drop for RequestRecord {
    fn drop(&mut self) {
        if !self.finished {
            self.update(Err("Cancelled".into()));
        }
    }
}

fn excerpt(text: &str) -> String {
    let mut result: String = text.chars().take(4096).collect();
    if result.len() < text.len() {
        result.push_str("\n[…]");
    }
    result
}

/// Authentication headers are never passed here; omit credentials and binary
/// media from payloads without shortening any textual prompt.
fn inspectable(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let hidden = matches!(
                        key.to_ascii_lowercase().as_str(),
                        "authorization"
                            | "api_key"
                            | "access_token"
                            | "refresh_token"
                            | "cookie"
                            | "password"
                    );
                    let media = matches!(key.as_str(), "input_audio" | "image_url" | "audio_url");
                    (
                        key.clone(),
                        if hidden {
                            Value::String("[redacted]".into())
                        } else if media {
                            Value::String("[media omitted]".into())
                        } else {
                            inspectable(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(inspectable).collect()),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: u64, text: &str) -> ModelRequestRecord {
        ModelRequestRecord {
            id,
            started_ms: 0,
            operation: "fixture".into(),
            model: "fixture".into(),
            request: json!({"messages":[{"role":"user","content":text}]}),
            parts: Vec::new(),
            elapsed_ms: None,
            error: None,
            output: String::new(),
        }
    }

    fn push(journal: &mut Journal, record: ModelRequestRecord) {
        let size = record_bytes(&record);
        journal.entries.push_back((record, size));
        journal.bytes += size;
        journal.trim();
    }

    #[test]
    fn diagnostics_evicts_old_requests_without_truncating_the_latest_prompt() {
        let mut journal = Journal {
            next_id: 0,
            entries: VecDeque::new(),
            bytes: 0,
        };
        for id in 0..80 {
            push(&mut journal, record(id, "complete prompt"));
        }
        assert_eq!(journal.entries.len(), MAX_REQUESTS);
        assert_eq!(journal.entries.front().unwrap().0.id, 16);
        let large = "完整风格与当前输入".repeat(MAX_BYTES / 12);
        push(&mut journal, record(80, &large));
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(
            journal.entries[0].0.request["messages"][0]["content"],
            large
        );
        push(&mut journal, record(81, "new request"));
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].0.id, 81);
        assert!(journal.bytes < MAX_BYTES);
    }

    #[test]
    fn diagnostics_omits_credentials_and_audio_but_preserves_text() {
        let text = "Full style.\n完整输入。".repeat(100);
        let request = json!({"api_key":"private", "messages":[{"role":"user","content":[
            {"type":"input_text","text":text}, {"type":"input_audio","input_audio":{"data":"binary payload"}}
        ]}]});
        let captured = inspectable(&request);
        assert_eq!(captured["messages"][0]["content"][0]["text"], text);
        assert!(!captured.to_string().contains("private"));
        assert!(!captured.to_string().contains("binary payload"));
    }

    #[test]
    fn diagnostics_records_failure_and_cancellation_separately_from_success() {
        let request = json!({"model":"diagnostic-fixture", "messages":[]});
        let failed = RequestRecord::start("fixture", &request, Vec::new());
        let failure_id = failed.id;
        failed.finish(Err::<&str, _>("HTTP 503 service unavailable"));
        let cancelled = RequestRecord::start("fixture", &request, Vec::new());
        let cancelled_id = cancelled.id;
        drop(cancelled);
        let records = requests();
        assert_eq!(
            records
                .iter()
                .find(|r| r.id == failure_id)
                .unwrap()
                .error
                .as_deref(),
            Some("HTTP 503 service unavailable")
        );
        assert_eq!(
            records
                .iter()
                .find(|r| r.id == cancelled_id)
                .unwrap()
                .error
                .as_deref(),
            Some("Cancelled")
        );
    }
}
