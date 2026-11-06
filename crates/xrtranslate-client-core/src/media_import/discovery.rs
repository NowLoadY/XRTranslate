//! Shared discovery of public media, browser pages and subscription catalogues.
use std::{
    collections::HashSet,
    sync::{Arc, atomic::AtomicBool},
};

use reqwest::Url;
use serde_json::Value;

use super::http::{HttpClient, http_url};
pub use super::links::input_url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaResource {
    pub title: String,
    pub url: String,
    pub kind: MediaKind,
    pub video: bool,
    pub thumbnail: Option<String>,
    webpage: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Recorded,
    Live,
    Pending,
    Ended,
    Unknown,
}

/// A feed is a selection of episodes, never an implicit download of its first item.
pub fn resolve(
    input: &str,
    proxy: &str,
    stop: Arc<AtomicBool>,
) -> Result<Vec<MediaResource>, String> {
    let url = input_url(input).ok_or("Paste a valid link")?;
    let client = HttpClient::new(proxy, stop)?;
    resolve_url(&client, url, &mut HashSet::new())
}

fn resolve_url(
    client: &HttpClient,
    mut url: Url,
    visited: &mut HashSet<String>,
) -> Result<Vec<MediaResource>, String> {
    url.set_fragment(None);
    if visited.len() >= 16 || !visited.insert(url.to_string()) {
        return Err("No public audio found on this page".into());
    }
    if url.host_str() == Some("podcasts.apple.com") {
        return resolve_apple(client, &url);
    }
    let response = client.get(&url, None)?;
    if response.url() != &url {
        url = response.url().clone();
        url.set_fragment(None);
        if !visited.insert(url.to_string()) {
            return Err("No public audio found on this page".into());
        }
        if url.host_str() == Some("podcasts.apple.com") {
            return resolve_apple(client, &url);
        }
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("mpegurl") || url.path().ends_with(".m3u8") {
        let document = client.document(response)?;
        let kind = hls_kind(client, &url, &document, 0)?;
        return Ok(vec![MediaResource {
            title: url
                .path()
                .rsplit('/')
                .next()
                .unwrap_or("Online audio")
                .to_owned(),
            url: url.into(),
            kind,
            video: true,
            thumbnail: None,
            webpage: None,
        }]);
    }
    if content_type.starts_with("audio/")
        || content_type.starts_with("video/")
        || super::links::media_path(&url) && !content_type.contains("html")
    {
        return Ok(vec![MediaResource {
            title: url
                .path_segments()
                .and_then(|mut parts| parts.next_back())
                .filter(|name| !name.is_empty())
                .unwrap_or("Online audio")
                .to_owned(),
            url: url.into(),
            kind: if ["icy-name", "icy-br", "icy-description"]
                .iter()
                .any(|key| response.headers().contains_key(*key))
            {
                MediaKind::Live
            } else if response.content_length().is_some() {
                MediaKind::Recorded
            } else {
                MediaKind::Unknown
            },
            video: content_type.starts_with("video/"),
            thumbnail: None,
            webpage: None,
        }]);
    }
    let document = client.document(response)?;
    if matches!(
        url.host_str(),
        Some("www.xiaoyuzhoufm.com" | "xiaoyuzhoufm.com")
    ) {
        let mut episodes = parse_xiaoyuzhou(&document)?;
        let thumbnail = super::links::page_links(&document, &url).thumbnail;
        for episode in &mut episodes {
            episode.thumbnail = thumbnail.clone();
        }
        return if episodes.is_empty() {
            Err("No public audio found on this page".into())
        } else {
            Ok(episodes)
        };
    }
    if let Ok(episodes) = parse_feed(&document, &url)
        && !episodes.is_empty()
    {
        return Ok(episodes);
    }
    let page = super::links::page_links(&document, &url);
    let mut error = None;
    // Prefer the page's player, then its most relevant feed, then download links.
    for (sources, feed) in [
        (page.media, false),
        (page.feeds, true),
        (page.downloads, false),
    ] {
        let mut episodes: Vec<MediaResource> = Vec::new();
        for source in sources {
            if visited.len() >= 16 {
                break;
            }
            if visited.contains(source.as_str()) {
                continue;
            }
            match resolve_url(client, source, visited) {
                Ok(mut found) if feed => {
                    let page_url = page.canonical.as_ref().unwrap_or(&url);
                    if let Some(index) = found
                        .iter()
                        .position(|episode| episode.webpage.as_ref() == Some(page_url))
                    {
                        return Ok(vec![found.swap_remove(index)]);
                    }
                    return Ok(found);
                }
                Ok(found) => {
                    for episode in found {
                        if !episodes.iter().any(|existing| existing.url == episode.url) {
                            episodes.push(episode);
                        }
                    }
                }
                Err(cause) => {
                    error.get_or_insert(cause);
                }
            }
        }
        if !episodes.is_empty() {
            if episodes.len() == 1 {
                if !page.title.is_empty() {
                    episodes[0].title = page.title;
                }
                if episodes[0].thumbnail.is_none() {
                    episodes[0].thumbnail = page.thumbnail;
                }
            }
            return Ok(episodes);
        }
    }
    Err(error.unwrap_or_else(|| "No public audio found on this page".into()))
}

fn hls_kind(
    client: &HttpClient,
    base: &Url,
    document: &str,
    depth: usize,
) -> Result<MediaKind, String> {
    if depth > 3 || !document.trim_start().starts_with("#EXTM3U") {
        return Err("Invalid HLS playlist".into());
    }
    if document.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
        return Ok(MediaKind::Recorded);
    }
    if document.contains("#EXT-X-TARGETDURATION:") {
        return Ok(MediaKind::Live);
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

fn resolve_apple(client: &HttpClient, url: &Url) -> Result<Vec<MediaResource>, String> {
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
        .filter_map(|entry| {
            let mut resource = episode(entry["trackName"].as_str(), entry["episodeUrl"].as_str())?;
            resource.thumbnail = entry["artworkUrl600"]
                .as_str()
                .or_else(|| entry["artworkUrl100"].as_str())
                .and_then(|value| http_url(value).ok())
                .map(Into::into);
            Some(resource)
        })
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

fn episode(title: Option<&str>, url: Option<&str>) -> Option<MediaResource> {
    Some(MediaResource {
        title: title
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Untitled episode")
            .to_owned(),
        url: http_url(url?).ok()?.into(),
        kind: MediaKind::Recorded,
        video: false,
        thumbnail: None,
        webpage: None,
    })
}

fn parse_feed(document: &str, base: &Url) -> Result<Vec<MediaResource>, String> {
    let xml = roxmltree::Document::parse(document)
        .map_err(|_| "This link is not a supported podcast page or RSS feed".to_owned())?;
    let mut episodes: Vec<MediaResource> = Vec::new();
    let channel_image = xml
        .descendants()
        .find(|node| matches!(node.tag_name().name(), "channel" | "feed"))
        .and_then(|node| feed_image(node, base));
    for item in xml.descendants().filter(|node| {
        node.has_tag_name("item")
            || node.tag_name().name() == "entry"
            || node.has_tag_name(("https://podcastindex.org/namespace/1.0", "liveItem"))
    }) {
        let kind = if item.tag_name().name() == "liveItem" {
            match item.attribute("status") {
                Some("live") => MediaKind::Live,
                Some("pending") => MediaKind::Pending,
                Some("ended") => MediaKind::Ended,
                _ => continue,
            }
        } else {
            MediaKind::Recorded
        };
        let same_kind = episodes
            .iter()
            .filter(|episode| {
                (episode.kind == MediaKind::Recorded) == (kind == MediaKind::Recorded)
            })
            .count();
        if same_kind >= if kind == MediaKind::Recorded { 200 } else { 32 } {
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
            && !episodes.iter().any(|existing: &MediaResource| {
                existing.url == episode.url && existing.kind == kind
            })
        {
            episode.kind = kind;
            episode.thumbnail = feed_image(item, base).or_else(|| channel_image.clone());
            episode.webpage = item
                .children()
                .find(|node| {
                    node.tag_name().name() == "link"
                        && node.attribute("rel").is_none_or(|rel| rel == "alternate")
                })
                .and_then(|node| node.attribute("href").or_else(|| node.text()))
                .and_then(|link| base.join(link.trim()).ok());
            episode.video = enclosure
                .and_then(|node| node.attribute("type"))
                .is_some_and(|mime| mime.starts_with("video/"));
            episodes.push(episode);
        }
    }
    Ok(episodes)
}

fn feed_image(node: roxmltree::Node<'_, '_>, base: &Url) -> Option<String> {
    node.children()
        .filter(|node| matches!(node.tag_name().name(), "image" | "thumbnail"))
        .find_map(|node| {
            node.attribute("href")
                .or_else(|| node.attribute("url"))
                .or_else(|| {
                    node.children()
                        .find(|node| node.tag_name().name() == "url")
                        .and_then(|node| node.text())
                })
        })
        .and_then(|value| super::links::absolute_url(base, value))
        .map(Into::into)
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

fn parse_xiaoyuzhou(document: &str) -> Result<Vec<MediaResource>, String> {
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
                MediaKind::Recorded,
                MediaKind::Live,
                MediaKind::Pending,
                MediaKind::Ended
            ]
        );
        assert_eq!(episodes[1].url, "https://example.org/live.m3u8");
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
        assert_eq!(episodes[0].url, "https://example.org/one.mp3?a=1&b=2");
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
