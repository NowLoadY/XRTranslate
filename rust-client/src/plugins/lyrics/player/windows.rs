use super::{Playback, Track};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
};

pub(in crate::plugins::lyrics) struct Player {
    manager: Manager,
    _apartment: Apartment,
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

impl Player {
    pub async fn new() -> Result<Self, String> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|error| error.to_string())?;
        let apartment = Apartment;
        let manager = Manager::RequestAsync()
            .map_err(|error| error.to_string())?
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            manager,
            _apartment: apartment,
        })
    }

    pub async fn poll(&mut self) -> Result<Option<Playback>, String> {
        let mut paused = None;
        if let Ok(session) = self.manager.GetCurrentSession()
            && let Ok(playback) = snapshot(&session).await
            && playback.track.is_music()
        {
            if playback.playing {
                return Ok(Some(playback));
            }
            paused = Some(playback);
        }
        let sessions = self
            .manager
            .GetSessions()
            .map_err(|error| error.to_string())?;
        for session in sessions {
            if let Ok(playback) = snapshot(&session).await
                && playback.playing
                && playback.track.is_music()
            {
                return Ok(Some(playback));
            }
        }
        Ok(paused)
    }
}

async fn snapshot(session: &Session) -> windows::core::Result<Playback> {
    let media = session.TryGetMediaPropertiesAsync()?.await?;
    let info = session.GetPlaybackInfo()?;
    let timeline = session.GetTimelineProperties()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let rate = info
        .PlaybackRate()
        .and_then(|value| value.Value())
        .ok()
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .unwrap_or(1.0);
    let start = timeline.StartTime()?.Duration;
    let duration_ms = (timeline.EndTime()?.Duration - start).max(0) as u64 / 10_000;
    let mut position_ms = (timeline.Position()?.Duration - start).max(0) as u64 / 10_000;
    // WinRT positions are anchored to LastUpdatedTime, not to the read time.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let updated = timeline.LastUpdatedTime()?.UniversalTime / 10_000 - 11_644_473_600_000;
    if playing && updated > 0 {
        position_ms = position_ms.saturating_add(((now - updated).max(0) as f64 * rate) as u64);
    }
    let artist = media.Artist()?.to_string();
    Ok(Playback {
        track: Track {
            title: media.Title()?.to_string(),
            artists: artist
                .split('/')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect(),
            album: media.AlbumTitle()?.to_string(),
        },
        embedded_lyrics: None,
        duration_ms,
        position_ms,
        observed: Instant::now(),
        rate,
        playing,
    })
}
