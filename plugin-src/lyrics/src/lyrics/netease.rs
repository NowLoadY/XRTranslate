// Matching adapted from desktop-lyric/src/lyric.js.
// SPDX-FileCopyrightText: tuberry
// SPDX-License-Identifier: GPL-3.0-or-later

use super::player::Track;
use serde::Deserialize;

const ORIGIN: &str = "https://music.163.com";
const RESPONSE_LIMIT: usize = 256 * 1024;

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Song {
    id: u64,
    name: String,
    #[serde(default)]
    duration: u64,
    album: Named,
    artists: Vec<Named>,
}

#[derive(Deserialize)]
struct Search {
    result: SearchResult,
}

#[derive(Deserialize)]
struct SearchResult {
    #[serde(default)]
    songs: Vec<Song>,
}

#[derive(Deserialize)]
struct Lyrics {
    lrc: Option<LyricText>,
}

#[derive(Deserialize)]
struct LyricText {
    #[serde(default)]
    lyric: String,
}

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("XRTranslate/lyrics (https://github.com/NowLoadY/XRTranslate)")
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())
}

pub(super) async fn fetch(
    client: &reqwest::Client,
    track: &Track,
    duration_ms: u64,
) -> Result<String, String> {
    let term = format!("{} {}", track.title, track.artists.join(" "));
    let response = client
        .get(url(
            "/api/search/get/web",
            &[("s", term.as_str()), ("limit", "30"), ("type", "1")],
        )?)
        .header(reqwest::header::REFERER, ORIGIN);
    let search: Search = json(response).await?;
    let title = normalize(&track.title);
    let album = normalize(&track.album);
    let artists = track
        .artists
        .iter()
        .map(|name| normalize(name))
        .collect::<Vec<_>>();
    let song = search
        .result
        .songs
        .into_iter()
        .filter(|song| normalize(&song.name) == title)
        .filter(|song| {
            artists.is_empty()
                || song
                    .artists
                    .iter()
                    .any(|artist| artists.contains(&normalize(&artist.name)))
        })
        .min_by_key(|song| {
            (
                !album.is_empty() && normalize(&song.album.name) != album,
                if duration_ms > 0 {
                    song.duration.abs_diff(duration_ms)
                } else {
                    0
                },
            )
        });
    let Some(song) = song else {
        return Ok(String::new());
    };
    // Do not take the first unrelated search hit when metadata cannot match.
    if duration_ms > 0 && song.duration > 0 && song.duration.abs_diff(duration_ms) > 15_000 {
        return Ok(String::new());
    }
    let lyrics: Lyrics = json(
        client
            .get(url(
                "/api/song/lyric",
                &[("id", &song.id.to_string()), ("lv", "0")],
            )?)
            .header(reqwest::header::REFERER, ORIGIN),
    )
    .await?;
    Ok(lyrics.lrc.map(|lrc| lrc.lyric).unwrap_or_default())
}

fn url(path: &str, params: &[(&str, &str)]) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse(&format!("{ORIGIN}{path}")).map_err(|error| error.to_string())?;
    url.query_pairs_mut().extend_pairs(params.iter().copied());
    Ok(url)
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

async fn json<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, String> {
    let mut response = request
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            return Err("Lyric response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}
