//! Playback event names and log decoding are documented by
//! Coooookies/netease-cloudmusic-detector (MIT; see the retained attribution).
use super::Track;
use serde_json::Value;

#[derive(Default)]
pub(super) struct State {
    track: Option<LoggedTrack>,
    clock: Option<Clock>,
    playing: Option<bool>,
    play_id: Option<String>,
}

struct LoggedTrack {
    pid: u32,
    id: String,
    title: String,
    artists: Vec<String>,
    duration_ms: u64,
}

struct Clock {
    position_ms: u64,
    at: u64,
    started: u64,
}

pub(super) struct Sample {
    pub pid: u32,
    pub started: u64,
    pub position_ms: u64,
    pub duration_ms: u64,
}

impl State {
    pub fn push(&mut self, line: &str) {
        let Some((header, mut payload)) = line.split_once("【playing】,") else {
            return;
        };
        let mut fields = header.strip_prefix('[').unwrap_or_default().split(':');
        let Some(pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            return;
        };
        let Some(at) = fields.nth(2).and_then(|value| value.parse::<u64>().ok()) else {
            return;
        };
        let Some(Value::String(name)) = argument(&mut payload) else {
            return;
        };
        if !matches!(
            name.as_str(),
            "playOneTrackInPlayingList"
                | "checkPlayPrivilege"
                | "native播放资源load完成，开始播放"
                | "native播放state"
                | "setPlayingPosition"
        ) {
            return;
        }
        let Some(value) = argument(&mut payload) else {
            return;
        };
        if matches!(
            name.as_str(),
            "playOneTrackInPlayingList" | "checkPlayPrivilege"
        ) {
            let value = value.get("track").unwrap_or(&value);
            let Some(track) = logged_track(pid, value) else {
                return;
            };
            if self
                .track
                .as_ref()
                .is_none_or(|old| old.pid != pid || old.id != track.id)
            {
                self.clock = None;
                self.playing = None;
                self.play_id = None;
            }
            self.track = Some(track);
            return;
        }
        let Some(track) = &self.track else { return };
        if track.pid != pid || self.clock.as_ref().is_some_and(|clock| at < clock.at) {
            return;
        }
        match name.as_str() {
            "native播放资源load完成，开始播放"
                if id(&value["songId"]).as_deref() == Some(&track.id) =>
            {
                self.clock = Some(Clock {
                    position_ms: 0,
                    at,
                    started: at,
                });
                self.playing = Some(true);
                self.play_id = None;
            }
            "native播放state" => {
                let Some(Value::String(play_id)) = argument(&mut payload) else {
                    return;
                };
                if play_id.split('_').next() != Some(track.id.as_str())
                    || self
                        .play_id
                        .as_ref()
                        .is_some_and(|current| current != &play_id)
                {
                    return;
                }
                let playing = match value.as_u64() {
                    Some(1) => true,
                    Some(2) => false,
                    _ => {
                        self.clock = None;
                        self.playing = None;
                        return;
                    }
                };
                if let Some(clock) = &mut self.clock {
                    if self.playing == Some(true) {
                        clock.position_ms = clock.position_ms.saturating_add(at - clock.at);
                    }
                    clock.at = at;
                }
                self.playing = Some(playing);
                self.play_id = Some(play_id);
            }
            "setPlayingPosition" => {
                let Some(position_ms) = milliseconds(&value) else {
                    return;
                };
                if position_ms <= track.duration_ms && self.playing.is_some() {
                    self.clock = Some(Clock {
                        position_ms,
                        at,
                        started: at,
                    });
                }
            }
            _ => {}
        }
    }

    pub fn sample(&self, track: &Track, playing: bool, now: u64) -> Option<Sample> {
        let logged = self.track.as_ref()?;
        let clock = self.clock.as_ref()?;
        if self.playing != Some(playing)
            || logged.title.trim() != track.title.trim()
            || track.artists.is_empty()
            || !track.artists.iter().all(|artist| {
                logged
                    .artists
                    .iter()
                    .any(|name| name.trim() == artist.trim())
            })
        {
            return None;
        }
        let elapsed = now.checked_sub(clock.at)?;
        let position_ms = clock
            .position_ms
            .saturating_add(if playing { elapsed } else { 0 });
        // An old log must not display the final cue forever after a missed event.
        if position_ms > logged.duration_ms.saturating_add(2000) {
            return None;
        }
        Some(Sample {
            pid: logged.pid,
            started: clock.started,
            position_ms: position_ms.min(logged.duration_ms),
            duration_ms: logged.duration_ms,
        })
    }
}

fn logged_track(pid: u32, value: &Value) -> Option<LoggedTrack> {
    let duration_ms = value["duration"]
        .as_u64()
        .filter(|duration| *duration > 0)?;
    Some(LoggedTrack {
        pid,
        id: id(&value["id"])?,
        title: value["name"].as_str()?.to_owned(),
        artists: value["artists"]
            .as_array()?
            .iter()
            .filter_map(|artist| artist["name"].as_str().map(str::to_owned))
            .collect(),
        duration_ms,
    })
}

fn id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn milliseconds(value: &Value) -> Option<u64> {
    let seconds = value.as_f64()?;
    (seconds.is_finite() && seconds >= 0.0 && seconds < u64::MAX as f64 / 1000.0)
        .then(|| (seconds * 1000.0).round() as u64)
}

fn argument(input: &mut &str) -> Option<Value> {
    let mut stream = serde_json::Deserializer::from_str(input.trim_start_matches([',', ' ']))
        .into_iter::<Value>();
    let value = stream.next()?.ok()?;
    *input = &input.trim_start_matches([',', ' '])[stream.byte_offset()..];
    Some(value)
}
