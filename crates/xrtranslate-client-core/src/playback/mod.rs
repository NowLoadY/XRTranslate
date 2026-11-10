mod channels;
pub mod remote;
mod style;
pub use channels::AudioChannelItem;
pub use style::SubtitleStyle;
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub mod audio;
#[cfg(all(any(windows, target_os = "linux"), feature = "mpv"))]
mod gpu;
#[cfg(all(target_os = "linux", feature = "mpv"))]
pub use gpu::graphics_setup;
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

pub fn create() -> Result<Box<dyn MediaBackend>, String> {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerDiagnostics {
    pub hwdec_current: String,
    pub video_codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub dropped_frames: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackAudio {
    Original(Option<i64>),
    External(PathBuf),
    Mix(Vec<PlaybackAudio>),
    Adjusted {
        source: Box<PlaybackAudio>,
        settings: AudioMixSettings,
    },
}

/// Per-source levels, independent of the player's listening volume. Intervals
/// use the media timeline, so the same envelope also works after seeking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioMixSettings {
    pub volume: u16,
    pub duck_intervals: Vec<[i64; 2]>,
}

impl Default for AudioMixSettings {
    fn default() -> Self {
        Self {
            volume: 100,
            duck_intervals: Vec::new(),
        }
    }
}

impl PlaybackAudio {
    pub(crate) fn sources(&self) -> Vec<&Self> {
        self.inputs()
            .into_iter()
            .map(|(source, _)| source)
            .collect()
    }

    pub(crate) fn inputs(&self) -> Vec<(&Self, Option<&AudioMixSettings>)> {
        match self {
            Self::Mix(sources) => sources.iter().flat_map(Self::inputs).collect(),
            Self::Adjusted { source, settings } => vec![(source, Some(settings))],
            source => vec![(source, None)],
        }
    }

    pub fn adjusted(self, settings: AudioMixSettings) -> Self {
        if settings == AudioMixSettings::default() {
            self
        } else {
            Self::Adjusted {
                source: Box::new(self),
                settings,
            }
        }
    }
}

impl Default for PlaybackAudio {
    fn default() -> Self {
        Self::Original(None)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioTrack {
    pub id: i64,
    pub title: String,
    pub selected: bool,
    pub external: Option<PathBuf>,
}

pub trait MediaBackend: Send {
    fn reopen(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn audio_tracks(&self) -> &[AudioTrack] {
        &[]
    }
    fn select_audio(&mut self, selection: PlaybackAudio) -> Result<(), String> {
        match selection {
            PlaybackAudio::Original(_) => Ok(()),
            _ => Err("Audio track switching is unavailable".into()),
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
    fn set_subtitle_style(&mut self, _style: &SubtitleStyle) {}
    fn get_audio_channel_count(&self) -> Option<usize>;
    #[allow(dead_code)]
    fn get_audio_layout(&self) -> Option<String>;
    fn set_channel_routing(&mut self, channels: &[AudioChannelItem]);
    fn set_audio_only_mode(&mut self, enabled: bool);
}

pub mod installer;
#[cfg(windows)]
static HOST_RUNTIME_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Independent plugins share the host's installed playback runtime.
#[cfg(windows)]
pub fn configure_host_runtime(host_executable: &std::path::Path) {
    if let Some(directory) = host_executable.parent() {
        let _ = HOST_RUNTIME_DIRECTORY.set(directory.to_owned());
        configure_dll_search_paths();
    }
}

#[cfg(windows)]
pub fn configure_dll_search_paths() {
    use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
    use windows::core::HSTRING;
    for directory in runtime_bin_directories() {
        if ["mpv-2.dll", "libmpv-2.dll"]
            .iter()
            .any(|name| directory.join(name).is_file())
        {
            let directory = directory.canonicalize().unwrap_or(directory);
            let _ = unsafe { SetDllDirectoryW(&HSTRING::from(directory.as_os_str())) };
            return;
        }
    }
}

/// One writable destination, also shared by independently built media plugins.
pub fn runtime_install_directory() -> PathBuf {
    #[cfg(windows)]
    let host = HOST_RUNTIME_DIRECTORY.get().cloned();
    #[cfg(not(windows))]
    let host: Option<PathBuf> = None;
    let root = host
        .or_else(|| std::env::current_exe().ok()?.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let data = xrtranslate_config::data_root(&root);
    if data != root {
        data.join("runtime/playback")
    } else {
        root.join("resources/bin")
    }
}

pub fn runtime_bin_directories() -> Vec<PathBuf> {
    let mut dirs = vec![runtime_install_directory()];
    #[cfg(windows)]
    if let Some(directory) = HOST_RUNTIME_DIRECTORY.get() {
        dirs.push(directory.join("resources/bin"));
        dirs.push(directory.clone());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("resources/bin"));
        }
    }
    if let Ok(root) = std::env::current_dir() {
        dirs.push(root.join("resources/bin"));
    }
    dirs.push(PathBuf::from("rust-client/resources/bin"));
    dirs
}
