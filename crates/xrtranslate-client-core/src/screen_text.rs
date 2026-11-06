//! Shared screen observation and presentation values.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenTextState {
    pub source: String,
    pub status: Option<String>,
    pub busy: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum CaptureEvent {
    Busy(u64),
    Text(u64, String),
    Idle(u64),
    Failed {
        revision: u64,
        error: String,
        fatal: bool,
    },
}

impl CaptureEvent {
    pub fn revision(&self) -> u64 {
        match self {
            Self::Busy(revision)
            | Self::Text(revision, _)
            | Self::Idle(revision)
            | Self::Failed { revision, .. } => *revision,
        }
    }
}
