//! Provider- and plugin-neutral operation contract shared by UI Director and agents.
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub(crate) mod catalog;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub read_only: bool,
}

impl Tool {
    pub fn new(
        name: &'static str,
        description: &'static str,
        properties: Value,
        required: &[&str],
    ) -> Self {
        Self {
            name,
            description,
            read_only: false,
            input_schema: serde_json::json!({
                "type": "object", "properties": properties, "required": required,
                "additionalProperties": false,
            }),
        }
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
}

/// Plugins publish operations and keep their validation and implementation.
/// The Director only discovers a provider and forwards the unchanged call.
pub(crate) trait ToolProvider {
    fn tools(&self) -> Vec<Tool>;
    fn call(&mut self, call: &ToolCall) -> ToolResult;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
}

fn empty_object() -> Value {
    serde_json::json!({})
}

pub(crate) type ToolResult = Result<Value, String>;

pub(crate) fn arguments<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, String> {
    serde_json::from_value(call.arguments.clone())
        .map_err(|error| format!("Invalid arguments: {error}"))
}
