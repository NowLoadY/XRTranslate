//! Global shortcut preferences survive platform changes.
use serde::{Deserialize, Serialize};
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub(crate) use native::{Event, Listener};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub enabled: bool,
    pub shortcut: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: cfg!(windows),
            shortcut: "Ctrl+Alt+Y".into(),
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn clipboard_text() -> Result<String, String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.get_text())
        .map_err(|_| "Copy some text, then try again.".into())
}
