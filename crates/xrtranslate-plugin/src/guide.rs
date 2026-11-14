//! Guide planning stays in a plugin; the host renders these presentation facts.
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "guide.presentation.v1";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub page: String,
    pub language: String,
    pub pages: Vec<Page>,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub route: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub text: String,
    pub targets: Vec<String>,
    pub reveal: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Audio>,
}

/// A bundled clip, also used by the video workflow to align narration precisely.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Audio {
    pub file: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub page: String,
    pub label: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub chapters: Vec<Chapter>,
    pub chapter: usize,
    pub step: usize,
}

impl Session {
    pub fn current(&self) -> Option<(&Chapter, &Step)> {
        let chapter = self.chapters.get(self.chapter)?;
        Some((chapter, chapter.steps.get(self.step)?))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub revision: u64,
    pub session: Option<Session>,
    /// The host has applied this revision and completed its view preparation.
    pub ready: bool,
}
