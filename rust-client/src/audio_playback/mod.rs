//! Local audio playback shared by plugins.
mod player;
mod source;
pub(crate) use player::AudioPlayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}
