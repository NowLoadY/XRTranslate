//! The player retains decoded audio/video in the shell; plugins own their controls.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use xrtranslate_plugin::Client;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Open,
    Close,
    Load { source: MediaSource },
    Play,
    Pause,
    Stop,
    Seek { ms: i64 },
    Volume { value: f32 },
    Mute { value: bool },
    Audio { selection: PlaybackAudio },
    RefreshAudio,
    Video { size: [u32; 2] },
    Visible { value: bool },
    Subtitles { text: String },
    Style { style: SubtitleStyle },
    Channels { channels: Vec<AudioChannelItem> },
    AudioOnly { value: bool },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub time_ms: i64,
    pub duration_ms: i64,
    pub status: PlaybackStatus,
    pub diagnostics: PlayerDiagnostics,
    pub tracks: Vec<AudioTrack>,
    pub channels: Option<usize>,
    pub layout: Option<String>,
    pub video: bool,
    pub error: Option<String>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            time_ms: 0,
            duration_ms: 0,
            status: PlaybackStatus::Stopped,
            diagnostics: Default::default(),
            tracks: Vec::new(),
            channels: None,
            layout: None,
            video: false,
            error: None,
        }
    }
}
pub type Snapshots = Arc<Mutex<BTreeMap<u64, Snapshot>>>;
pub struct Player {
    client: Client,
    id: u64,
    snapshots: Snapshots,
    snapshot: Snapshot,
    size: [u32; 2],
    visible: bool,
    subtitle: String,
    style: Option<SubtitleStyle>,
    routing: Vec<AudioChannelItem>,
}
impl Player {
    pub fn new(client: Client, id: u64, snapshots: Snapshots) -> Result<Self, String> {
        client.event("playback", json!({"id":id,"command":Command::Open}))?;
        Ok(Self {
            client,
            id,
            snapshots,
            snapshot: Default::default(),
            size: [0; 2],
            visible: false,
            subtitle: String::new(),
            style: None,
            routing: Vec::new(),
        })
    }
    fn send(&mut self, command: Command) {
        if let Err(error) = self
            .client
            .event("playback", json!({"id":self.id,"command":command}))
        {
            self.snapshot.error = Some(error)
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.send(Command::Close);
        self.snapshots.lock().unwrap().remove(&self.id);
    }
}
impl MediaBackend for Player {
    fn reopen(&mut self) -> Result<(), String> {
        self.send(Command::Close);
        self.send(Command::Open);
        self.size = [0; 2];
        self.visible = false;
        self.style = None;
        self.subtitle.clear();
        self.routing.clear();
        self.snapshot = Default::default();
        Ok(())
    }
    fn audio_tracks(&self) -> &[AudioTrack] {
        &self.snapshot.tracks
    }
    fn select_audio(&mut self, selection: PlaybackAudio) -> Result<(), String> {
        self.send(Command::Audio { selection });
        Ok(())
    }
    fn refresh_audio(&mut self) -> Result<(), String> {
        self.send(Command::RefreshAudio);
        Ok(())
    }
    fn supports_video(&self) -> bool {
        self.snapshot.video
    }
    fn take_error(&mut self) -> Option<String> {
        self.snapshot.error.take()
    }
    fn load_local_file(&mut self, path: PathBuf) -> Result<(), String> {
        self.send(Command::Load {
            source: MediaSource::LocalFile(path),
        });
        Ok(())
    }
    fn load_stream_url(&mut self, url: String) -> Result<(), String> {
        self.send(Command::Load {
            source: MediaSource::NetworkStream(url),
        });
        Ok(())
    }
    fn play(&mut self) {
        self.send(Command::Play);
        self.snapshot.status = PlaybackStatus::Playing
    }
    fn pause(&mut self) {
        self.send(Command::Pause);
        self.snapshot.status = PlaybackStatus::Paused
    }
    fn stop(&mut self) {
        self.send(Command::Stop);
        self.snapshot.status = PlaybackStatus::Stopped
    }
    fn seek(&mut self, ms: i64) {
        self.send(Command::Seek { ms });
        self.snapshot.time_ms = ms
    }
    fn set_volume(&mut self, value: f32) {
        self.send(Command::Volume { value })
    }
    fn set_mute(&mut self, value: bool) {
        self.send(Command::Mute { value })
    }
    fn get_time_ms(&self) -> i64 {
        self.snapshot.time_ms
    }
    fn get_duration_ms(&self) -> i64 {
        self.snapshot.duration_ms
    }
    fn get_status(&self) -> PlaybackStatus {
        self.snapshot.status
    }
    fn get_diagnostics(&self) -> PlayerDiagnostics {
        self.snapshot.diagnostics.clone()
    }
    fn tick(&mut self) {
        if let Some(snapshot) = self.snapshots.lock().unwrap().remove(&self.id) {
            self.snapshot = snapshot
        }
    }
    fn video_frame(&mut self, size: [u32; 2]) -> Result<Option<eframe::egui::TextureId>, String> {
        if self.size != size {
            self.send(Command::Video { size });
            self.size = size;
        }
        self.set_video_visible(true);
        Ok(self
            .snapshot
            .video
            .then_some(eframe::egui::TextureId::User(self.id)))
    }
    fn set_video_visible(&mut self, value: bool) {
        if self.visible != value {
            self.visible = value;
            self.send(Command::Visible { value })
        }
    }
    fn set_osd_subtitle(&mut self, text: &str) {
        if self.subtitle != text {
            self.subtitle = text.into();
            self.send(Command::Subtitles { text: text.into() })
        }
    }
    fn set_subtitle_style(&mut self, style: &SubtitleStyle) {
        if self.style.as_ref() != Some(style) {
            self.style = Some(style.clone());
            self.send(Command::Style {
                style: style.clone(),
            })
        }
    }
    fn get_audio_channel_count(&self) -> Option<usize> {
        self.snapshot.channels
    }
    fn get_audio_layout(&self) -> Option<String> {
        self.snapshot.layout.clone()
    }
    fn set_channel_routing(&mut self, channels: &[AudioChannelItem]) {
        if self.routing != channels {
            self.routing = channels.into();
            self.send(Command::Channels {
                channels: channels.into(),
            })
        }
    }
    fn set_audio_only_mode(&mut self, value: bool) {
        self.send(Command::AudioOnly { value })
    }
}
