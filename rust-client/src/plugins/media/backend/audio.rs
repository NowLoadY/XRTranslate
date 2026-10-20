//! Adapt shared local audio playback to the media player's optional video API.
use super::{MediaBackend, PlaybackStatus, PlayerDiagnostics};
use crate::audio_playback::AudioPlayer;
use std::path::PathBuf;

pub struct AudioBackend(AudioPlayer);

impl AudioBackend {
    pub fn new() -> Result<Self, String> {
        AudioPlayer::new().map(Self)
    }
}

impl MediaBackend for AudioBackend {
    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String> {
        self.0.load_local_file(path)
    }
    fn load_stream_url(&mut self, _: String) -> Result<(), String> {
        Err("Playback is unavailable for this file.".into())
    }
    fn play(&mut self) {
        self.0.play();
    }
    fn pause(&mut self) {
        self.0.pause();
    }
    fn stop(&mut self) {
        self.0.stop();
    }
    fn seek(&mut self, ms: i64) {
        self.0.seek(ms);
    }
    fn set_volume(&mut self, volume: f32) {
        self.0.set_volume(volume);
    }
    fn set_mute(&mut self, mute: bool) {
        self.0.set_mute(mute);
    }
    fn get_time_ms(&self) -> i64 {
        self.0.get_time_ms()
    }
    fn get_duration_ms(&self) -> i64 {
        self.0.get_duration_ms()
    }
    fn get_status(&self) -> PlaybackStatus {
        self.0.get_status()
    }
    fn get_diagnostics(&self) -> PlayerDiagnostics {
        Default::default()
    }
    fn tick(&mut self) {}
    fn take_error(&mut self) -> Option<String> {
        self.0.take_error()
    }
    fn supports_video(&self) -> bool {
        false
    }
    fn set_osd_subtitle(&mut self, _: &str) {}
    fn get_audio_channel_count(&self) -> Option<usize> {
        self.0.get_audio_channel_count()
    }
    fn get_audio_layout(&self) -> Option<String> {
        None
    }
    fn set_channel_routing(&mut self, channels: &[super::super::task::AudioChannelItem]) {
        self.0
            .set_enabled_channels(channels.iter().map(|channel| channel.playback).collect());
    }
    fn set_audio_only_mode(&mut self, _: bool) {}
}
