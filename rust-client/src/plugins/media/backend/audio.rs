//! Adapt shared local audio playback to the media player's optional video API.
use super::{MediaBackend, PlaybackStatus, PlayerDiagnostics};
use crate::audio_playback::AudioPlayer;
use std::path::PathBuf;

pub struct AudioBackend {
    player: AudioPlayer,
    original: Option<PathBuf>,
    selected: Option<PathBuf>,
}

impl AudioBackend {
    pub fn new() -> Result<Self, String> {
        AudioPlayer::new().map(|player| Self {
            player,
            original: None,
            selected: None,
        })
    }
}

impl MediaBackend for AudioBackend {
    fn refresh_audio(&mut self) -> Result<(), String> {
        if let Some(path) = self.selected.take() {
            self.select_audio(super::PlaybackAudio::External(path))?;
        }
        Ok(())
    }

    fn select_audio(&mut self, selection: super::PlaybackAudio) -> Result<(), String> {
        let path = match selection {
            super::PlaybackAudio::Original(_) => self.original.clone(),
            super::PlaybackAudio::External(path) => Some(path),
        };
        if path == self.selected {
            return Ok(());
        }
        let Some(path) = path else { return Ok(()) };
        let position = self.player.get_time_ms();
        let playing = self.player.get_status() == PlaybackStatus::Playing;
        self.player.load_local_file(path.clone())?;
        self.player.seek(position);
        if playing {
            self.player.play();
        }
        self.selected = Some(path);
        Ok(())
    }

    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String> {
        self.original = Some(path.clone());
        self.selected = Some(path.clone());
        self.player.load_local_file(path)
    }
    fn load_stream_url(&mut self, _: String) -> Result<(), String> {
        Err("Playback is unavailable for this file.".into())
    }
    fn play(&mut self) {
        self.player.play();
    }
    fn pause(&mut self) {
        self.player.pause();
    }
    fn stop(&mut self) {
        self.player.stop();
    }
    fn seek(&mut self, ms: i64) {
        self.player.seek(ms);
    }
    fn set_volume(&mut self, volume: f32) {
        self.player.set_volume(volume);
    }
    fn set_mute(&mut self, mute: bool) {
        self.player.set_mute(mute);
    }
    fn get_time_ms(&self) -> i64 {
        self.player.get_time_ms()
    }
    fn get_duration_ms(&self) -> i64 {
        self.player.get_duration_ms()
    }
    fn get_status(&self) -> PlaybackStatus {
        self.player.get_status()
    }
    fn get_diagnostics(&self) -> PlayerDiagnostics {
        Default::default()
    }
    fn tick(&mut self) {}
    fn take_error(&mut self) -> Option<String> {
        self.player.take_error()
    }
    fn supports_video(&self) -> bool {
        false
    }
    fn set_osd_subtitle(&mut self, _: &str) {}
    fn get_audio_channel_count(&self) -> Option<usize> {
        self.player.get_audio_channel_count()
    }
    fn get_audio_layout(&self) -> Option<String> {
        None
    }
    fn set_channel_routing(&mut self, channels: &[super::super::task::AudioChannelItem]) {
        self.player
            .set_enabled_channels(channels.iter().map(|channel| channel.playback).collect());
    }
    fn set_audio_only_mode(&mut self, _: bool) {}
}
