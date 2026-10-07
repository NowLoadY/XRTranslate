#[cfg(feature = "mpv")]
pub mod mpv;
#[cfg(not(feature = "mpv"))]
#[path = "mpv_stub.rs"]
pub mod mpv;
pub mod window;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaSource {
    LocalFile(PathBuf),
    NetworkStream(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    #[cfg_attr(not(feature = "mpv"), allow(dead_code))]
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Default)]
pub struct PlayerDiagnostics {
    pub hwdec_current: String,
    pub video_codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub dropped_frames: i64,
}

pub trait MediaBackend: Send {
    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String>;
    fn load_stream_url(&mut self, url: String) -> Result<(), String>;

    fn play(&mut self);
    fn pause(&mut self);
    fn stop(&mut self);
    fn seek(&mut self, ms: i64);

    fn set_volume(&mut self, volume: f32);
    fn set_mute(&mut self, mute: bool);

    fn get_time_ms(&self) -> i64;
    fn get_duration_ms(&self) -> i64;
    fn get_status(&self) -> PlaybackStatus;
    fn get_diagnostics(&self) -> PlayerDiagnostics;

    fn tick(&mut self);
    /// Attach to a native video host. A null handle detaches the current host.
    fn attach_native_host(&mut self, host_handle: *mut std::ffi::c_void);
    fn set_osd_subtitle(&mut self, text: &str);
    fn get_audio_channel_count(&self) -> Option<usize>;
    #[allow(dead_code)]
    fn get_audio_layout(&self) -> Option<String>;
    fn set_channel_routing(&mut self, channels: &[super::task::AudioChannelItem]);
    fn set_audio_only_mode(&mut self, enabled: bool);
}
