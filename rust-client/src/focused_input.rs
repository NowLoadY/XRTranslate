//! Native insertion into an already focused editable control.
//!
//! This capability has no translation, queue, clipboard, or plugin policy. Its
//! caller owns a background worker and creates/uses/drops `Platform` there.

#[cfg(windows)]
#[path = "focused_input/windows.rs"]
mod platform;

#[cfg(windows)]
pub(crate) use platform::{Platform, timestamp};

#[cfg(not(windows))]
pub(crate) fn timestamp() -> u64 {
    0
}

#[cfg(not(windows))]
pub(crate) struct Platform;

#[cfg(not(windows))]
pub(crate) struct Target;

#[cfg(not(windows))]
impl Platform {
    pub(crate) fn new() -> Result<Self, String> {
        Err("Focused text input is only supported on Windows".to_owned())
    }

    pub(crate) fn poll_events(&self) {}

    pub(crate) fn focused_target_since(&self, _produced_at: u64) -> Result<Option<Target>, String> {
        Ok(None)
    }

    pub(crate) fn insert_if<G>(
        &self,
        _target: &Target,
        _text: &str,
        _permit: impl FnOnce() -> Option<G>,
    ) -> Result<bool, String> {
        Ok(false)
    }
}
