//! Transient request inspection data. Never used as model context.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptPartKind {
    Instruction,
    Style,
    History,
    Terminology,
    Context,
    Input,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPart {
    pub kind: PromptPartKind,
    pub text: String,
}

impl crate::PromptExecutionTrace {
    pub fn prompt_parts(&self, graph: &crate::PromptNodeGraph) -> Vec<PromptPart> {
        use crate::{PromptNodeKind, PromptSystemValue as Value};
        graph
            .nodes
            .iter()
            .filter_map(|node| {
                let kind = if graph
                    .translation_style
                    .as_ref()
                    .is_some_and(|style| style.node_id == node.id)
                {
                    PromptPartKind::Style
                } else {
                    match node.kind {
                        PromptNodeKind::SystemValue {
                            value: Value::CurrentInput,
                        } => PromptPartKind::Input,
                        PromptNodeKind::SystemValue {
                            value: Value::RecentTurns { .. } | Value::PreviousRevision,
                        } => PromptPartKind::History,
                        PromptNodeKind::SystemValue {
                            value: Value::Terminology,
                        } => PromptPartKind::Terminology,
                        PromptNodeKind::SystemValue {
                            value:
                                Value::SurroundingSource
                                | Value::RecognitionContext
                                | Value::LanguageOrder,
                        } => PromptPartKind::Context,
                        _ => return None,
                    }
                };
                let text = &self.node(&node.id)?.output;
                (!text.is_empty()).then(|| PromptPart {
                    kind,
                    text: text.clone(),
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequestRecord {
    pub id: u64,
    pub started_ms: u64,
    pub operation: String,
    pub model: String,
    pub request: Value,
    pub parts: Vec<PromptPart>,
    pub elapsed_ms: Option<u64>,
    pub error: Option<String>,
    pub output: String,
}

impl ModelRequestRecord {
    /// Keep message order and contents exactly as sent; wire parameters remain
    /// available separately in `request` (including multimodal descriptors).
    pub fn prompt_text(&self) -> String {
        let mut sections = Vec::new();
        if let Some(messages) = self.request.get("messages").and_then(Value::as_array) {
            for message in messages {
                let role = message["role"].as_str().unwrap_or("message");
                let content = &message["content"];
                sections.push(format!(
                    "[{role}]\n{}",
                    content.as_str().map(str::to_owned).unwrap_or_else(|| {
                        serde_json::to_string_pretty(content).unwrap_or_default()
                    })
                ));
            }
        }
        for key in ["prompt", "instructions", "input"] {
            if let Some(value) = self.request.get(key) {
                sections.push(format!(
                    "[{key}]\n{}",
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
                ));
            }
        }
        if let Some(domains) = self
            .request
            .pointer("/translation_options/domains")
            .and_then(Value::as_str)
        {
            sections.push(format!("[domains]\n{domains}"));
        }
        sections.join("\n\n")
    }

    pub fn report(&self) -> String {
        let mut parameters = self.request.clone();
        if let Some(object) = parameters.as_object_mut() {
            for key in ["messages", "prompt", "instructions", "input"] {
                object.remove(key);
            }
        }
        if let Some(options) = parameters
            .get_mut("translation_options")
            .and_then(Value::as_object_mut)
        {
            options.remove("domains");
        }
        format!(
            "Request #{} · {} · {}\nStarted: {} ms since Unix epoch\nElapsed: {}\nStatus: {}\n\n{}\n\nOutput:\n{}\n\nRequest parameters:\n{}",
            self.id,
            self.operation,
            self.model,
            self.started_ms,
            self.elapsed_ms
                .map_or_else(|| "pending".into(), |ms| format!("{ms} ms")),
            self.error
                .as_deref()
                .unwrap_or(if self.elapsed_ms.is_some() {
                    "completed"
                } else {
                    "running"
                }),
            self.prompt_text(),
            self.output,
            serde_json::to_string_pretty(&parameters).unwrap_or_default()
        )
    }
}
