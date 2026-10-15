//! Presentation policy shared by the platform playback adapters.
use super::{
    player::{Playback, Track},
    timeline,
};
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
    // The title clock belongs to its visible lifetime, not to the one-second
    // media snapshots. Lyrics, pause, disconnect and track changes reset it.
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
    ) -> (String, Status) {
        let Some(playback) = playback else {
            self.title = None;
            return (String::new(), Status::Waiting);
        };
        if !playback.playing {
            self.title = None;
            return (String::new(), Status::Paused);
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
        let text = if lyric.is_empty() {
            if self
                .title
                .as_ref()
                .is_none_or(|(track, _)| *track != playback.track)
            {
                self.title = Some((playback.track.clone(), now));
            }
            let elapsed = now.saturating_duration_since(self.title.as_ref().unwrap().1);
            // Also identify the song during intros, instrumental gaps and outros.
            title(&playback.track, content_budget, elapsed)
        } else {
            self.title = None;
            lyric
        };
        let text = if budget == 0 {
            String::new()
        } else if text.is_empty() {
            MUSIC_PREFIX.trim_end().to_owned()
        } else {
            format!("{MUSIC_PREFIX}{text}")
        };
        (text, status)
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
