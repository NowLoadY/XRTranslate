//! Shared presentation and input facts, independent of the output device.
mod commands;
pub use commands::Command;
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SurfaceStatus {
    pub available: bool,
    pub present: bool,
    pub attention: bool,
    pub greeting: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VoiceMode {
    pub requested: bool,
    pub active: bool,
    pub review: bool,
}
