#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub mod audio;
#[cfg(all(windows, feature = "mpv"))]
mod gpu;
#[cfg(all(target_os = "linux", feature = "mpv"))]
#[path = "gpu_linux.rs"]
mod gpu;
#[cfg(feature = "mpv")]
pub mod mpv;
#[cfg(not(feature = "mpv"))]
#[path = "mpv_stub.rs"]
pub mod mpv;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaSource {
    LocalFile(PathBuf),
    NetworkStream(String),
    OnlineFile(String),
}

pub use crate::audio_playback::PlaybackStatus;

pub(super) fn create() -> Result<Box<dyn MediaBackend>, String> {
    match mpv::MpvBackend::new() {
        Ok(player) => Ok(Box::new(player)),
        Err(error) => {
            #[cfg(any(windows, target_os = "linux", target_os = "android"))]
            {
                log::debug!("Using native audio playback: {error}");
                audio::AudioBackend::new().map(|player| Box::new(player) as Box<dyn MediaBackend>)
            }
            #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
            Err(error)
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(not(all(target_os = "linux", feature = "mpv")), allow(dead_code))]
pub enum VideoDisplay {
    X11(usize),
    Wayland(usize),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackAudio {
    Original(Option<i64>),
    External(PathBuf),
}

impl Default for PlaybackAudio {
    fn default() -> Self {
        Self::Original(None)
    }
}

#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub id: i64,
    pub title: String,
    pub selected: bool,
    pub external: Option<PathBuf>,
}

pub trait MediaBackend: Send {
    fn audio_tracks(&self) -> &[AudioTrack] {
        &[]
    }
    fn select_audio(&mut self, selection: PlaybackAudio) -> Result<(), String> {
        match selection {
            PlaybackAudio::Original(_) => Ok(()),
            PlaybackAudio::External(_) => Err("Audio track switching is unavailable".into()),
        }
    }

    fn refresh_audio(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn supports_video(&self) -> bool {
        true
    }
    fn take_error(&mut self) -> Option<String> {
        None
    }
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
    fn initialize_video(
        &mut self,
        _state: &eframe::egui_wgpu::RenderState,
        _ctx: &eframe::egui::Context,
        _display: Option<VideoDisplay>,
    ) -> Result<(), String> {
        Err("GPU video playback is unavailable on this platform".into())
    }
    fn video_frame(&mut self, _size: [u32; 2]) -> Result<Option<eframe::egui::TextureId>, String> {
        Ok(None)
    }
    fn set_video_visible(&mut self, _visible: bool) {}
    fn set_osd_subtitle(&mut self, text: &str);
    fn set_subtitle_style(&mut self, _style: &super::subtitles::SubtitleStyle) {}
    fn get_audio_channel_count(&self) -> Option<usize>;
    #[allow(dead_code)]
    fn get_audio_layout(&self) -> Option<String>;
    fn set_channel_routing(&mut self, channels: &[super::task::AudioChannelItem]);
    fn set_audio_only_mode(&mut self, enabled: bool);
}
