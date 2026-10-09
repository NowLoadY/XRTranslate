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
