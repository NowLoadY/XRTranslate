use super::{Playback, Track};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Media::MediaPlaybackType,
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
            && !playback.track.title.trim().is_empty()
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
                && !playback.track.title.trim().is_empty()
            {
                if playback.playing {
                    return Ok(Some(playback));
                }
                if paused.is_none() {
                    paused = Some(playback);
                }
            }
        }
        Ok(paused)
    }
}

async fn snapshot(session: &Session) -> windows::core::Result<Playback> {
    let media = session.TryGetMediaPropertiesAsync()?.await?;
    let info = session.GetPlaybackInfo()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let rate = info
        .PlaybackRate()
        .and_then(|value| value.Value())
        .ok()
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .unwrap_or(1.0);
    // Metadata remains useful when the player omits or cannot return a timeline.
    let timing = (|| -> windows::core::Result<_> {
        let timeline = session.GetTimelineProperties()?;
        let start = timeline.StartTime()?.Duration;
        let end = timeline.EndTime()?.Duration;
        let position = timeline.Position()?.Duration;
        let updated = timeline.LastUpdatedTime()?.UniversalTime / 10_000 - 11_644_473_600_000;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        Ok((
            end.saturating_sub(start).max(0) as u64 / 10_000,
            timeline_position(
                start,
                end,
                position,
                updated,
                now,
                if playing { rate } else { 0.0 },
            ),
        ))
    })();
    let (duration_ms, position_ms) = timing.unwrap_or((0, None));
    let artist = media.Artist()?.to_string();
    let track = Track {
        title: media.Title()?.to_string(),
        artists: artist
            .split('/')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        album: media.AlbumTitle()?.to_string(),
    };
    // Browsers can label a plain video title as Music. Require structured song
    // metadata as well, and always honor an explicit non-music media type.
    let is_music = track.has_music_metadata()
        && !matches!(
            media.PlaybackType().and_then(|kind| kind.Value()).ok(),
            Some(MediaPlaybackType::Video | MediaPlaybackType::Image)
        );
    Ok(Playback {
        track,
        is_music,
        embedded_lyrics: None,
        duration_ms,
        position_ms,
        observed: Instant::now(),
        rate,
        playing,
    })
}

fn timeline_position(
    start: i64,
    end: i64,
    position: i64,
    updated: i64,
    now: i64,
    rate: f64,
) -> Option<u64> {
    // All-zero SMTC timelines (including LastUpdatedTime = 1601) are common.
    // Never turn them into a zero-second anchor that restarts on every poll.
    if end <= start || position < start || position > end || updated <= 0 || updated > now {
        return None;
    }
    let duration = end.saturating_sub(start) as u64 / 10_000;
    let position = position.saturating_sub(start) as u64 / 10_000;
    // WinRT positions are anchored to LastUpdatedTime, not to the read time.
    let elapsed = (now.saturating_sub(updated) as f64 * rate) as u64;
    Some(position.saturating_add(elapsed).min(duration))
}
