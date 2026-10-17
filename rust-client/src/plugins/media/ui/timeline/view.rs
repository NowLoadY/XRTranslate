use crate::plugins::media::subtitles::SubtitleCue;
use std::ops::Range;

/// A time window and its reversible focus-and-context projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) focus: f64,
    strength: f64,
}

impl View {
    pub(super) fn full(duration: i64) -> Self {
        Self {
            start: 0.0,
            end: duration.max(1) as f64,
            focus: 0.0,
            strength: 0.0,
        }
    }

    pub(super) fn following(cues: &[SubtitleCue], duration: i64, focus: f64, width: f32) -> Self {
        let full = Self::full(duration);
        let focus = finite_clamp(focus, 0.0, full.end);
        if cues.len() <= 1 {
            return Self { focus, ..full };
        }

        let count = cues.len() as f64;
        // Reserve a readable average width before enlarging cues near playback.
        let budget = (f64::from(width) / 24.0).max(12.0).min(count);
        let position = cue_position(cues, full.end, focus);
        let first = (position - budget / 2.0).clamp(0.0, count - budget);
        let start = time_at_position(cues, full.end, first);
        let end = time_at_position(cues, full.end, first + budget);
        // A group of equal timestamps can consume the entire window. There is
        // no useful local time scale then; the full range remains navigable.
        if end <= start {
            return Self { focus, ..full };
        }
        Self {
            start,
            end,
            focus: focus.clamp(start, end),
            strength: 4.0,
        }
    }

    pub(super) fn fraction(self, time: f64) -> f64 {
        let time = finite_clamp(time, self.start, self.end);
        if time <= self.start {
            return 0.0;
        }
        if time >= self.end {
            return 1.0;
        }
        let position = (time - self.start) / (self.end - self.start);
        if self.strength == 0.0 {
            return position;
        }
        let focus = (self.focus - self.start) / (self.end - self.start);
        let left = (-self.strength * focus).asinh();
        let right = (self.strength * (1.0 - focus)).asinh();
        (((self.strength * (position - focus)).asinh() - left) / (right - left)).clamp(0.0, 1.0)
    }

    pub(super) fn time(self, fraction: f64) -> f64 {
        let fraction = finite_clamp(fraction, 0.0, 1.0);
        if fraction <= 0.0 {
            return self.start;
        }
        if fraction >= 1.0 {
            return self.end;
        }
        let position = if self.strength == 0.0 {
            fraction
        } else {
            let focus = (self.focus - self.start) / (self.end - self.start);
            let left = (-self.strength * focus).asinh();
            let right = (self.strength * (1.0 - focus)).asinh();
            focus + (left + fraction * (right - left)).sinh() / self.strength
        };
        (self.start + position * (self.end - self.start)).clamp(self.start, self.end)
    }

    /// The last cue starting at or before the left edge may cross it. The
    /// caller clips it using the timeline's authoritative end time.
    pub(super) fn visible_range(self, cues: &[SubtitleCue]) -> Range<usize> {
        let first = cues
            .partition_point(|cue| (cue.start_ms as f64) <= self.start)
            .saturating_sub(1);
        let end = cues.partition_point(|cue| (cue.start_ms as f64) < self.end);
        first..end.max(first)
    }
}

fn finite_clamp(value: f64, min: f64, max: f64) -> f64 {
    if value.is_nan() {
        min
    } else {
        value.clamp(min, max)
    }
}

fn anchor_time(cues: &[SubtitleCue], duration: f64, index: usize) -> f64 {
    if index == 0 {
        0.0
    } else if index >= cues.len() {
        duration
    } else {
        (cues[index].start_ms as f64).clamp(0.0, duration)
    }
}

fn time_at_position(cues: &[SubtitleCue], duration: f64, position: f64) -> f64 {
    let index = position.floor() as usize;
    let left = anchor_time(cues, duration, index);
    let right = anchor_time(cues, duration, index + 1);
    left + (right - left) * position.fract()
}

fn cue_position(cues: &[SubtitleCue], duration: f64, time: f64) -> f64 {
    if time <= 0.0 {
        return 0.0;
    }
    if time >= duration {
        return cues.len() as f64;
    }
    // The next start is strictly later, even when several cues share a start.
    let next = cues
        .partition_point(|cue| (cue.start_ms as f64) <= time)
        .clamp(1, cues.len());
    let left = anchor_time(cues, duration, next - 1);
    let right = anchor_time(cues, duration, next);
    if right <= left {
        return next as f64;
    }
    (next - 1) as f64 + ((time - left) / (right - left)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cues(count: usize, step: i64) -> Vec<SubtitleCue> {
        (0..count)
            .map(|index| SubtitleCue {
                id: index.to_string(),
                start_ms: index as i64 * step,
                end_ms: (index as i64 + 1) * step,
                speaker_name: None,
                original_text: String::new(),
                translated_text: None,
                source_language: String::new(),
                target_language: String::new(),
            })
            .collect()
    }

    fn assert_roundtrip(view: View) {
        assert!(view.start.is_finite() && view.end.is_finite() && view.focus.is_finite());
        assert!(view.end > view.start);
        assert_eq!(view.fraction(view.start), 0.0);
        assert_eq!(view.fraction(view.end), 1.0);
        let mut previous = view.start;
        for sample in 0..=100 {
            let fraction = sample as f64 / 100.0;
            let time = view.time(fraction);
            assert!(time.is_finite() && time >= previous);
            assert!((view.fraction(time) - fraction).abs() < 1e-9);
            previous = time;
        }
    }

    #[test]
    fn full_range_is_linear_and_local_range_is_reversible_at_both_edges() {
        let cues = cues(1000, 1000);
        let full = View::full(1_000_000);
        assert_eq!(full.fraction(250_000.0), 0.25);
        assert_eq!(full.time(0.75), 750_000.0);
        assert_roundtrip(full);
        for focus in [0.0, 1000.0, 500_500.0, 999_500.0, 1_000_000.0] {
            assert_roundtrip(View::following(&cues, 1_000_000, focus, 600.0));
        }
    }

    #[test]
    fn long_lists_keep_a_bounded_neighborhood_and_preserve_its_size_at_the_edges() {
        let cues = cues(10_000, 1000);
        for focus in [0.0, 5_000_500.0, 10_000_000.0] {
            let view = View::following(&cues, 10_000_000, focus, 600.0);
            assert!((view.end - view.start - 25_000.0).abs() < 1e-6);
            assert!(view.visible_range(&cues).len() <= 27);
            assert!(view.start <= focus && focus <= view.end);
        }
        let narrow = View::following(&cues, 10_000_000, 5_000_000.0, 1.0);
        assert_eq!(narrow.end - narrow.start, 12_000.0);
    }

    #[test]
    fn equal_time_spans_get_more_space_near_the_focus() {
        let cues = cues(1000, 1000);
        let view = View::following(&cues, 1_000_000, 500_500.0, 600.0);
        let near = view.fraction(view.focus + 500.0) - view.fraction(view.focus - 500.0);
        let far = view.fraction(view.start + 1000.0) - view.fraction(view.start);
        assert!(near > far * 1.5);
    }

    #[test]
    fn window_boundaries_move_continuously_across_cues_and_the_initial_clamp() {
        let cues = cues(200, 1000);
        for boundary in [10_000.0, 50_000.0, 190_000.0] {
            let before = View::following(&cues, 200_000, boundary - 0.1, 480.0);
            let after = View::following(&cues, 200_000, boundary + 0.1, 480.0);
            assert!((after.start - before.start).abs() < 1.0);
            assert!((after.end - before.end).abs() < 1.0);
            assert!((after.fraction(boundary) - before.fraction(boundary)).abs() < 0.001);
        }
    }

    #[test]
    fn range_includes_the_cue_crossing_its_left_edge() {
        let cues = cues(1000, 1000);
        let view = View::following(&cues, 1_000_000, 500_250.0, 600.0);
        let range = view.visible_range(&cues);
        assert!((cues[range.start].start_ms as f64) <= view.start);
        assert!((cues[range.start].end_ms as f64) > view.start);
        assert!((cues[range.end - 1].start_ms as f64) < view.end);
        assert!((cues[range.end].start_ms as f64) >= view.end);

        let mut repeated = cues.clone();
        for cue in &mut repeated {
            cue.start_ms = 0;
        }
        assert_eq!(View::full(1000).visible_range(&repeated), 999..1000);
        assert_eq!(View::full(1000).visible_range(&[]), 0..0);
    }

    #[test]
    fn empty_single_duplicate_and_extreme_inputs_stay_finite() {
        assert_roundtrip(View::following(&[], 0, f64::NAN, f32::NAN));
        assert_roundtrip(View::following(&cues(1, 1000), 1000, f64::INFINITY, -1.0));
        let mut repeated = cues(100, 0);
        for cue in &mut repeated {
            cue.start_ms = 5000;
            cue.end_ms = 5000;
        }
        for focus in [0.0, 4999.0, 5000.0, 5001.0, 10_000.0] {
            assert_roundtrip(View::following(&repeated, 10_000, focus, 320.0));
        }
        let view = View::following(&repeated, i64::MAX, f64::INFINITY, f32::INFINITY);
        assert_roundtrip(view);
        assert_eq!(view.fraction(f64::NAN), 0.0);
        assert_eq!(view.time(f64::NEG_INFINITY), view.start);
        assert_eq!(view.time(f64::INFINITY), view.end);
    }
}
