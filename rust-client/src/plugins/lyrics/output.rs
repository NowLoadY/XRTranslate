//! Presentation policy shared by the platform playback adapters.
use super::{
    player::{Playback, Track},
    timeline,
};
use crate::plugins::text_output::LyricsText;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

const MAX_LINE_CHARS: usize = 24;
const MUSIC_PREFIX: &str = "🎵 ";
const TITLE_HOLD_SECONDS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Idle,
    Waiting,
    Paused,
    MediaTitle,
    NoTimeline,
    Loading,
    NoLyrics,
    LookupFailed,
    Synced,
}

impl Status {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Idle => "Select Lyrics in an enabled header or footer to start.",
            Self::Waiting => "Waiting for a player. Enable system media controls in your player.",
            Self::Paused => "Playback paused.",
            Self::MediaTitle => "Showing media title.",
            Self::NoTimeline => {
                "Showing song title: the player does not provide playback progress."
            }
            Self::Loading => "Showing song title while loading lyrics.",
            Self::NoLyrics => "Showing song title: no matching timed lyrics found.",
            Self::LookupFailed => "Showing song title: lyrics could not be loaded.",
            Self::Synced => "Lyrics synchronized with playback.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lookup {
    NotStarted,
    Loading,
    Ready,
    Failed,
}

#[derive(Default)]
pub(super) struct Renderer {
    // Scroll the title independently of lyric changes and media snapshots.
    // Pause, disconnect and track changes reset it.
    title: Option<(Track, Instant)>,
}

impl Renderer {
    pub(super) fn render(
        &mut self,
        playback: Option<&Playback>,
        cues: &[timeline::Cue],
        lookup: Lookup,
        budget: usize,
        now: Instant,
    ) -> (LyricsText, Status) {
        let Some(playback) = playback else {
            self.title = None;
            return (LyricsText::default(), Status::Waiting);
        };
        if !playback.playing {
            self.title = None;
            return (LyricsText::default(), Status::Paused);
        }
        let budget = budget.min(MAX_LINE_CHARS);
        let content_budget = budget.saturating_sub(MUSIC_PREFIX.chars().count());
        let (lyric, status) = if !playback.is_music {
            (String::new(), Status::MediaTitle)
        } else if let Some(position) = playback.position() {
            match lookup {
                Lookup::NotStarted | Lookup::Loading => (String::new(), Status::Loading),
                Lookup::Failed => (String::new(), Status::LookupFailed),
                Lookup::Ready if cues.iter().all(|cue| cue.text.is_empty()) => {
                    (String::new(), Status::NoLyrics)
                }
                Lookup::Ready => (
                    timeline::viewport(cues, position, playback.duration_ms, content_budget),
                    Status::Synced,
                ),
            }
        } else {
            (String::new(), Status::NoTimeline)
        };
        if self
            .title
            .as_ref()
            .is_none_or(|(track, _)| *track != playback.track)
        {
            self.title = Some((playback.track.clone(), now));
        }
        let elapsed = now.saturating_duration_since(self.title.as_ref().unwrap().1);
        let title = title(&playback.track, content_budget, elapsed);
        let prefix = |text: &str| {
            if budget == 0 {
                String::new()
            } else if text.is_empty() {
                MUSIC_PREFIX.trim_end().to_owned()
            } else {
                format!("{MUSIC_PREFIX}{text}")
            }
        };
        (
            LyricsText {
                title: prefix(&title),
                // An unavailable lyric leaves the second region empty.
                lyric: if lyric.is_empty() {
                    String::new()
                } else {
                    prefix(&lyric)
                },
            },
            status,
        )
    }
}

fn title(track: &Track, budget: usize, elapsed: Duration) -> String {
    let text = std::iter::once(track.title.as_str())
        .chain(track.artists.iter().map(String::as_str))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.chars().filter(|c| !c.is_control()).collect::<String>();
    if text.chars().count() <= budget {
        return text;
    }
    if budget == 0 {
        return String::new();
    }
    let graphemes = text.graphemes(true).collect::<Vec<_>>();
    let mut tail_length = 0;
    let mut last_start = graphemes.len();
    for (index, grapheme) in graphemes.iter().enumerate().rev() {
        let length = grapheme.chars().count();
        if tail_length + length > budget {
            break;
        }
        tail_length += length;
        last_start = index;
    }
    // Advance one complete grapheme per second, holding both ends before looping.
    let phase = elapsed.as_secs() % (last_start as u64 + 2 * TITLE_HOLD_SECONDS);
    let start = phase
        .saturating_sub(TITLE_HOLD_SECONDS)
        .min(last_start as u64) as usize;
    let mut output = String::new();
    let mut used = 0;
    for grapheme in graphemes.into_iter().skip(start) {
        let length = grapheme.chars().count();
        if used + length > budget {
            break;
        }
        output.push_str(grapheme);
        used += length;
    }
    output.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(now: Instant) -> Playback {
        Playback {
            track: Track {
                title: "歌名".into(),
                artists: vec!["歌手".into()],
                album: String::new(),
            },
            is_music: true,
            embedded_lyrics: None,
            duration_ms: 30_000,
            position_ms: Some(5_000),
            observed: now,
            rate: 0.0,
            playing: true,
        }
    }

    #[test]
    fn title_and_lyric_are_independent_and_unavailable_lyrics_stay_empty() {
        let now = Instant::now();
        let mut playback = playback(now);
        let mut renderer = Renderer::default();
        let cues = timeline::parse("[00:01]当前歌词\n[00:10]...");
        let (text, status) = renderer.render(Some(&playback), &cues, Lookup::Ready, 24, now);
        assert_eq!(status, Status::Synced);
        assert_eq!(text.title, "🎵 歌名 · 歌手");
        assert_eq!(text.lyric, "🎵 当前歌词");
        assert_eq!(text.preferred(), text.lyric);

        for lookup in [Lookup::Loading, Lookup::Failed, Lookup::Ready] {
            let (text, _) = renderer.render(Some(&playback), &[], lookup, 24, now);
            assert_eq!(text.title, "🎵 歌名 · 歌手");
            assert!(text.lyric.is_empty());
            assert_eq!(text.preferred(), text.title);
        }
        for position in [Some(0), Some(12_000), None] {
            playback.position_ms = position;
            let (text, _) = renderer.render(Some(&playback), &cues, Lookup::Ready, 24, now);
            assert_eq!(text.title, "🎵 歌名 · 歌手");
            assert!(text.lyric.is_empty());
        }
        playback.playing = false;
        assert!(
            renderer
                .render(Some(&playback), &cues, Lookup::Ready, 24, now)
                .0
                .is_empty()
        );
        assert!(
            renderer
                .render(None, &cues, Lookup::Ready, 24, now)
                .0
                .is_empty()
        );
    }

    #[test]
    fn title_scrolls_while_lyrics_are_visible_and_resets_for_a_new_track() {
        let now = Instant::now();
        let mut playback = playback(now);
        playback.track.title = "这是一个需要滚动才能显示完整名称的歌曲标题".into();
        let cues = timeline::parse("[00:01]当前歌词");
        let mut renderer = Renderer::default();
        let (start, _) = renderer.render(Some(&playback), &cues, Lookup::Ready, 12, now);
        let (scrolled, _) = renderer.render(
            Some(&playback),
            &cues,
            Lookup::Ready,
            12,
            now + Duration::from_secs(5),
        );
        assert_ne!(start.title, scrolled.title);
        assert_eq!(start.lyric, scrolled.lyric);
        for text in [&start.title, &start.lyric, &scrolled.title, &scrolled.lyric] {
            assert!(text.starts_with(MUSIC_PREFIX));
            assert!(text.chars().count() <= 12);
        }
        playback.track.title = "新歌".into();
        assert_eq!(
            renderer
                .render(
                    Some(&playback),
                    &cues,
                    Lookup::Ready,
                    12,
                    now + Duration::from_secs(6)
                )
                .0
                .title,
            "🎵 新歌 · 歌手"
        );
    }
}
