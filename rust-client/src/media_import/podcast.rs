//! Public podcast links resolve to ordinary audio URLs before recognition starts.
use std::sync::{Arc, atomic::AtomicBool};

use reqwest::Url;
use serde_json::Value;

use super::http::{HttpClient, http_url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PodcastEpisode {
    pub title: String,
    pub audio_url: String,
    pub kind: AudioKind,
    pub video: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioKind {
    Recorded,
    Live,
    Pending,
    Ended,
    Unknown,
}

/// A feed is a selection of episodes, never an implicit download of its first item.
pub(crate) fn resolve(
    input: &str,
    proxy: &str,
    stop: Arc<AtomicBool>,
) -> Result<Vec<PodcastEpisode>, String> {
    let url = http_url(input)?;
    let client = HttpClient::new(proxy, stop)?;
    if url.host_str() == Some("podcasts.apple.com") {
        return resolve_apple(&client, &url);
    }
    let response = client.get(&url, None)?;
    let url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("mpegurl") || url.path().ends_with(".m3u8") {
        let document = client.document(response)?;
        let kind = hls_kind(&client, &url, &document, 0)?;
        return Ok(vec![PodcastEpisode {
            title: url
                .path()
                .rsplit('/')
                .next()
                .unwrap_or("Online audio")
                .to_owned(),
            audio_url: url.into(),
            kind,
            video: true,
        }]);
    }
    if content_type.starts_with("audio/")
        || content_type.starts_with("video/")
        || matches!(
            url.path().rsplit('.').next(),
            Some("mp3" | "m4a" | "aac" | "ogg" | "flac" | "wav" | "mp4" | "webm" | "mkv")
        ) && !content_type.contains("html")
    {
        return Ok(vec![PodcastEpisode {
            title: url
                .path_segments()
                .and_then(|mut parts| parts.next_back())
                .filter(|name| !name.is_empty())
                .unwrap_or("Online audio")
                .to_owned(),
            audio_url: url.into(),
            kind: if ["icy-name", "icy-br", "icy-description"]
                .iter()
                .any(|key| response.headers().contains_key(*key))
            {
                AudioKind::Live
            } else if response.content_length().is_some() {
                AudioKind::Recorded
            } else {
                AudioKind::Unknown
            },
            video: content_type.starts_with("video/"),
        }]);
    }
    let document = client.document(response)?;
    let episodes = if matches!(
        url.host_str(),
        Some("www.xiaoyuzhoufm.com" | "xiaoyuzhoufm.com")
    ) {
        parse_xiaoyuzhou(&document)?
    } else {
        parse_feed(&document, &url)?
    };
    if episodes.is_empty() {
        return Err(
            "No public podcast audio found. Use an episode link, RSS feed, or direct audio URL"
                .into(),
        );
    }
    Ok(episodes)
}

fn hls_kind(
    client: &HttpClient,
    base: &Url,
    document: &str,
    depth: usize,
) -> Result<AudioKind, String> {
    if depth > 3 || !document.trim_start().starts_with("#EXTM3U") {
        return Err("Invalid HLS playlist".into());
    }
    if document.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
        return Ok(AudioKind::Recorded);
    }
    if document.contains("#EXT-X-TARGETDURATION:") {
        return Ok(AudioKind::Live);
    }
    let link = document
        .lines()
        .find_map(|line| {
            if line.starts_with("#EXT-X-MEDIA:") && line.contains("TYPE=AUDIO") {
                line.split("URI=\"")
                    .nth(1)
                    .and_then(|value| value.split('"').next())
            } else {
                None
            }
        })
        .or_else(|| {
            document
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        })
        .ok_or("HLS playlist has no audio stream")?;
    let response = client.get(&base.join(link).map_err(|e| e.to_string())?, None)?;
    let base = response.url().clone();
    hls_kind(client, &base, &client.document(response)?, depth + 1)
}

fn resolve_apple(client: &HttpClient, url: &Url) -> Result<Vec<PodcastEpisode>, String> {
    let episode_id = url
        .query_pairs()
        .find(|(key, _)| key == "i")
        .map(|(_, id)| id.into_owned());
    let id = url
        .path_segments()
        .and_then(|mut parts| parts.find_map(|part| part.strip_prefix("id")))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("Apple Podcasts link has no show or episode ID")?;
    let mut lookup = Url::parse("https://itunes.apple.com/lookup").unwrap();
    lookup
        .query_pairs_mut()
        .append_pair("id", id)
        .append_pair("entity", "podcastEpisode")
        .append_pair("limit", "200");
    if let Some(country) = url
        .path_segments()
        .and_then(|mut parts| parts.next())
        .filter(|country| country.len() == 2 && country.bytes().all(|b| b.is_ascii_alphabetic()))
    {
        lookup.query_pairs_mut().append_pair("country", country);
    }
    let data: Value = serde_json::from_str(&client.document(client.get(&lookup, None)?)?)
        .map_err(|e| format!("Invalid Apple Podcasts response: {e}"))?;
    let results = data["results"]
        .as_array()
        .ok_or("Apple Podcasts returned no results")?;
    let episodes = results
        .iter()
        .filter(|entry| {
            episode_id.as_ref().is_none_or(|id| {
                entry["trackId"]
                    .as_u64()
                    .is_some_and(|value| value.to_string() == *id)
            })
        })
        .filter_map(|entry| episode(entry["trackName"].as_str(), entry["episodeUrl"].as_str()))
        .collect::<Vec<_>>();
    if !episodes.is_empty() {
        return Ok(episodes);
    }
    // An episode URL must never fall back to a different episode from the show.
    if episode_id.is_some() {
        return Err("This Apple Podcasts episode has no public audio URL".into());
    }
    let feed = results
        .iter()
        .find_map(|entry| entry["feedUrl"].as_str())
        .ok_or("This Apple Podcasts show has no public RSS feed")?;
    let response = client.get(&http_url(feed)?, None)?;
    let base = response.url().clone();
    let episodes = parse_feed(&client.document(response)?, &base)?;
    if episodes.is_empty() {
        return Err("No public episodes found in the podcast feed".into());
    }
    Ok(episodes)
}

fn episode(title: Option<&str>, url: Option<&str>) -> Option<PodcastEpisode> {
    Some(PodcastEpisode {
        title: title
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Untitled episode")
            .to_owned(),
        audio_url: http_url(url?).ok()?.into(),
        kind: AudioKind::Recorded,
        video: false,
    })
}

fn parse_feed(document: &str, base: &Url) -> Result<Vec<PodcastEpisode>, String> {
    let xml = roxmltree::Document::parse(document)
        .map_err(|_| "This link is not a supported podcast page or RSS feed".to_owned())?;
    let mut episodes: Vec<PodcastEpisode> = Vec::new();
    for item in xml.descendants().filter(|node| {
        node.has_tag_name("item")
            || node.tag_name().name() == "entry"
            || node.has_tag_name(("https://podcastindex.org/namespace/1.0", "liveItem"))
    }) {
        let kind = if item.tag_name().name() == "liveItem" {
            match item.attribute("status") {
                Some("live") => AudioKind::Live,
                Some("pending") => AudioKind::Pending,
                Some("ended") => AudioKind::Ended,
                _ => continue,
            }
        } else {
            AudioKind::Recorded
        };
        let same_kind = episodes
            .iter()
            .filter(|episode| {
                (episode.kind == AudioKind::Recorded) == (kind == AudioKind::Recorded)
            })
            .count();
        if same_kind >= if kind == AudioKind::Recorded { 200 } else { 32 } {
            continue;
        }
        let title = item
            .children()
            .find(|node| node.tag_name().name() == "title")
            .and_then(|node| node.text());
        let enclosure = item.children().find(|node| {
            (node.tag_name().name() == "enclosure"
                || node.tag_name().name() == "link" && node.attribute("rel") == Some("enclosure"))
                && node.attribute("type").is_none_or(audio_mime)
        });
        let link = enclosure
            .and_then(|node| node.attribute("url").or_else(|| node.attribute("href")))
            .or_else(|| {
                item.children()
                    .filter(|node| {
                        node.has_tag_name((
                            "https://podcastindex.org/namespace/1.0",
                            "alternateEnclosure",
                        )) && node.attribute("type").is_some_and(audio_mime)
                    })
                    .flat_map(|node| node.children())
                    .find(|node| {
                        node.has_tag_name(("https://podcastindex.org/namespace/1.0", "source"))
                    })
                    .and_then(|node| node.attribute("uri"))
            });
        if let Some(link) = link
            && let Ok(url) = base.join(link)
            && let Some(mut episode) = episode(title, Some(url.as_str()))
            && !episodes.iter().any(|existing: &PodcastEpisode| {
                existing.audio_url == episode.audio_url && existing.kind == kind
            })
        {
            episode.kind = kind;
            episode.video = enclosure
                .and_then(|node| node.attribute("type"))
                .is_some_and(|mime| mime.starts_with("video/"));
            episodes.push(episode);
        }
    }
    Ok(episodes)
}

fn audio_mime(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("audio/")
        || mime.starts_with("video/")
        || matches!(
            mime.as_str(),
            "application/octet-stream" | "application/vnd.apple.mpegurl" | "application/x-mpegurl"
        )
}

fn parse_xiaoyuzhou(document: &str) -> Result<Vec<PodcastEpisode>, String> {
    // Read the page's serialized public episode data, not comments or recommendations.
    let marker = document
        .find("id=\"__NEXT_DATA__\"")
        .ok_or("Podcast page has no public episode data")?;
    let script = &document[marker..];
    let start = script.find('>').ok_or("Invalid podcast page data")? + 1;
    let end = script[start..]
        .find("</script>")
        .ok_or("Invalid podcast page data")?
        + start;
    let data: Value = serde_json::from_str(&script[start..end]).map_err(|e| e.to_string())?;
    let props = &data["props"]["pageProps"];
    let entries = if props["episode"].is_object() {
        vec![&props["episode"]]
    } else {
        props["podcast"]["episodes"]
            .as_array()
            .map(|entries| entries.iter().collect())
            .unwrap_or_default()
    };
    Ok(entries
        .into_iter()
        .filter(|entry| {
            entry["payType"].as_str().is_none_or(|kind| kind == "FREE")
                && entry["media"]["source"]["mode"]
                    .as_str()
                    .is_none_or(|mode| mode == "PUBLIC")
        })
        .filter_map(|entry| {
            episode(
                entry["title"].as_str(),
                entry["enclosure"]["url"]
                    .as_str()
                    .or_else(|| entry["media"]["source"]["url"].as_str()),
            )
        })
        .take(200)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_rss_keeps_live_status_separate_from_recorded_episodes() {
        let feed = r#"<rss xmlns:podcast="https://podcastindex.org/namespace/1.0"><channel>
        <item><title>Replay</title><enclosure url="https://example.org/replay.mp3" type="audio/mpeg"/></item>
        <podcast:liveItem status="live"><title>On air</title><podcast:alternateEnclosure type="application/vnd.apple.mpegurl"><podcast:source uri="live.m3u8"/></podcast:alternateEnclosure><podcast:contentLink href="https://example.org/player"/></podcast:liveItem>
        <podcast:liveItem status="pending"><title>Tomorrow</title><enclosure url="https://example.org/radio" type="audio/mpeg"/></podcast:liveItem>
        <podcast:liveItem status="ended"><title>Ended</title><enclosure url="https://example.org/radio" type="audio/mpeg"/></podcast:liveItem>
        </channel></rss>"#;
        let episodes = parse_feed(feed, &Url::parse("https://example.org/rss").unwrap()).unwrap();
        assert_eq!(
            episodes
                .iter()
                .map(|episode| episode.kind)
                .collect::<Vec<_>>(),
            [
                AudioKind::Recorded,
                AudioKind::Live,
                AudioKind::Pending,
                AudioKind::Ended
            ]
        );
        assert_eq!(episodes[1].audio_url, "https://example.org/live.m3u8");
    }

    #[test]
    fn feeds_select_audio_enclosures_and_keep_titles_and_escaped_urls() {
        let rss = r#"<rss><channel><title>Show</title>
            <item><title><![CDATA[One & two]]></title><enclosure type="image/jpeg" url="cover.jpg"/><enclosure type="audio/mpeg" url="/one.mp3?a=1&amp;b=2"/></item>
            <item><title>Unsafe</title><enclosure type="audio/mpeg" url="file:///private.mp3"/></item>
            <item><title>Duplicate</title><enclosure url="/one.mp3?a=1&amp;b=2"/></item>
            <item><title>Two</title><enclosure url="two.m4a"/></item></channel></rss>"#;
        let episodes =
            parse_feed(rss, &Url::parse("https://example.org/feed.xml").unwrap()).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].title, "One & two");
        assert_eq!(episodes[0].audio_url, "https://example.org/one.mp3?a=1&b=2");
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Atom</title><link rel="enclosure" type="audio/mpeg" href="audio.mp3"/></entry></feed>"#;
        assert_eq!(
            parse_feed(atom, &Url::parse("https://example.org/").unwrap()).unwrap()[0].title,
            "Atom"
        );
    }

    #[test]
    fn single_episode_page_does_not_import_recommendations_or_paid_audio() {
        let page = |pay_type| {
            format!(
                r#"<script id="__NEXT_DATA__" type="application/json">{{"props":{{"pageProps":{{"episode":{{"title":"Selected","payType":"{pay_type}","enclosure":{{"url":"https://example.org/audio.m4a"}}}},"recommendations":[{{"title":"Wrong"}}]}}}}}}</script>"#
            )
        };
        assert_eq!(
            parse_xiaoyuzhou(&page("FREE")).unwrap()[0].title,
            "Selected"
        );
        assert!(parse_xiaoyuzhou(&page("PAY")).unwrap().is_empty());
    }
}
