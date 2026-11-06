//! Shared input cleanup and standards-based discovery; no site-specific scraping.
use dom_query::Document;
use reqwest::Url;
use serde_json::Value;

use super::http::http_url;

pub fn input_url(input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() || std::path::Path::new(input).is_file() {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    // Share text may put the URL immediately after a title or Chinese punctuation.
    let start = ["https://", "http://", "www."]
        .iter()
        .filter_map(|prefix| lower.find(prefix))
        .min();
    let text = start.map_or(input, |index| &input[index..]);
    text.split(|c: char| c.is_whitespace() || "<>\"'`【】「」『』“”‘’，。！？；、｜".contains(c))
        .find_map(input_token)
}

fn input_token(token: &str) -> Option<Url> {
    let mut candidate = token
        .trim_start_matches(['(', '[', '（'])
        .trim_end_matches(['.', ',', ';', '!']);
    // Preserve parentheses in valid URLs, stripping only the share text's wrapper.
    for (open, close) in [('(', ')'), ('[', ']'), ('（', '）')] {
        while candidate.ends_with(close)
            && candidate.matches(close).count() > candidate.matches(open).count()
        {
            candidate = candidate.strip_suffix(close)?;
        }
    }
    if let Ok(url) = http_url(candidate) {
        return Some(url);
    }
    if candidate.contains(['\\', '@']) || candidate.starts_with(['/', '.', '~']) {
        return None;
    }
    let url = http_url(&format!("https://{candidate}")).ok()?;
    let host = url.host_str()?;
    let (_, suffix) = host.rsplit_once('.')?;
    (suffix.len() >= 2 && suffix.bytes().all(|c| c.is_ascii_alphabetic())).then_some(url)
}

#[derive(Default)]
pub struct PageLinks {
    pub title: String,
    pub canonical: Option<Url>,
    pub thumbnail: Option<String>,
    pub media: Vec<Url>,
    pub feeds: Vec<Url>,
    pub downloads: Vec<Url>,
}

pub fn page_links(document: &str, page_url: &Url) -> PageLinks {
    let document = Document::from(document);
    let base = document
        .select("base[href]")
        .attr("href")
        .and_then(|href| page_url.join(&href).ok())
        .unwrap_or_else(|| page_url.clone());
    let mut links = PageLinks {
        title: document.select("title").text().trim().to_owned(),
        canonical: document
            .select("link[rel='canonical']")
            .attr("href")
            .and_then(|href| page_url.join(&href).ok()),
        ..Default::default()
    };
    links.thumbnail = document
        .select("meta[property='og:image']")
        .attr("content")
        .or_else(|| document.select("video[poster]").attr("poster"))
        .and_then(|value| absolute_url(&base, &value))
        .map(Into::into);
    for node in document.select("audio, video").iter() {
        // Child sources are alternative encodings of the same player.
        let sources = node.select("source[src]");
        for src in node
            .attr("src")
            .into_iter()
            .chain(sources.iter().filter_map(|node| node.attr("src")))
        {
            if push_url(&mut links.media, &base, &src) {
                break;
            }
        }
    }
    for node in document.select("meta[property][content]").iter() {
        if let (Some(property), Some(content)) = (node.attr("property"), node.attr("content")) {
            match property.as_ref() {
                "og:audio"
                | "og:audio:url"
                | "og:audio:secure_url"
                | "og:video"
                | "og:video:url"
                | "og:video:secure_url" => {
                    push_url(&mut links.media, &base, &content);
                }
                "og:title" => links.title = content.trim().to_owned(),
                _ => {}
            }
        }
    }
    for node in document.select("script[type='application/ld+json']").iter() {
        if let Ok(value) = serde_json::from_str(&node.text()) {
            structured_media(&value, &base, &mut links.media);
        }
    }
    for node in document.select("link[href], a[href]").iter() {
        let Some(href) = node.attr("href") else {
            continue;
        };
        let mime = node.attr("type").unwrap_or_default().to_ascii_lowercase();
        if matches!(
            mime.split(';').next().unwrap_or("").trim(),
            "application/rss+xml" | "application/atom+xml"
        ) {
            push_url(&mut links.feeds, &base, &href);
        } else if let Ok(url) = base.join(&href) {
            if media_path(&url) || mime.starts_with("audio/") || mime.starts_with("video/") {
                push_url(&mut links.downloads, &base, &href);
            }
        }
    }
    // A site can advertise both an all-site feed and the current show's feed.
    links.feeds.sort_by_key(|url| {
        std::cmp::Reverse((
            url.origin() == page_url.origin(),
            url.path()
                .split('/')
                .zip(page_url.path().split('/'))
                .take_while(|(a, b)| a == b)
                .count(),
        ))
    });
    links
}

pub fn media_path(url: &Url) -> bool {
    matches!(
        url.path()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "mp3" | "m4a" | "aac" | "ogg" | "opus" | "flac" | "wav" | "mp4" | "webm" | "mkv" | "m3u8"
    )
}

fn push_url(urls: &mut Vec<Url>, base: &Url, value: &str) -> bool {
    if urls.len() < 16
        && let Some(url) = absolute_url(base, value)
    {
        if !urls.contains(&url) {
            urls.push(url);
        }
        return true;
    }
    false
}

pub fn absolute_url(base: &Url, value: &str) -> Option<Url> {
    let mut url = base.join(value.trim()).ok()?;
    http_url(url.as_str()).ok()?;
    url.set_fragment(None);
    Some(url)
}

fn structured_media(value: &Value, base: &Url, urls: &mut Vec<Url>) {
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|value| structured_media(value, base, urls)),
        Value::Object(object) => {
            let is_media = |kind: &Value| {
                kind.as_str().is_some_and(|kind| {
                    matches!(kind.rsplit('/').next(), Some("AudioObject" | "VideoObject"))
                })
            };
            if object.get("@type").is_some_and(|kind| {
                is_media(kind)
                    || kind
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(is_media))
            }) && let Some(url) = object.get("contentUrl").and_then(Value::as_str)
            {
                push_url(urls, base, url);
            }
            for value in object.values() {
                structured_media(value, base, urls);
            }
        }
        _ => {}
    }
}
