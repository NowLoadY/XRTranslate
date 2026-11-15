//! Bounded text windows for plugin outputs. Budgets count Unicode scalar values;
//! window edges always fall on complete graphemes, including joined emoji.
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

pub struct TextViewport<'a> {
    text: &'a str,
    // Byte and character offsets, including an end sentinel.
    boundaries: Vec<(usize, usize)>,
    budget: usize,
    last_start: usize,
}

impl<'a> TextViewport<'a> {
    pub fn new(text: &'a str, budget: usize) -> Self {
        let mut characters = 0;
        let mut boundaries = Vec::new();
        for (byte, grapheme) in text.grapheme_indices(true) {
            boundaries.push((byte, characters));
            characters += grapheme.chars().count();
        }
        boundaries.push((text.len(), characters));
        let last_start = boundaries
            .partition_point(|(_, offset)| characters - offset > budget)
            .min(boundaries.len().saturating_sub(2));
        Self {
            text,
            boundaries,
            budget,
            last_start,
        }
    }

    pub fn overflows(&self) -> bool {
        self.boundaries.last().unwrap().1 > self.budget
    }

    pub fn start(&self) -> &'a str {
        self.at_grapheme(0)
    }

    /// Start at or after a character offset, without splitting a grapheme or
    /// scrolling past the last full window. Useful for externally timed lyrics.
    pub fn at_offset(&self, characters: usize) -> &'a str {
        let start = self
            .boundaries
            .partition_point(|(_, offset)| *offset < characters);
        self.at_grapheme(start.min(self.last_start))
    }

    /// Keep an externally timed reading position near the center, holding the
    /// first and last full windows. Progress is normalized to the range 0..=1.
    pub fn at_progress(&self, progress: f64) -> &'a str {
        let characters = self.boundaries.last().unwrap().1;
        let offset = (characters as f64 * progress.clamp(0.0, 1.0) - self.budget as f64 / 2.0)
            .max(0.0) as usize;
        self.at_offset(offset.min(characters.saturating_sub(self.budget)))
    }

    /// With a lifetime, traverse the text once and hold both ends within that
    /// lifetime. Without one, advance one grapheme per second and loop, holding
    /// each end for two seconds. The caller owns clocks and reset policy.
    pub fn scroll(&self, elapsed: Duration, lifetime: Option<Duration>) -> &'a str {
        if !self.overflows() || self.budget == 0 {
            return self.start();
        }
        let start = match lifetime {
            Some(lifetime) => {
                let hold = (lifetime / 4).min(Duration::from_secs(2));
                let travel = lifetime.saturating_sub(hold * 2).as_secs_f64();
                let progress = if travel > 0.0 {
                    elapsed.saturating_sub(hold).as_secs_f64() / travel
                } else {
                    0.0
                };
                (progress.clamp(0.0, 1.0) * self.last_start as f64) as usize
            }
            None => {
                let phase = elapsed.as_secs() % (self.last_start as u64 + 4);
                phase.saturating_sub(2).min(self.last_start as u64) as usize
            }
        };
        self.at_grapheme(start)
    }

    fn at_grapheme(&self, mut start: usize) -> &'a str {
        let (_, characters) = self.boundaries[start];
        let mut end = self
            .boundaries
            .partition_point(|(_, offset)| *offset <= characters.saturating_add(self.budget))
            .saturating_sub(1);
        // Trim only whole whitespace graphemes, never the space belonging to a
        // combining sequence. This also keeps blank edges out of lyric windows.
        let whitespace = |index: usize| {
            self.text[self.boundaries[index].0..self.boundaries[index + 1].0]
                .chars()
                .all(char::is_whitespace)
        };
        while start < end && whitespace(start) {
            start += 1;
        }
        while end > start && whitespace(end - 1) {
            end -= 1;
        }
        &self.text[self.boundaries[start].0..self.boundaries[end].0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_obey_character_budgets_without_splitting_graphemes() {
        let text = "甲e\u{301}👩‍💻🇨🇳 \u{301}乙丙";
        let boundaries = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect::<Vec<_>>();
        for budget in 0..=text.chars().count() + 1 {
            let viewport = TextViewport::new(text, budget);
            for offset in 0..=text.chars().count() + 1 {
                let visible = viewport.at_offset(offset);
                assert!(visible.chars().count() <= budget);
                if !visible.is_empty() {
                    let start = visible.as_ptr() as usize - text.as_ptr() as usize;
                    assert!(boundaries.contains(&start));
                    assert!(boundaries.contains(&(start + visible.len())));
                }
            }
        }
        assert_eq!(TextViewport::new("👩‍💻", 2).start(), "");
        assert_eq!(TextViewport::new("", 0).scroll(Duration::MAX, None), "");
    }

    #[test]
    fn timed_scroll_exposes_the_tail_before_expiry_and_short_text_stays_still() {
        let viewport = TextViewport::new("ABCDEFGHIJK", 4);
        let lifetime = Duration::from_secs(12);
        assert_eq!(viewport.scroll(Duration::ZERO, Some(lifetime)), "ABCD");
        assert_eq!(
            viewport.scroll(Duration::from_secs(2), Some(lifetime)),
            "ABCD"
        );
        assert_eq!(
            viewport.scroll(Duration::from_secs(6), Some(lifetime)),
            "DEFG"
        );
        assert_eq!(
            viewport.scroll(Duration::from_secs(10), Some(lifetime)),
            "HIJK"
        );
        assert_eq!(
            viewport.scroll(Duration::from_secs(11), Some(lifetime)),
            "HIJK"
        );
        assert_eq!(TextViewport::new("短句", 4).scroll(lifetime, None), "短句");
    }

    #[test]
    fn external_progress_centers_the_text_and_holds_both_ends() {
        let viewport = TextViewport::new("ABCDEFGHIJKLMNOPQRST", 6);
        for (progress, expected) in [
            (-1.0, "ABCDEF"),
            (0.0, "ABCDEF"),
            (0.5, "HIJKLM"),
            (1.0, "OPQRST"),
            (2.0, "OPQRST"),
        ] {
            assert_eq!(viewport.at_progress(progress), expected);
        }
        assert_eq!(TextViewport::new("短句", 4).at_progress(0.75), "短句");
        assert_eq!(TextViewport::new("", 0).at_progress(1.0), "");
        // The reading position must not cut through an emoji or combining mark.
        assert_eq!(
            TextViewport::new("甲👩‍💻e\u{301}乙", 3).at_progress(0.5),
            "e\u{301}乙"
        );
        assert_eq!(TextViewport::new("👩‍💻", 2).at_progress(1.0), "");
    }

    #[test]
    fn looping_scroll_holds_both_ends_and_restarts() {
        let viewport = TextViewport::new("abcdef", 3);
        let frames = (0..8)
            .map(|second| viewport.scroll(Duration::from_secs(second), None))
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            ["abc", "abc", "abc", "bcd", "cde", "def", "def", "abc"]
        );
    }
}
