//! Global shortcut preferences survive platform changes.
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "linux"))]
mod native;
#[cfg(any(windows, target_os = "linux"))]
pub(crate) use native::{Event, Listener, clipboard_text};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub enabled: bool,
    pub shortcut: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: "Ctrl+Alt+Y".into(),
        }
    }
}
