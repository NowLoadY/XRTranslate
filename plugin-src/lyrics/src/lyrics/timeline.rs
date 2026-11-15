// Timing and progress-centered scrolling adapted from desktop-lyric/src/paper.js.
// SPDX-FileCopyrightText: tuberry
// SPDX-License-Identifier: GPL-3.0-or-later

use xrtranslate_plugin::text::TextViewport;

#[derive(Debug)]
pub(super) struct Cue {
    pub start_ms: u64,
    pub text: String,
}

pub(super) fn parse(lrc: &str) -> Vec<Cue> {
    let offset = lrc
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("[offset:")?
                .strip_suffix(']')?
                .trim()
                .parse::<i64>()
                .ok()
        })
        .unwrap_or(0);
    let mut cues = Vec::new();
    for line in lrc.lines().take(4096) {
        let mut rest = line.trim().trim_start_matches('\u{feff}');
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some((time, tail)) = tag.split_once(']') else {
                break;
            };
            if let Some(ms) = timestamp(time) {
                times.push(ms.saturating_sub(offset).max(0) as u64);
            }
            rest = tail;
        }
        let text = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut text = text
            .chars()
            .filter(|c| !c.is_control())
            .take(2048)
            .collect::<String>();
        // Ellipsis-only cues mark gaps. Keep their timestamps so the existing
        // title/artist fallback handles display and scrolling during the gap.
        if text
            .chars()
            .all(|c| matches!(c, '.' | '…' | '⋯' | '。' | '．' | ' '))
        {
            text.clear();
        }
        for start_ms in times {
            cues.push(Cue {
                start_ms,
                text: text.clone(),
            });
        }
    }
    cues.sort_by_key(|cue| cue.start_ms);
    // Keep one line at a timestamp, while retaining empty lines as actual gaps.
    cues.dedup_by(|later, earlier| {
        if later.start_ms != earlier.start_ms {
            return false;
        }
        if !later.text.is_empty() {
            earlier.text = std::mem::take(&mut later.text);
        }
        true
    });
    cues
}

fn timestamp(tag: &str) -> Option<i64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes = minutes.parse::<u32>().ok()?;
    let seconds = seconds.replace(':', ".").parse::<f64>().ok()?;
    (seconds.is_finite() && (0.0..60.0).contains(&seconds))
        .then(|| i64::from(minutes) * 60_000 + (seconds * 1000.0).round() as i64)
}

pub(super) fn viewport(cues: &[Cue], position_ms: u64, duration_ms: u64, budget: usize) -> String {
    let Some((index, end)) = active(cues, position_ms, duration_ms) else {
        return String::new();
    };
    let cue = &cues[index];
    let progress = (position_ms - cue.start_ms) as f64 / (end - cue.start_ms).max(1) as f64;
    TextViewport::new(&cue.text, budget)
        .at_progress(progress)
        .to_owned()
}

pub(super) fn window(
    cues: &[Cue],
    position_ms: u64,
    duration_ms: u64,
    lines: usize,
) -> Option<xrtranslate_plugin::lyrics::LyricWindow> {
    if lines == 0 {
        return None;
    }
    let lines = lines.min(32);
    let (index, end) = active(cues, position_ms, duration_ms)?;
    let start = index.saturating_sub(lines - 1);
    let stop = (index + lines).min(cues.len());
    let progress = u128::from(position_ms - cues[index].start_ms) * 1000
        / u128::from((end - cues[index].start_ms).max(1));
    Some(xrtranslate_plugin::lyrics::LyricWindow {
        lines: cues[start..stop]
            .iter()
            .enumerate()
            .map(|(offset, cue)| {
                if start + offset == index {
                    cue.text.clone()
                } else {
                    TextViewport::new(&cue.text, 128).start().to_owned()
                }
            })
            .collect(),
        current: index - start,
        progress: progress.min(1000) as u16,
    })
}

fn active(cues: &[Cue], position_ms: u64, duration_ms: u64) -> Option<(usize, u64)> {
    let index = cues
        .partition_point(|cue| cue.start_ms <= position_ms)
        .checked_sub(1)?;
    let cue = &cues[index];
    let end = cues
        .get(index + 1)
        .map(|next| next.start_ms)
        .unwrap_or_else(|| {
            if duration_ms > cue.start_ms {
                duration_ms
            } else {
                cue.start_ms + 8000
            }
        });
    (position_ms < end && !cue.text.is_empty()).then_some((index, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_window_tracks_current_cue_seeks_gaps_and_end_of_song() {
        let cues = parse("[00:01]First\n[00:03]Second\n[00:05]Third\n[00:07]...\n[00:09]Last");
        assert!(window(&cues, 0, 12_000, 9).is_none());
        assert!(window(&cues, 4_000, 12_000, 0).is_none());
        let middle = window(&cues, 4_000, 12_000, 2).unwrap();
        assert_eq!(middle.lines, ["First", "Second", "Third"]);
        assert_eq!(middle.current, 1);
        assert_eq!(middle.progress, 500);
        let seek = window(&cues, 9_000, 12_000, 9).unwrap();
        assert_eq!(seek.lines[seek.current], "Last");
        assert_eq!(seek.progress, 0);
        assert!(window(&cues, 8_000, 12_000, 9).is_none());
        assert!(window(&cues, 12_000, 12_000, 9).is_none());
    }

    #[test]
    fn shared_viewport_preserves_singing_progress_and_cue_gaps() {
        let cues = parse("[00:01]ABCDEFGHIJKLMNOPQRST\n[00:05]...");
        assert_eq!(viewport(&cues, 1_000, 10_000, 6), "ABCDEF");
        assert_eq!(viewport(&cues, 3_000, 10_000, 6), "HIJKLM");
        assert_eq!(viewport(&cues, 4_999, 10_000, 6), "OPQRST");
        assert_eq!(viewport(&cues, 5_000, 10_000, 6), "");
    }
}
