//! Idle music owns only the space left by visible banners. Three lyric regions
//! combine into one song display; regular captions never enter this path.
use super::{BannerContentType, Block, Frame, LyricsText, OscSettings, TextViewport, banners};
use xrtranslate_plugin::lyrics::LyricWindow;

const LINE_CHARS: usize = 24;
const CURRENT_MARKER: &str = "› ";

pub(super) fn render(
    header: &str,
    footer: &str,
    lyrics: &LyricsText,
    settings: &OscSettings,
    frame: &mut Frame<'_>,
) -> String {
    let unified = settings.header_config.content_type == BannerContentType::Lyrics
        && settings.footer_config.content_type == BannerContentType::Lyrics;
    let limit = settings.text_limit();
    let lines = settings.line_limit();
    let has_lyrics = lyrics.window.as_ref().is_some_and(|window| {
        window
            .lines
            .get(window.current)
            .is_some_and(|line| !line.trim().is_empty())
    }) || !lyrics.lyric.is_empty();
    if unified && !has_lyrics {
        let title = TextViewport::new(&lyrics.title, limit).start().to_owned();
        let text = frame.compose(&title, "", "", lines);
        frame.renderer.regions.unified_lyrics = true;
        return text;
    }
    // Each separate banner keeps one compact row, leaving most room for lyrics.
    // The merged display reserves the title once and returns the footer's space.
    let banner_budget = (limit / 3).min(LINE_CHARS);
    let header = frame.banner(
        Block::Header,
        header,
        banner_budget,
        settings.header_config.content_type,
    );
    let footer = if unified {
        String::new()
    } else {
        frame.banner(
            Block::Footer,
            footer,
            banner_budget,
            settings.footer_config.content_type,
        )
    };
    let occupied_lines = header.lines().count() + footer.lines().count();
    let pet_budget = banners::pet_budget([&settings.header_config, &settings.footer_config], limit)
        .min(banner_budget);
    let budget = limit.saturating_sub(
        settings.header_config.reserved_chars(&header, pet_budget)
            + settings.footer_config.reserved_chars(&footer, pet_budget)
            + usize::from(!header.is_empty())
            + usize::from(!footer.is_empty()),
    );
    let body_lines = lines.saturating_sub(occupied_lines);
    let body = if let Some(window) = &lyrics.window {
        fit_window(window, budget, body_lines)
    } else {
        let fallback = if has_lyrics {
            lyrics.lyric.as_str()
        } else if !(settings.header_config.content_type == BannerContentType::Lyrics
            && !header.is_empty()
            || settings.footer_config.content_type == BannerContentType::Lyrics
                && !footer.is_empty())
        {
            lyrics.title.as_str()
        } else {
            ""
        };
        TextViewport::new(fallback, budget.min(LINE_CHARS))
            .start()
            .to_owned()
    };
    let text = frame.compose(&header, &body, &footer, lines);
    frame.renderer.regions.unified_lyrics = unified;
    text
}

fn fit_window(window: &LyricWindow, budget: usize, line_limit: usize) -> String {
    let Some(current) = window.lines.get(window.current) else {
        return String::new();
    };
    if budget == 0 || line_limit == 0 {
        return String::new();
    }
    let marker = if budget > CURRENT_MARKER.chars().count() {
        CURRENT_MARKER
    } else {
        ""
    };
    let width = budget
        .min(LINE_CHARS)
        .saturating_sub(marker.chars().count());
    let current = clean(current);
    let visible = TextViewport::new(&current, width)
        .at_progress(f64::from(window.progress) / 1000.0)
        .to_owned();
    if visible.is_empty() {
        return String::new();
    }
    let current = format!("{marker}{visible}");
    let mut used = current.chars().count();
    let mut selected = vec![(window.current, current)];
    let (mut before, mut after) = (true, true);
    for distance in 1..window.lines.len() {
        if selected.len() >= line_limit || (!before && !after) {
            break;
        }
        // Prefer the next line when only two rows fit. Extra rows grow around
        // the current cue, retaining chronological order after selection.
        for (index, open) in [
            (
                window
                    .current
                    .checked_add(distance)
                    .filter(|&index| index < window.lines.len()),
                &mut after,
            ),
            (window.current.checked_sub(distance), &mut before),
        ] {
            if !*open || selected.len() >= line_limit {
                continue;
            }
            let Some(index) = index else {
                *open = false;
                continue;
            };
            let text = clean(&window.lines[index]);
            let text = TextViewport::new(&text, LINE_CHARS).start().to_owned();
            // Empty cues delimit instrumental gaps; don't pull unrelated lines
            // across a gap just to fill available space.
            if text.is_empty() || used + 1 + text.chars().count() > budget {
                *open = false;
                continue;
            }
            used += 1 + text.chars().count();
            selected.push((index, text));
        }
    }
    selected.sort_by_key(|(index, _)| *index);
    selected
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "chatbox_lyrics_tests.rs"]
mod tests;
