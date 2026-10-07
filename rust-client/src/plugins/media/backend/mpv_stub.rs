//! Capability stub used when the native MPV backend is not selected.
//!
//! Keeping the same backend type lets the host remain platform-neutral while
//! making the missing optional capability explicit to the user.

use super::{MediaBackend, PlaybackStatus, PlayerDiagnostics};
use std::path::PathBuf;

const UNAVAILABLE: &str = "MPV playback is unavailable; rebuild rust-client with the `mpv` feature";

pub struct MpvBackend;

impl MpvBackend {
    pub fn new() -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }
}

impl MediaBackend for MpvBackend {
    fn load_local_file(&mut self, _path: PathBuf) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }

    fn load_stream_url(&mut self, _url: String) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }

    fn play(&mut self) {}
    fn pause(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _ms: i64) {}
    fn set_volume(&mut self, _volume: f32) {}
    fn set_mute(&mut self, _mute: bool) {}
    fn get_time_ms(&self) -> i64 {
        0
    }
    fn get_duration_ms(&self) -> i64 {
        0
    }
    fn get_status(&self) -> PlaybackStatus {
        PlaybackStatus::Stopped
    }
    fn get_diagnostics(&self) -> PlayerDiagnostics {
        PlayerDiagnostics::default()
    }
    fn tick(&mut self) {}
    fn attach_native_host(&mut self, _host_handle: *mut std::ffi::c_void) {}
    fn set_osd_subtitle(&mut self, _text: &str) {}
    fn get_audio_channel_count(&self) -> Option<usize> {
        None
    }
    fn get_audio_layout(&self) -> Option<String> {
        None
    }
    fn set_channel_routing(&mut self, _channels: &[super::super::task::AudioChannelItem]) {}
    fn set_audio_only_mode(&mut self, _enabled: bool) {}
}
