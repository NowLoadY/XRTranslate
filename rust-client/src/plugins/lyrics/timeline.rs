// Timing and progress-centered scrolling adapted from desktop-lyric/src/paper.js.
// SPDX-FileCopyrightText: tuberry
// SPDX-License-Identifier: GPL-3.0-or-later

use unicode_segmentation::UnicodeSegmentation;

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
    let Some(index) = cues
        .partition_point(|cue| cue.start_ms <= position_ms)
        .checked_sub(1)
    else {
        return String::new();
    };
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
    if budget == 0 || position_ms >= end || cue.text.is_empty() {
        return String::new();
    }
    let graphemes = cue.text.graphemes(true).collect::<Vec<_>>();
    let lengths = graphemes
        .iter()
        .map(|text| text.chars().count())
        .collect::<Vec<_>>();
    let total: usize = lengths.iter().sum();
    if total <= budget {
        return cue.text.clone();
    }
    let progress = (position_ms - cue.start_ms) as f64 / (end - cue.start_ms).max(1) as f64;
    // As in desktop-lyric, keep the estimated singing position near the center,
    // holding the start and end still. OSC limits are Unicode characters.
    let offset =
        ((total as f64 * progress - budget as f64 / 2.0).max(0.0) as usize).min(total - budget);
    let mut skipped = 0;
    let mut used = 0;
    let mut output = String::new();
    for (text, length) in graphemes.into_iter().zip(lengths) {
        if skipped < offset {
            skipped += length;
            continue;
        }
        if used + length > budget {
            break;
        }
        output.push_str(text);
        used += length;
    }
    output.trim().to_owned()
}
