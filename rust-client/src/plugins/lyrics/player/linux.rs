// MPRIS synchronization follows desktop-lyric/src/mpris.js and extension.js.
// SPDX-FileCopyrightText: tuberry
// SPDX-FileCopyrightText: NowLoadY
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{Playback, Track};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use zbus::{Connection, Proxy, zvariant::OwnedValue};

type Properties = HashMap<String, OwnedValue>;

pub(in crate::plugins::lyrics) struct Player {
    connection: Connection,
    selected: String,
}

impl Player {
    pub async fn new() -> Result<Self, String> {
        let connection = zbus::connection::Builder::session()
            .map_err(|error| error.to_string())?
            .method_timeout(Duration::from_millis(750))
            .build()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            connection,
            selected: String::new(),
        })
    }

    pub async fn poll(&mut self) -> Result<Option<Playback>, String> {
        let bus = zbus::fdo::DBusProxy::new(&self.connection)
            .await
            .map_err(|error| error.to_string())?;
        let mut names = bus
            .list_names()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|name| name.to_string())
            .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
            .collect::<Vec<_>>();
        names.sort_by_key(|name| (name != &self.selected, name.clone()));
        let mut paused = None;
        for name in names {
            let Ok(playback) = self.snapshot(&name).await else {
                continue;
            };
            if !playback.track.is_music() {
                continue;
            }
            if playback.playing {
                self.selected = name;
                return Ok(Some(playback));
            }
            if paused.is_none() {
                paused = Some(playback);
            }
        }
        Ok(paused)
    }

    async fn snapshot(&self, name: &str) -> Result<Playback, zbus::Error> {
        let proxy = Proxy::new(
            &self.connection,
            name,
            "/org/mpris/MediaPlayer2",
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        let mut properties: Properties = proxy
            .call("GetAll", &("org.mpris.MediaPlayer2.Player",))
            .await?;
        let mut metadata = properties
            .remove("Metadata")
            .and_then(|value| Properties::try_from(value).ok())
            .unwrap_or_default();
        let track = Track {
            title: string(&metadata, "xesam:title"),
            album: string(&metadata, "xesam:album"),
            artists: metadata
                .remove("xesam:artist")
                .and_then(|value| Vec::<String>::try_from(value).ok())
                .unwrap_or_default()
                .into_iter()
                .flat_map(|artist| {
                    artist
                        .split('/')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .collect(),
        };
        let position = properties
            .get("Position")
            .and_then(|value| i64::try_from(value).ok());
        // Some MPRIS implementations omit Position from GetAll.
        let position = match position {
            Some(value) => value,
            None => {
                let value: OwnedValue = proxy
                    .call("Get", &("org.mpris.MediaPlayer2.Player", "Position"))
                    .await?;
                i64::try_from(value).unwrap_or(0)
            }
        };
        let rate = properties
            .get("Rate")
            .and_then(|value| f64::try_from(value).ok())
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .unwrap_or(1.0);
        let lyrics = string(&metadata, "xesam:asText");
        Ok(Playback {
            track,
            embedded_lyrics: (!lyrics.is_empty() && lyrics.len() <= 256 * 1024).then_some(lyrics),
            duration_ms: metadata
                .get("mpris:length")
                .and_then(|value| i64::try_from(value).ok())
                .unwrap_or(0)
                .max(0) as u64
                / 1000,
            position_ms: position.max(0) as u64 / 1000,
            observed: Instant::now(),
            rate,
            playing: string(&properties, "PlaybackStatus") == "Playing",
        })
    }
}

fn string(values: &Properties, key: &str) -> String {
    values
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok())
        .unwrap_or_default()
        .to_owned()
}
