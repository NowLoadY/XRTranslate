use xrtranslate_engine::{
    collapse_asr_split_words, ends_at_sentence_boundary, is_split_word_pair,
    remove_transcript_overlap,
};
use xrtranslate_protocol::CorpusTermMatch;

const SOFT_CAPTION_SPAN_MS: f64 = 4_500.0;
const HARD_CAPTION_SPAN_MS: f64 = 9_000.0;
const SOURCE_SOFT_LIMIT: usize = 128;
const SENTENCE_ROLL_MIN_UNITS: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RevisableText {
    stable: String,
    hypothesis: String,
}

pub(crate) struct RevisionUpdate {
    pub(crate) text: String,
    pub(crate) hypothesis_start: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HandoffText {
    pub(crate) text: String,
    pub(crate) source_start: usize,
}

impl RevisableText {
    pub(crate) fn new(hypothesis: &str) -> Self {
        let hypothesis = collapse_asr_split_words(hypothesis.trim());
        Self {
            stable: String::new(),
            hypothesis,
        }
    }

    pub(crate) fn update(&mut self, hypothesis: &str, overlap_ratio: f32) -> RevisionUpdate {
        let hypothesis = collapse_asr_split_words(hypothesis.trim());
        let hypothesis = hypothesis.as_str();
        if hypothesis.is_empty() {
            return self.snapshot();
        }
        if self.hypothesis.is_empty() {
            self.hypothesis = hypothesis.to_owned();
            return self.snapshot();
        }

        let previous_tokens = alignment_tokens(&self.hypothesis);
        let next_tokens = alignment_tokens(hypothesis);
        let commit_end = alignment_start(&previous_tokens, &next_tokens).map_or_else(
            || {
                let retained = overlap_ratio.clamp(0.15, 0.8);
                let commit_count =
                    ((previous_tokens.len() as f32) * (1.0 - retained)).floor() as usize;
                previous_tokens
                    .get(commit_count.min(previous_tokens.len().saturating_sub(1)))
                    .map_or(self.hypothesis.len(), |token| token.start)
            },
            |index| previous_tokens[index].start,
        );
        append_joined(&mut self.stable, self.hypothesis[..commit_end].trim());
        self.hypothesis = hypothesis.to_owned();
        self.snapshot()
    }

    pub(crate) fn stable_text(&self) -> &str {
        &self.stable
    }

    pub(crate) fn handoff(&self, next: &str, overlap_ratio: f32) -> HandoffText {
        handoff_text(&self.hypothesis, next, overlap_ratio)
    }

    fn snapshot(&self) -> RevisionUpdate {
        let mut text = self.stable.clone();
        if needs_separator(&text, &self.hypothesis) {
            text.push(' ');
        }
        let hypothesis_start = text.len();
        text.push_str(&self.hypothesis);
        let text = collapse_asr_split_words(&text);
        RevisionUpdate {
            text,
            hypothesis_start,
        }
    }
}

#[derive(Clone)]
struct AlignmentToken {
    normalized: String,
    start: usize,
    end: usize,
}

fn alignment_tokens(text: &str) -> Vec<AlignmentToken> {
    let mut tokens = Vec::new();
    let mut word_start = None;
    for (offset, character) in text.char_indices() {
        if is_compact_script(character) {
            if let Some(start) = word_start.take() {
                tokens.push(AlignmentToken {
                    normalized: text[start..offset].to_lowercase(),
                    start,
                    end: offset,
                });
            }
            tokens.push(AlignmentToken {
                normalized: character.to_string(),
                start: offset,
                end: offset + character.len_utf8(),
            });
        } else if character.is_alphanumeric() || character == '\'' {
            word_start.get_or_insert(offset);
        } else if let Some(start) = word_start.take() {
            tokens.push(AlignmentToken {
                normalized: text[start..offset].to_lowercase(),
                start,
                end: offset,
            });
        }
    }
    if let Some(start) = word_start {
        tokens.push(AlignmentToken {
            normalized: text[start..].to_lowercase(),
            start,
            end: text.len(),
        });
    }
    tokens
}

fn alignment_start(previous: &[AlignmentToken], next: &[AlignmentToken]) -> Option<usize> {
    sequence_alignment(previous, next).map(|alignment| alignment.previous_start)
}

struct SequenceAlignment {
    previous_start: usize,
    previous_end: usize,
    next_start: usize,
    next_end: usize,
    matches: usize,
    longest_run: usize,
}

fn sequence_alignment(
    previous: &[AlignmentToken],
    next: &[AlignmentToken],
) -> Option<SequenceAlignment> {
    const LIMIT: usize = 32;
    let previous_offset = previous.len().saturating_sub(LIMIT);
    let previous = &previous[previous_offset..];
    let next = &next[..next.len().min(LIMIT)];
    let mut lengths = vec![vec![0_u8; next.len() + 1]; previous.len() + 1];
    for left in (0..previous.len()).rev() {
        for right in (0..next.len()).rev() {
            lengths[left][right] = if tokens_aligned(previous, left, next, right) {
                lengths[left + 1][right + 1].saturating_add(1)
            } else {
                lengths[left + 1][right].max(lengths[left][right + 1])
            };
        }
    }
    let score = lengths[0][0] as usize;
    if score == 0 {
        return None;
    }
    let mut left = 0;
    let mut right = 0;
    let mut pairs = Vec::with_capacity(score);
    while left < previous.len() && right < next.len() {
        if tokens_aligned(previous, left, next, right) {
            pairs.push((left, right));
            left += 1;
            right += 1;
            continue;
        }
        if lengths[left + 1][right] >= lengths[left][right + 1] {
            left += 1;
        } else {
            right += 1;
        }
    }
    let &(first_previous, first_next) = pairs.first()?;
    let &(last_previous, last_next) = pairs.last()?;
    let distinctive = pairs
        .iter()
        .any(|&(previous_index, _)| previous[previous_index].normalized.chars().count() >= 4);
    let longest_run = pairs
        .windows(2)
        .fold((1, 1), |(longest, current), pair| {
            let consecutive = pair[1].0 == pair[0].0 + 1 && pair[1].1 == pair[0].1 + 1;
            let current = if consecutive { current + 1 } else { 1 };
            (longest.max(current), current)
        })
        .0;
    (pairs.len() >= 2 || distinctive).then_some(SequenceAlignment {
        previous_start: previous_offset + first_previous,
        previous_end: previous_offset + last_previous,
        next_start: first_next,
        next_end: last_next,
        matches: pairs.len(),
        longest_run,
    })
}

fn tokens_aligned(
    previous: &[AlignmentToken],
    left: usize,
    next: &[AlignmentToken],
    right: usize,
) -> bool {
    let p = &previous[left].normalized;
    let n = &next[right].normalized;
    if p == n {
        return true;
    }
    if (left == previous.len() - 1 || right == 0) && (n.starts_with(p) || p.starts_with(n)) {
        let min_len = p.len().min(n.len());
        if min_len >= 2 {
            return true;
        }
    }
    false
}

pub(crate) fn handoff_text(previous: &str, next: &str, overlap_ratio: f32) -> HandoffText {
    let previous_cleaned = collapse_asr_split_words(previous);
    let next_cleaned = collapse_asr_split_words(next);
    let previous = previous_cleaned.as_str();
    let next = next_cleaned.as_str();

    let previous_tokens = alignment_tokens(previous);
    let next_tokens = alignment_tokens(next);
    if let Some(alignment) = sequence_alignment(&previous_tokens, &next_tokens) {
        let next_span = alignment.next_end.saturating_sub(alignment.next_start) + 1;
        let near_next_start = alignment.next_start <= 2;
        let near_previous_end = previous_tokens
            .len()
            .saturating_sub(alignment.previous_end + 1)
            <= 3;
        let dense = alignment.matches.saturating_mul(2) + 1 >= next_span;
        let expected_overlap =
            ((next_tokens.len() as f32) * overlap_ratio.clamp(0.15, 0.8)).ceil() as usize;
        let bounded = alignment.next_end < expected_overlap.saturating_add(4);
        let strong_phrase = alignment.longest_run >= 3 || (alignment.matches >= 4 && dense);
        if strong_phrase && near_next_start && near_previous_end && bounded {
            let start = skip_boundary_punctuation(next, next_tokens[alignment.next_end].end);
            return handoff_slice(next, start);
        }
    }

    let deduplicated = remove_transcript_overlap(previous, next);
    let start = next.rfind(&deduplicated).unwrap_or_default();
    handoff_slice(next, start)
}

fn skip_boundary_punctuation(text: &str, mut start: usize) -> usize {
    for character in text[start..].chars() {
        if character.is_whitespace()
            || matches!(
                character,
                ',' | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '\u{3001}'
                    | '\u{3002}'
                    | '\u{ff0c}'
                    | '\u{ff1b}'
                    | '\u{ff1a}'
                    | '\u{ff01}'
                    | '\u{ff1f}'
                    | '-'
                    | '\u{2014}'
            )
        {
            start += character.len_utf8();
        } else {
            break;
        }
    }
    start
}

fn handoff_slice(text: &str, mut start: usize) -> HandoffText {
    while let Some(character) = text[start..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        start += character.len_utf8();
    }
    HandoffText {
        text: text[start..].to_owned(),
        source_start: start,
    }
}

fn append_joined(destination: &mut String, addition: &str) {
    if addition.is_empty() {
        return;
    }
    if needs_separator(destination, addition) {
        destination.push(' ');
    }
    destination.push_str(addition);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AppendPosition {
    destination_start: usize,
    source_start: usize,
}

pub(crate) fn append_text(current: &mut String, addition: &str) -> Option<AppendPosition> {
    let deduplicated = remove_transcript_overlap(current, addition);
    let deduplicated = deduplicated.trim();
    if deduplicated.is_empty() {
        return None;
    }
    let source_start = addition.rfind(deduplicated).unwrap_or_default();
    if needs_separator(current, deduplicated) {
        current.push(' ');
    }
    let destination_start = current.len();
    current.push_str(deduplicated);
    Some(AppendPosition {
        destination_start,
        source_start,
    })
}

pub(crate) fn append_segment(current: &mut String, addition: &str) -> Option<AppendPosition> {
    let source = addition;
    let addition = source.trim();
    if addition.is_empty() {
        return None;
    }
    let source_start = source.find(addition).unwrap_or_default();
    if needs_separator(current, addition) {
        current.push(' ');
    }
    let destination_start = current.len();
    current.push_str(addition);
    Some(AppendPosition {
        destination_start,
        source_start,
    })
}

pub(crate) fn append_term_matches(
    current: &mut Vec<CorpusTermMatch>,
    additions: &[CorpusTermMatch],
    position: Option<AppendPosition>,
) {
    let Some(position) = position else {
        return;
    };
    let Ok(destination_start) = u32::try_from(position.destination_start) else {
        return;
    };
    let Ok(source_start) = u32::try_from(position.source_start) else {
        return;
    };
    current.extend(additions.iter().cloned().filter_map(|mut term| {
        if term.start_byte < source_start {
            return None;
        }
        term.start_byte = term
            .start_byte
            .checked_sub(source_start)?
            .checked_add(destination_start)?;
        term.end_byte = term
            .end_byte
            .checked_sub(source_start)?
            .checked_add(destination_start)?;
        Some(term)
    }));
}

pub(crate) fn retain_tail(
    text: &mut String,
    matches: Option<&mut Vec<CorpusTermMatch>>,
    max_chars: usize,
) {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return;
    }
    let byte_start = text
        .char_indices()
        .nth(char_count - max_chars)
        .map_or(text.len(), |(offset, _)| offset);
    text.drain(..byte_start);
    let Some(matches) = matches else {
        return;
    };
    let Ok(byte_start) = u32::try_from(byte_start) else {
        matches.clear();
        return;
    };
    matches.retain_mut(|term| {
        let Some(start) = term.start_byte.checked_sub(byte_start) else {
            return false;
        };
        let Some(end) = term.end_byte.checked_sub(byte_start) else {
            return false;
        };
        term.start_byte = start;
        term.end_byte = end;
        true
    });
}

pub(crate) fn should_roll_caption(source: &str, source_start_ms: f64, next_end_ms: f64) -> bool {
    let span_ms = (next_end_ms - source_start_ms).max(0.0);
    span_ms >= HARD_CAPTION_SPAN_MS
        || (span_ms >= SOFT_CAPTION_SPAN_MS
            && text_units(source) >= SENTENCE_ROLL_MIN_UNITS
            && ends_at_sentence_boundary(source))
        || source.chars().count() >= SOURCE_SOFT_LIMIT
}

fn text_units(text: &str) -> usize {
    let mut units = 0;
    let mut in_word = false;
    for character in text.chars() {
        if is_compact_script(character) {
            units += 1;
            in_word = false;
        } else if character.is_alphanumeric() || character == '\'' {
            if !in_word {
                units += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    units
}

pub(crate) fn needs_separator(current: &str, addition: &str) -> bool {
    if current.is_empty() || addition.is_empty() {
        return false;
    }
    let left_last = current.chars().last();
    let right_first = addition.chars().next();
    let Some((left, right)) = left_last.zip(right_first) else {
        return false;
    };
    if left.is_whitespace()
        || right.is_whitespace()
        || is_compact_script(left)
        || is_compact_script(right)
    {
        return false;
    }
    if left.is_alphabetic() && right.is_alphabetic() {
        let last_word = current
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| !w.is_empty())
            .last()
            .unwrap_or_default();
        let first_word = addition
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| !w.is_empty())
            .next()
            .unwrap_or_default();
        if !last_word.is_empty()
            && !first_word.is_empty()
            && is_split_word_pair(last_word, first_word)
        {
            return false;
        }
    }
    true
}

fn is_compact_script(character: char) -> bool {
    matches!(
        character as u32,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_protocol::CorpusTermSource;

    #[test]
    fn joins_languages_and_removes_window_overlap() {
        let mut english = "We cross the central street.".to_owned();
        append_text(&mut english, "the central street, then turn left.");
        assert_eq!(english, "We cross the central street. then turn left.");

        let mut chinese = "你好，".to_owned();
        append_text(&mut chinese, "世界");
        assert_eq!(chinese, "你好，世界");
    }

    #[test]
    fn adjacent_source_segments_keep_intentional_repetition() {
        let mut text = "go".to_owned();
        append_segment(&mut text, "  go again  ");

        assert_eq!(text, "go go again");
    }

    #[test]
    fn term_offsets_follow_only_the_new_suffix() {
        let mut text = "hello central street".to_owned();
        let position = append_text(&mut text, "central street then Mercy");
        let additions = vec![
            CorpusTermMatch {
                start_byte: 0,
                end_byte: 14,
                text: "central street".into(),
                sources: Vec::<CorpusTermSource>::new(),
            },
            CorpusTermMatch {
                start_byte: 20,
                end_byte: 25,
                text: "Mercy".into(),
                sources: Vec::new(),
            },
        ];
        let mut matches = Vec::new();
        append_term_matches(&mut matches, &additions, position);

        assert_eq!(text, "hello central street then Mercy");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            &text[matches[0].start_byte as usize..matches[0].end_byte as usize],
            "Mercy"
        );

        retain_tail(&mut text, Some(&mut matches), 5);
        assert_eq!(text, "Mercy");
        assert_eq!(matches[0].start_byte, 0);
        assert_eq!(matches[0].end_byte, 5);
    }

    #[test]
    fn caption_handoff_consumes_reworded_overlap_at_a_bubble_boundary() {
        let handoff = handoff_text(
            "I frown, but you use it to end the sentence with a period.",
            "use it to end the sentence with a period. I can only say I admit it.",
            0.5,
        );

        assert_eq!(handoff.text, "I can only say I admit it.");
        assert!(handoff.source_start > 0);
    }

    #[test]
    fn caption_handoff_handles_compact_script_punctuation() {
        let previous = "\u{4f60}\u{5374}\u{7528}\u{79bb}\u{5f00}\u{6765}\u{6253}\u{4e0b}\u{53e5}\u{70b9}\u{3002}";
        let next = "\u{6253}\u{4e0b}\u{53e5}\u{70b9}\u{ff0c}\u{53ea}\u{80fd}\u{8bf4}\u{6211}\u{8ba4}\u{4e86}\u{3002}";
        let handoff = handoff_text(previous, next, 0.5);

        assert_eq!(
            handoff.text,
            "\u{53ea}\u{80fd}\u{8bf4}\u{6211}\u{8ba4}\u{4e86}\u{3002}"
        );
    }

    #[test]
    fn caption_handoff_does_not_remove_an_unanchored_prefix() {
        let next = "I can only say I admit it.";
        let handoff = handoff_text("A different sentence ends here.", next, 0.5);

        assert_eq!(handoff.text, next);
        assert_eq!(handoff.source_start, 0);
    }

    #[test]
    fn caption_handoff_keeps_natural_repetition_without_an_overlap_phrase() {
        let next = "I think this is a new beginning.";
        let handoff = handoff_text("I know that this is the end.", next, 0.5);

        assert_eq!(handoff.text, next);
        assert_eq!(handoff.source_start, 0);
    }

    #[test]
    fn caption_rollover_prefers_sentence_edges_but_has_a_hard_limit() {
        assert!(!should_roll_caption("short phrase", 0.0, 5_000.0));
        assert!(should_roll_caption(
            "This is a complete source sentence with enough stable words to roll cleanly.",
            0.0,
            5_000.0
        ));
        assert!(!should_roll_caption("Follow you.", 0.0, 5_000.0));
        assert!(should_roll_caption("long unresolved phrase", 0.0, 9_000.0));
        assert!(!should_roll_caption("unfinished source", 0.0, 5_000.0));
    }

    #[test]
    fn caption_rollover_does_not_treat_a_dotted_abbreviation_as_a_sentence() {
        let abbreviation =
            "This complete caption has enough stable source words but remains entirely O.K.";
        assert!(!should_roll_caption(abbreviation, 0.0, 5_000.0));

        let sentence = "This complete caption has enough stable source words and ends normally with extra context.";
        assert!(should_roll_caption(sentence, 0.0, 5_000.0));
    }

    #[test]
    fn revisable_text_keeps_stable_prefix_and_rewrites_overlap() {
        let mut text = RevisableText::new("we walk across the central street");
        let update = text.update("the central station and turn left", 0.34);
        assert_eq!(
            update.text,
            "we walk across the central station and turn left"
        );
    }

    #[test]
    fn revisable_text_aligns_compact_scripts() {
        let mut text = RevisableText::new("那天我们走了很久");
        let update = text.update("走了很久没有争吵", 0.5);
        assert_eq!(update.text, "那天我们走了很久没有争吵");
    }

    #[test]
    fn revisable_text_repairs_split_words_and_revises_partial_tokens() {
        let mut text = RevisableText::new(
            "So, literally, what reinforcement learning does is it goes to the ones that worked real",
        );
        let update = text.update("the ones that worked really well.", 0.34);
        assert_eq!(
            update.text,
            "So, literally, what reinforcement learning does is it goes to the ones that worked really well."
        );

        let mut split_text = RevisableText::new("the ones that worked real ly");
        let update2 = split_text.update("worked really well.", 0.34);
        assert_eq!(update2.text, "the ones that worked really well.");
    }

    #[test]
    fn caption_handoff_handles_split_words_at_bubble_boundary() {
        let handoff = handoff_text(
            "So, literally, what reinforcement learning does is it goes to the ones that worked real ly well.",
            "really well. Next sentence continues.",
            0.5,
        );
        assert_eq!(handoff.text, "Next sentence continues.");
        assert!(handoff.source_start > 0);
    }

    #[test]
    fn append_text_joins_split_words_seamlessly() {
        let mut text = "worked real".to_owned();
        append_text(&mut text, "ly well");
        assert_eq!(text, "worked really well");
    }
}
