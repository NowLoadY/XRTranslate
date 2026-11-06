//! Local audio playback shared by plugins.
mod player;
mod source;
pub use player::AudioPlayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}
