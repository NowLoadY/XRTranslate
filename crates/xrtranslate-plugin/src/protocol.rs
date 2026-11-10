use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

pub type Result = std::result::Result<Value, String>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Text(pub BTreeMap<String, String>);
impl Text {
    pub fn get(&self, language: &str) -> &str {
        self.0
            .get(language)
            .or_else(|| self.0.get("en"))
            .map(String::as_str)
            .unwrap_or("")
    }
}
impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self(BTreeMap::from([("en".into(), value.into())]))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    /// Optional release metadata. Compatibility depends only on min_host_version.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub min_host_version: String,
    pub title: Text,
    #[serde(default)]
    pub description: Text,
    pub executable: String,
    /// Opt in to application release bundles; independent installation is unaffected.
    #[serde(default)]
    pub bundle_with_host: bool,
    #[serde(default)]
    pub page: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default = "default_navigation_order")]
    pub navigation_order: i32,
    #[serde(default)]
    pub navigation_group: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub models: Vec<Model>,
}
fn default_navigation_order() -> i32 {
    500
}
impl Manifest {
    pub fn validate(&self) -> std::result::Result<(), String> {
        identifier(&self.id)?;
        relative_path(&self.executable)?;
        if let Some(icon) = &self.icon {
            relative_path(icon)?;
        }
        semver::Version::parse(&self.min_host_version).map_err(|error| error.to_string())?;
        if self.title.get("en").is_empty() {
            return Err("Plugin name is required".into());
        }
        let mut names = std::collections::HashSet::new();
        for tool in &self.tools {
            if !tool.name.starts_with(&format!("{}.", self.id)) || !names.insert(&tool.name) {
                return Err(format!("Invalid or repeated plugin tool: {}", tool.name));
            }
        }
        let mut models = std::collections::HashSet::new();
        for model in &self.models {
            identifier(&model.id)?;
            relative_path(&model.directory)?;
            if !models.insert(&model.id) || model.files.is_empty() {
                return Err("Invalid model catalogue".into());
            }
            let mut files = std::collections::HashSet::new();
            for file in &model.files {
                relative_path(&file.path)?;
                if !files.insert(&file.path) || file.bytes == 0 || !file.url.starts_with("https://")
                {
                    return Err("Invalid model file".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub directory: String,
    pub revision: String,
    #[serde(default)]
    pub runtime: String,
    pub files: Vec<ModelFile>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub url: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub host_executable: PathBuf,
    pub language: String,
    pub data_directory: PathBuf,
    pub model_directory: Option<PathBuf>,
    pub runtimes: BTreeMap<String, PathBuf>,
    pub config: Value,
    pub capabilities: Vec<String>,
}

/// Calls, results and events have the same shape in either direction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Initialize {
        environment: Environment,
    },
    Call {
        id: u64,
        name: String,
        arguments: Value,
    },
    Reply {
        id: u64,
        result: Result,
    },
    Cancel {
        id: u64,
    },
    Event {
        name: String,
        value: Value,
    },
    View {
        widgets: Vec<Widget>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    /// A plugin-owned immediate-mode interface rendered by the host GPU.
    Surface,
    Label {
        text: Text,
    },
    Heading {
        text: Text,
    },
    Row {
        children: Vec<Widget>,
    },
    Column {
        children: Vec<Widget>,
    },
    Card {
        children: Vec<Widget>,
    },
    Link {
        text: Text,
        url: String,
    },
    Button {
        id: String,
        label: Text,
        #[serde(default = "yes")]
        enabled: bool,
        #[serde(default)]
        primary: bool,
    },
    Input {
        id: String,
        label: Text,
        value: String,
        #[serde(default = "yes")]
        enabled: bool,
    },
    Toggle {
        id: String,
        label: Text,
        value: bool,
        #[serde(default = "yes")]
        enabled: bool,
    },
    Select {
        id: String,
        label: Text,
        value: String,
        choices: Vec<(String, Text)>,
    },
    Number {
        id: String,
        label: Text,
        value: f64,
        min: f64,
        max: f64,
    },
    Scene {
        id: String,
        scene: Scene,
    },
    Host {
        capability: String,
        #[serde(default = "yes")]
        enabled: bool,
    },
    Busy,
    Error {
        message: String,
    },
}
fn yes() -> bool {
    true
}

/// General 3D points and lines. The host owns projection and camera interaction.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub points: Vec<[f32; 3]>,
    pub lines: Vec<[usize; 2]>,
    pub target: [f32; 3],
    pub revision: u64,
}

pub fn identifier(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(format!("Invalid plugin identifier: {value}"));
    }
    Ok(())
}
pub fn relative_path(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.contains(['\\', ':'])
        || !Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(format!("Invalid package path: {value}"));
    }
    Ok(())
}
