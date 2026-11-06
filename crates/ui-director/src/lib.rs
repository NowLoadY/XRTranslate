//! Provider- and plugin-neutral operation contract shared by UI Director and agents.
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub mod catalog;

/// The visible page and active item path, independent of any application's routes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub page: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<LocationSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationSegment {
    /// The item's existing domain ID, usable by the application's tools.
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub read_only: bool,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        properties: Value,
        required: &[&str],
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
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
pub trait ToolProvider {
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

pub type ToolResult = Result<Value, String>;

pub fn arguments<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, String> {
    serde_json::from_value(call.arguments.clone())
        .map_err(|error| format!("Invalid arguments: {error}"))
}

/// One dispatch path for embedded agents, CLI requests and application plugins.
pub fn dispatch(providers: &mut [&mut dyn ToolProvider], call: &ToolCall) -> ToolResult {
    if catalog::handles(&call.name) {
        let tools = providers
            .iter()
            .flat_map(|provider| provider.tools())
            .collect::<Vec<_>>();
        return catalog::call(&tools, call);
    }
    providers
        .iter_mut()
        .find(|provider| provider.tools().iter().any(|tool| tool.name == call.name))
        .ok_or_else(|| format!("Tool unavailable: {}", call.name))?
        .call(call)
}

pub mod cli;
#[cfg(feature = "egui")]
pub mod egui;
pub mod feedback;

pub const DEFAULT_PORT: u16 = 18920;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Response {
    pub fn ok(message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            message: error.into(),
            data: None,
        }
    }
}
