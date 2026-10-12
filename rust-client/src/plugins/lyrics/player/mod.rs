use std::time::Instant;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(super) use linux::Player;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::Player;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Track {
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
}

impl Track {
    pub fn is_music(&self) -> bool {
        !self.title.trim().is_empty() && (!self.artists.is_empty() || !self.album.trim().is_empty())
    }
}

#[derive(Clone, Debug)]
pub(super) struct Playback {
    pub track: Track,
    pub embedded_lyrics: Option<String>,
    pub duration_ms: u64,
    pub position_ms: u64,
    pub observed: Instant,
    pub rate: f64,
    pub playing: bool,
}

impl Playback {
    pub fn position(&self) -> u64 {
        let elapsed = if self.playing {
            self.observed.elapsed().as_secs_f64() * self.rate * 1000.0
        } else {
            0.0
        };
        self.position_ms.saturating_add(elapsed.max(0.0) as u64)
    }
}
