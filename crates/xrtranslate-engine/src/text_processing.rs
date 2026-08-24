//! Text normalization shared by the native ASR and translation pipeline.
//!
//! The functions in this module mirror and extend the ASR and translation
//! processing paths. In particular, translation segmentation retains the original
//! source segment for display while stripping filler edges and validating content
//! across all supported languages (including Cyrillic, Latin script with accents,
//! CJK, Kana, Hangul, etc.) based on the active translation task.

mod sentence_boundary;

pub use sentence_boundary::ends_at_sentence_boundary;
use sentence_boundary::is_translation_boundary;

/// Maximum number of Unicode scalar values held before a comma may split an
/// otherwise unfinished translation segment.
pub const TRANSLATION_SOFT_SEGMENT_LIMIT: usize = 72;

/// Adjacent sentences at or below this content size are translated together.
/// One isolated short sentence remains independent so normal sentence timing
/// and revision behavior do not change.
const ULTRA_SHORT_TRANSLATION_TOKENS: usize = 2;
const ULTRA_SHORT_TRANSLATION_CHARACTERS: usize = 12;

const HARD_TRANSLATION_BOUNDARIES: &[char] =
    &['。', '！', '？', '；', '：', '.', '!', '?', ';', ':'];
const SOFT_TRANSLATION_BOUNDARIES: &[char] = &['，', '、', ','];
const FILLER_WORDS_DEFAULT: &[char] = &['嗯', '啊', '呃', '额', '哦', '噢', '唉', '哎'];
const FILLER_PUNCTUATION: &[char] = &[
    '，', '。', '！', '？', '；', '：', '、', ',', '.', '!', '?', ';', ':', '~', '…', ' ',
];
const STUTTER_CHARACTERS: &[char] = &[
    '我', '你', '他', '她', '它', '这', '那', '对', '是', '有', '没', '好', '啊', '嗯', '哦', '呃',
    '就',
];

/// Reconstructs one authoritative transcript from overlapping ASR windows.
///
/// The prefix which has left the overlap is immutable; only the current
/// hypothesis may be revised. Translation should consume [`Self::text`]
/// instead of translating each overlapping window independently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisableTranscript {
    stable: String,
    hypothesis: String,
}

impl RevisableTranscript {
    pub fn new(hypothesis: &str) -> Self {
        Self {
            stable: String::new(),
            hypothesis: collapse_asr_split_words(hypothesis.trim()),
        }
    }

    pub fn update(&mut self, hypothesis: &str, overlap_ratio: f32) -> String {
        let hypothesis = collapse_asr_split_words(hypothesis.trim());
        if hypothesis.is_empty() {
            return self.text();
        }
        if self.hypothesis.is_empty() {
            self.hypothesis = hypothesis;
            return self.text();
        }

        let previous_tokens = revision_tokens(&self.hypothesis);
        let next_tokens = revision_tokens(&hypothesis);
        let commit_end = revision_alignment_start(&previous_tokens, &next_tokens).map_or_else(
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
        append_revision_text(&mut self.stable, self.hypothesis[..commit_end].trim());
        self.hypothesis = hypothesis;
        self.text()
    }

    pub fn text(&self) -> String {
        let mut text = self.stable.clone();
        append_revision_text(&mut text, &self.hypothesis);
        collapse_asr_split_words(&text)
    }
}

#[derive(Clone)]
struct RevisionToken {
    normalized: String,
    start: usize,
}

fn revision_tokens(text: &str) -> Vec<RevisionToken> {
    let mut tokens = Vec::new();
    let mut word_start = None;
    for (offset, character) in text.char_indices() {
        if is_content_cjk_or_kana(character) || is_hangul(character) {
            if let Some(start) = word_start.take() {
                tokens.push(RevisionToken {
                    normalized: text[start..offset].to_lowercase(),
                    start,
                });
            }
            tokens.push(RevisionToken {
                normalized: character.to_string(),
                start: offset,
            });
        } else if character.is_alphanumeric() || character == '\'' {
            word_start.get_or_insert(offset);
        } else if let Some(start) = word_start.take() {
            tokens.push(RevisionToken {
                normalized: text[start..offset].to_lowercase(),
                start,
            });
        }
    }
    if let Some(start) = word_start {
        tokens.push(RevisionToken {
            normalized: text[start..].to_lowercase(),
            start,
        });
    }
    tokens
}

fn revision_alignment_start(previous: &[RevisionToken], next: &[RevisionToken]) -> Option<usize> {
    const LIMIT: usize = 32;
    let previous_offset = previous.len().saturating_sub(LIMIT);
    let previous = &previous[previous_offset..];
    let next = &next[..next.len().min(LIMIT)];
    let mut lengths = vec![vec![0_u8; next.len() + 1]; previous.len() + 1];
    for left in (0..previous.len()).rev() {
        for right in (0..next.len()).rev() {
            lengths[left][right] = if revision_tokens_match(previous, left, next, right) {
                lengths[left + 1][right + 1].saturating_add(1)
            } else {
                lengths[left + 1][right].max(lengths[left][right + 1])
            };
        }
    }
    if lengths[0][0] == 0 {
        return None;
    }
    let mut left = 0;
    let mut right = 0;
    let mut pairs = Vec::new();
    while left < previous.len() && right < next.len() {
        if revision_tokens_match(previous, left, next, right) {
            pairs.push((left, right));
            left += 1;
            right += 1;
        } else if lengths[left + 1][right] >= lengths[left][right + 1] {
            left += 1;
        } else {
            right += 1;
        }
    }
    let &(first_previous, _) = pairs.first()?;
    let distinctive = pairs
        .iter()
        .any(|&(index, _)| previous[index].normalized.chars().count() >= 4);
    (pairs.len() >= 2 || distinctive).then_some(previous_offset + first_previous)
}

fn revision_tokens_match(
    previous: &[RevisionToken],
    left: usize,
    next: &[RevisionToken],
    right: usize,
) -> bool {
    let previous_is_last = left == previous.len().saturating_sub(1);
    let previous_token = &previous[left].normalized;
    let next_token = &next[right].normalized;
    previous_token == next_token
        || ((previous_is_last || right == 0)
            && (next_token.starts_with(previous_token) || previous_token.starts_with(next_token))
            && previous_token.len().min(next_token.len()) >= 2)
}

fn append_revision_text(destination: &mut String, addition: &str) {
    if addition.is_empty() {
        return;
    }
    if destination
        .chars()
        .last()
        .zip(addition.chars().next())
        .is_some_and(|(left, right)| {
            !left.is_whitespace()
                && !right.is_whitespace()
                && !is_revision_compact(left)
                && !is_revision_compact(right)
        })
    {
        destination.push(' ');
    }
    destination.push_str(addition);
}

fn is_revision_compact(character: char) -> bool {
    is_content_cjk_or_kana(character)
        || is_hangul(character)
        || matches!(character as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// One source span prepared for translation.
///
/// [`translation_text`](Self::translation_text) has leading and trailing
/// filler words removed.  [`source_text`](Self::source_text) retains the
/// trimmed ASR text that should be shown to listeners.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationSegmentPair {
    /// Cleaned text supplied to translation and TTS.
    pub translation_text: String,
    /// Original, trimmed text supplied to the frontend.
    pub source_text: String,
}

/// Splits text at sentence endings and uses a comma only after 72 characters.
///
/// This reproduces the Python `split_translation_segments` behavior, including
/// its final `should_emit_segment(segment, 1)` filter.  Therefore an
/// unterminated tail is deliberately not emitted until it gains an accepted
/// terminal boundary.
pub fn split_translation_segments(text: &str) -> Vec<String> {
    split_translation_segments_internal(text, false)
}

fn split_translation_segments_internal(text: &str, emit_unterminated: bool) -> Vec<String> {
    let value = text.trim();
    if value.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut buffer = Vec::new();
    let characters = value.chars().collect::<Vec<_>>();
    for (index, &character) in characters.iter().enumerate() {
        buffer.push(character);
        if is_translation_boundary(&characters, index) {
            push_translation_segment(&mut segments, &buffer, emit_unterminated);
            buffer.clear();
            continue;
        }

        while buffer.len() >= TRANSLATION_SOFT_SEGMENT_LIMIT {
            let soft_break = buffer
                .iter()
                .rposition(|character| SOFT_TRANSLATION_BOUNDARIES.contains(character));
            let cutoff = soft_break.map_or(TRANSLATION_SOFT_SEGMENT_LIMIT, |index| index + 1);
            push_translation_segment(&mut segments, &buffer[..cutoff], emit_unterminated);
            buffer = trim_start_chars(&buffer[cutoff..]).to_vec();
        }
    }

    push_translation_segment(&mut segments, &buffer, emit_unterminated);
    merge_adjacent_ultra_short_segments(segments)
}

fn merge_adjacent_ultra_short_segments(segments: Vec<String>) -> Vec<String> {
    let mut merged = Vec::with_capacity(segments.len());
    let mut index = 0;
    while index < segments.len() {
        if !is_ultra_short_translation_segment(&segments[index]) {
            merged.push(segments[index].clone());
            index += 1;
            continue;
        }

        let run_start = index;
        while index < segments.len() && is_ultra_short_translation_segment(&segments[index]) {
            index += 1;
        }
        if index - run_start == 1 {
            merged.push(segments[run_start].clone());
            continue;
        }

        let mut combined = String::new();
        for segment in &segments[run_start..index] {
            if !combined.is_empty() && needs_segment_space(&combined, segment) {
                combined.push(' ');
            }
            combined.push_str(segment);
        }
        merged.push(combined);
    }
    merged
}

fn is_ultra_short_translation_segment(segment: &str) -> bool {
    let tokens = content_token_count(segment);
    let characters = segment
        .chars()
        .filter(|character| character.is_alphanumeric())
        .count();
    tokens > 0
        && tokens <= ULTRA_SHORT_TRANSLATION_TOKENS
        && characters <= ULTRA_SHORT_TRANSLATION_CHARACTERS
}

fn needs_segment_space(left: &str, right: &str) -> bool {
    let Some(left) = left
        .chars()
        .rev()
        .find(|character| character.is_alphanumeric())
    else {
        return false;
    };
    let Some(right) = right.chars().find(|character| character.is_alphanumeric()) else {
        return false;
    };
    !(is_content_cjk_or_kana(left)
        || is_hangul(left)
        || is_content_cjk_or_kana(right)
        || is_hangul(right))
}

/// Collapses only obvious adjacent ASR repetitions across all alphabetic and CJK scripts,
/// and merges split English words and subwords created across chunk boundaries or ASR tokenization.
///
/// The transformation is repeated until stable: adjacent words (case-insensitively across
/// alphabetic scripts), punctuation-separated CJK phrases, repeated short CJK phrases,
/// repeated CJK stutter characters, and split subword fragments are collapsed.
pub fn remove_asr_stutters(text: &str) -> String {
    let mut value = text.trim().to_owned();
    if value.is_empty() {
        return value;
    }

    loop {
        let previous = value.clone();
        value = collapse_matches(&previous, repeated_word_end);
        value = collapse_matches(&value, repeated_cjk_phrase_end);
        value = collapse_matches(&value, repeated_cjk_short_phrase_end);
        value = collapse_matches(&value, repeated_cjk_stutter_character_end);
        value = collapse_asr_split_words(&value);
        if value == previous {
            return value.trim().to_owned();
        }
    }
}

/// Removes audio-overlap text repeated at the boundary of two forced ASR
/// chunks. Matching ignores case, whitespace, and punctuation while the
/// returned suffix preserves the current transcript's original spelling.
///
/// A match must contain at least two content tokens, or one token of four or
/// more characters. This avoids deleting intentional short repetitions such
/// as "yes, yes" while still handling a split through a distinctive word.
pub fn remove_transcript_overlap(previous: &str, current: &str) -> String {
    let previous_cleaned = collapse_asr_split_words(previous);
    let current_cleaned = collapse_asr_split_words(current);
    let previous = &previous_cleaned;
    let current = &current_cleaned;

    let previous_tokens = overlap_tokens(previous);
    let current_tokens = overlap_tokens(current);
    let maximum = previous_tokens.len().min(current_tokens.len()).min(24);
    let matched = (1..=maximum).rev().find(|&count| {
        let left = &previous_tokens[previous_tokens.len() - count..];
        let right = &current_tokens[..count];
        let exact_match = left
            .iter()
            .zip(right)
            .all(|(left, right)| left.normalized == right.normalized);
        if exact_match {
            if count >= 2 {
                return true;
            }
            if let Some(token) = left.first() {
                let is_cjk = token.normalized.chars().any(is_content_cjk_or_kana)
                    || token.normalized.chars().any(is_hangul);
                if is_cjk || current_tokens.len() == 1 || token.normalized.chars().count() >= 4 {
                    return true;
                }
            }
        }

        // Subword / concatenated match
        let left_concat: String = left.iter().map(|token| token.normalized.as_str()).collect();
        let right_concat: String = right
            .iter()
            .map(|token| token.normalized.as_str())
            .collect();
        if left_concat == right_concat && (left_concat.chars().count() >= 4 || count >= 2) {
            return true;
        }

        // Trailing prefix match (previous ended with partial word, current completed it)
        if left.len() == right.len() && count >= 1 {
            let prefix_matches = if count > 1 {
                left[..count - 1]
                    .iter()
                    .zip(&right[..count - 1])
                    .all(|(l, r)| l.normalized == r.normalized)
            } else {
                true
            };
            if prefix_matches {
                let last_left = &left[count - 1].normalized;
                let last_right = &right[count - 1].normalized;
                if last_right.starts_with(last_left) {
                    let remainder = &last_right[last_left.len()..];
                    if (count >= 2 && last_left.len() >= 2)
                        || last_left.len() >= 4
                        || is_split_word_pair(last_left, remainder)
                    {
                        return true;
                    }
                }
            }
        }
        false
    });
    let Some(matched) = matched else {
        return current.trim().to_owned();
    };
    let cutoff = current_tokens[matched - 1].end_byte;
    current[cutoff..]
        .trim_start_matches(|character: char| {
            character.is_whitespace() || (!character.is_alphanumeric() && character != '\'')
        })
        .trim()
        .to_owned()
}

/// Removes filler words and filler punctuation from the two edges for a given source language.
pub fn strip_filler_edges_for_lang(text: &str, source_lang: &str) -> String {
    let fillers = filler_words_for_lang(source_lang);
    let mut value = text.trim().to_owned();
    let mut previous = None;
    while !value.is_empty() && previous.as_deref() != Some(value.as_str()) {
        previous = Some(value.clone());
        let characters: Vec<char> = value.chars().collect();
        let prefix_end = filler_prefix_end_custom(&characters, fillers);
        let without_prefix = characters[prefix_end..].iter().collect::<String>();
        let characters: Vec<char> = without_prefix.trim().chars().collect();
        let suffix_start = filler_suffix_start_custom(&characters, fillers);
        value = characters[..suffix_start]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned();
    }
    value
}

/// Removes default filler words and filler punctuation from the two edges.
pub fn strip_filler_edges(text: &str) -> String {
    strip_filler_edges_for_lang(text, "auto")
}

/// Returns whether text becomes empty after [`strip_filler_edges`].
pub fn is_filler_segment(text: &str) -> bool {
    strip_filler_edges(text).is_empty()
}

/// Produces translation text and display text for every emittable segment given a source language.
pub fn translation_segment_pairs_for_text_with_lang(
    text: &str,
    source_lang: &str,
) -> Vec<TranslationSegmentPair> {
    split_translation_segments(text)
        .into_iter()
        .filter_map(|source_text| translation_pair_with_lang(source_text.trim(), source_lang))
        .collect()
}

/// Produces translation segment pairs for a completed ASR chunk given a source language.
pub fn translation_segment_pairs_for_final_text_with_lang(
    text: &str,
    source_lang: &str,
) -> Vec<TranslationSegmentPair> {
    split_translation_segments_internal(text, true)
        .into_iter()
        .filter_map(|source_text| translation_pair_with_lang(&source_text, source_lang))
        .collect()
}

/// Produces bounded live-caption segments from a revisable transcript.
///
/// Completed sentence boundaries and long comma-delimited clauses are emitted
/// as stable display units while the final unterminated tail remains a normal
/// segment that may be replaced by the next ASR revision. The caller keeps the
/// whole returned list as one authoritative snapshot, so segment indices can
/// be reconciled atomically by downstream consumers.
pub fn translation_segment_pairs_for_live_text_with_lang(
    text: &str,
    source_lang: &str,
) -> Vec<TranslationSegmentPair> {
    split_translation_segments_internal(text, true)
        .into_iter()
        .filter_map(|source_text| translation_pair_with_lang(&source_text, source_lang))
        .collect()
}

/// Produces cleaned source strings supplied to the translation model for a given source language.
pub fn translation_segments_for_text_with_lang(text: &str, source_lang: &str) -> Vec<String> {
    translation_segment_pairs_for_text_with_lang(text, source_lang)
        .into_iter()
        .map(|pair| pair.translation_text)
        .collect()
}

/// Produces translation text and display text for every emittable segment.
pub fn translation_segment_pairs_for_text(text: &str) -> Vec<TranslationSegmentPair> {
    translation_segment_pairs_for_text_with_lang(text, "auto")
}

/// Produces translation segments for a completed ASR chunk.
pub fn translation_segment_pairs_for_final_text(text: &str) -> Vec<TranslationSegmentPair> {
    translation_segment_pairs_for_final_text_with_lang(text, "auto")
}

pub fn translation_segment_pairs_for_live_text(text: &str) -> Vec<TranslationSegmentPair> {
    translation_segment_pairs_for_live_text_with_lang(text, "auto")
}

/// Produces only the cleaned source strings supplied to the translation model.
pub fn translation_segments_for_text(text: &str) -> Vec<String> {
    translation_segments_for_text_with_lang(text, "auto")
}

fn push_if_emittable(segments: &mut Vec<String>, characters: &[char]) {
    let segment = characters.iter().collect::<String>();
    let trimmed = segment.trim();
    if !trimmed.is_empty()
        && trimmed.chars().last().is_some_and(|character| {
            matches!(
                character,
                '。' | '，' | ',' | '.' | '!' | '！' | '?' | '？' | ';' | '；' | ':'
            )
        })
    {
        segments.push(trimmed.to_owned());
    }
}

fn push_translation_segment(
    segments: &mut Vec<String>,
    characters: &[char],
    emit_unterminated: bool,
) {
    if emit_unterminated {
        let segment = characters.iter().collect::<String>();
        let segment = segment.trim();
        if !segment.is_empty() {
            segments.push(segment.to_owned());
        }
    } else {
        push_if_emittable(segments, characters);
    }
}

fn translation_pair_with_lang(
    source_text: &str,
    source_lang: &str,
) -> Option<TranslationSegmentPair> {
    let source_text = source_text.trim();
    let translation_text = strip_filler_edges_for_lang(source_text, source_lang);
    (content_token_count(&translation_text) > 0).then(|| TranslationSegmentPair {
        translation_text,
        source_text: source_text.to_owned(),
    })
}

fn filler_words_for_lang(lang: &str) -> &'static [char] {
    let norm = lang.trim().to_lowercase();
    let main_lang = norm.split(['-', '_']).next().unwrap_or("auto");
    match main_lang {
        "zh" | "auto" => FILLER_WORDS_DEFAULT,
        _ => &[],
    }
}

#[derive(Debug)]
struct OverlapToken {
    normalized: String,
    end_byte: usize,
}

fn overlap_tokens(text: &str) -> Vec<OverlapToken> {
    let characters = text.char_indices().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let (start_byte, character) = characters[index];
        if is_content_cjk_or_kana(character) || is_hangul(character) {
            tokens.push(OverlapToken {
                normalized: character.to_lowercase().collect(),
                end_byte: start_byte + character.len_utf8(),
            });
            index += 1;
            continue;
        }
        if character.is_alphanumeric() {
            let mut end = index + 1;
            while end < characters.len()
                && !is_content_cjk_or_kana(characters[end].1)
                && !is_hangul(characters[end].1)
                && (characters[end].1.is_alphanumeric() || characters[end].1 == '\'')
            {
                end += 1;
            }
            let end_byte = characters.get(end).map_or(text.len(), |(byte, _)| *byte);
            tokens.push(OverlapToken {
                normalized: text[start_byte..end_byte].to_lowercase(),
                end_byte,
            });
            index = end;
            continue;
        }
        index += 1;
    }
    tokens
}

fn is_hangul(character: char) -> bool {
    matches!(character as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

fn trim_start_chars(characters: &[char]) -> &[char] {
    let first_non_whitespace = characters
        .iter()
        .position(|character| !character.is_whitespace())
        .unwrap_or(characters.len());
    &characters[first_non_whitespace..]
}

fn collapse_matches(
    text: &str,
    mut find_match: impl FnMut(&[char], usize) -> Option<(usize, usize)>,
) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut collapsed = Vec::with_capacity(characters.len());
    let mut index = 0;
    while index < characters.len() {
        if let Some((unit_end, match_end)) = find_match(&characters, index) {
            collapsed.extend_from_slice(&characters[index..unit_end]);
            index = match_end;
        } else {
            collapsed.push(characters[index]);
            index += 1;
        }
    }
    collapsed.into_iter().collect()
}

fn repeated_word_end(characters: &[char], index: usize) -> Option<(usize, usize)> {
    if (index > 0 && is_word_re_character(characters[index - 1]))
        || !characters
            .get(index)
            .is_some_and(|character| is_word_re_character(*character))
    {
        return None;
    }
    let unit_end = word_unit_end(characters, index);
    let after_separator = word_separator_end(characters, unit_end)?;
    let unit = &characters[index..unit_end];
    let second_end = after_separator.checked_add(unit.len())?;
    if second_end > characters.len()
        || !characters[after_separator..second_end]
            .iter()
            .zip(unit)
            .all(|(right, left)| word_case_equal(*left, *right))
        || characters
            .get(second_end)
            .is_some_and(|character| is_word_re_character(*character))
    {
        return None;
    }
    Some((unit_end, second_end))
}

fn repeated_cjk_phrase_end(characters: &[char], index: usize) -> Option<(usize, usize)> {
    if !characters
        .get(index)
        .is_some_and(|character| is_han(*character))
    {
        return None;
    }
    let mut unit_end = index;
    while unit_end < characters.len() && unit_end - index < 8 && is_han(characters[unit_end]) {
        unit_end += 1;
    }
    let after_separator = separator_end(characters, unit_end, true)?;
    let unit_len = unit_end - index;
    let second_end = after_separator.checked_add(unit_len)?;
    (second_end <= characters.len()
        && characters[after_separator..second_end] == characters[index..unit_end])
        .then_some((unit_end, second_end))
}

fn repeated_cjk_short_phrase_end(characters: &[char], index: usize) -> Option<(usize, usize)> {
    if !characters
        .get(index)
        .is_some_and(|character| is_han(*character))
    {
        return None;
    }
    for length in (2..=6).rev() {
        let unit_end = index + length;
        let second_end = unit_end + length;
        if second_end <= characters.len()
            && characters[index..unit_end]
                .iter()
                .all(|character| is_han(*character))
            && characters[unit_end..second_end] == characters[index..unit_end]
        {
            return Some((unit_end, second_end));
        }
    }
    None
}

fn repeated_cjk_stutter_character_end(characters: &[char], index: usize) -> Option<(usize, usize)> {
    let character = *characters.get(index)?;
    if !STUTTER_CHARACTERS.contains(&character)
        || characters.get(index + 1).copied() != Some(character)
    {
        return None;
    }
    let mut end = index + 2;
    while characters.get(end).copied() == Some(character) {
        end += 1;
    }
    Some((index + 1, end))
}

fn word_unit_end(characters: &[char], index: usize) -> usize {
    let mut end = index;
    while characters
        .get(end)
        .is_some_and(|character| is_word_re_character(*character))
    {
        end += 1;
    }
    if characters.get(end) == Some(&'\'')
        && characters
            .get(end + 1)
            .is_some_and(|character| is_word_re_character(*character))
    {
        end += 1;
        while characters
            .get(end)
            .is_some_and(|character| is_word_re_character(*character))
        {
            end += 1;
        }
    }
    end
}

fn separator_end(characters: &[char], start: usize, punctuation_required: bool) -> Option<usize> {
    let mut end = start;
    while characters
        .get(end)
        .is_some_and(|character| character.is_whitespace())
    {
        end += 1;
    }
    let punctuation_start = end;
    while characters
        .get(end)
        .is_some_and(|character| is_stutter_separator_punctuation(*character))
    {
        end += 1;
    }
    if end != punctuation_start {
        while characters
            .get(end)
            .is_some_and(|character| character.is_whitespace())
        {
            end += 1;
        }
        return Some(end);
    }
    if punctuation_required {
        return None;
    }
    (end > start).then_some(end)
}

fn word_separator_end(characters: &[char], start: usize) -> Option<usize> {
    if let Some(end) = separator_end(characters, start, true) {
        return Some(end);
    }

    let mut end = start;
    while characters
        .get(end)
        .is_some_and(|character| character.is_whitespace())
    {
        end += 1;
    }
    (end > start).then_some(end)
}

fn is_stutter_separator_punctuation(character: char) -> bool {
    matches!(
        character,
        ',' | '，' | '、' | '.' | '!' | '！' | '?' | '？' | ';' | '；' | ':' | '：'
    )
}

fn is_word_re_character(character: char) -> bool {
    character.is_alphabetic() && !is_content_cjk_or_kana(character)
}

fn word_case_equal(left: char, right: char) -> bool {
    left.to_lowercase().collect::<String>() == right.to_lowercase().collect::<String>()
}

fn is_han(character: char) -> bool {
    matches!(character as u32, 0x4E00..=0x9FFF)
}

fn filler_prefix_end_custom(characters: &[char], fillers: &[char]) -> usize {
    let mut index = 0;
    let mut found_filler = false;
    loop {
        while characters
            .get(index)
            .is_some_and(|character| FILLER_PUNCTUATION.contains(character))
        {
            index += 1;
        }
        if characters
            .get(index)
            .is_some_and(|character| fillers.contains(character))
        {
            found_filler = true;
            index += 1;
        } else {
            return if found_filler { index } else { 0 };
        }
    }
}

fn filler_suffix_start_custom(characters: &[char], fillers: &[char]) -> usize {
    let mut index = characters.len();
    let mut found_filler = false;
    loop {
        while index > 0 && FILLER_PUNCTUATION.contains(&characters[index - 1]) {
            index -= 1;
        }
        if index > 0 && fillers.contains(&characters[index - 1]) {
            found_filler = true;
            index -= 1;
        } else {
            return if found_filler {
                index
            } else {
                characters.len()
            };
        }
    }
}

fn content_token_count(text: &str) -> usize {
    let characters: Vec<char> = text.chars().collect();
    let mut count = 0;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if is_content_cjk_or_kana(character) {
            count += 1;
            index += 1;
        } else if is_word_re_character(character) {
            count += 1;
            index = word_unit_end(&characters, index);
        } else if character.is_numeric() || character.is_ascii_digit() {
            count += 1;
            while characters
                .get(index)
                .is_some_and(|next| next.is_numeric() || next.is_ascii_digit())
            {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    count
}

fn is_content_cjk_or_kana(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0x3040..=0x30FF | 0x31F0..=0x31FF
    )
}

/// Returns true if two adjacent alphabetic words should be collapsed into a single word.
pub fn is_split_word_pair(left: &str, right: &str) -> bool {
    let l = left.to_lowercase();
    let r = right.to_lowercase();
    if l.is_empty() || r.is_empty() {
        return false;
    }
    if !l.chars().all(|c| c.is_alphabetic()) || !r.chars().all(|c| c.is_alphabetic()) {
        return false;
    }

    // 1. Non-standalone suffixes attaching to stems of length >= 2
    if l.len() >= 2 {
        match r.as_str() {
            "ly" | "ing" | "ed" | "ment" | "tion" | "sion" | "ness" | "ible" | "able" | "ful"
            | "less" | "ity" | "ive" | "ty" | "teen" | "th" | "eth" | "ward" | "wards" | "ship"
            | "hood" | "ize" | "ise" | "ate" => return true,
            "es" | "s" => {
                if !matches!(
                    l.as_str(),
                    "he" | "she" | "it" | "that" | "what" | "who" | "let" | "there"
                ) {
                    return true;
                }
            }
            "er" | "est" => {
                if matches!(
                    l.as_str(),
                    "fast"
                        | "slow"
                        | "high"
                        | "low"
                        | "big"
                        | "bigg"
                        | "small"
                        | "larg"
                        | "great"
                        | "bett"
                        | "furth"
                        | "earli"
                        | "lat"
                        | "long"
                        | "short"
                        | "old"
                        | "young"
                        | "new"
                        | "hard"
                        | "easi"
                        | "clear"
                        | "simpl"
                        | "strong"
                        | "smart"
                        | "cool"
                        | "warm"
                        | "deep"
                        | "rich"
                        | "poor"
                        | "tall"
                        | "quick"
                        | "dark"
                        | "bright"
                        | "hot"
                        | "hott"
                        | "cold"
                        | "clean"
                        | "fresh"
                        | "tough"
                        | "rough"
                        | "light"
                        | "heavi"
                        | "wid"
                        | "saf"
                        | "fin"
                        | "nic"
                        | "clos"
                ) {
                    return true;
                }
            }
            _ => {}
        }
    }

    // 2. Multi-part compound words & common split phrases
    match (l.as_str(), r.as_str()) {
        ("can", "not")
        | ("every", "thing")
        | ("some", "thing")
        | ("any", "thing")
        | ("no", "thing")
        | ("every", "one")
        | ("some", "one")
        | ("any", "one")
        | ("every", "body")
        | ("some", "body")
        | ("any", "body")
        | ("no", "body")
        | ("every", "where")
        | ("some", "where")
        | ("any", "where")
        | ("no", "where")
        | ("with", "out")
        | ("with", "in")
        | ("in", "side")
        | ("out", "side")
        | ("where", "ever")
        | ("what", "ever")
        | ("when", "ever")
        | ("how", "ever")
        | ("who", "ever")
        | ("which", "ever")
        | ("my", "self")
        | ("your", "self")
        | ("him", "self")
        | ("her", "self")
        | ("it", "self")
        | ("one", "self")
        | ("our", "selves")
        | ("your", "selves")
        | ("them", "selves")
        | ("any", "way")
        | ("any", "more")
        | ("some", "times")
        | ("some", "time")
        | ("some", "how")
        | ("some", "what")
        | ("any", "how")
        | ("over", "all")
        | ("there", "fore")
        | ("where", "as")
        | ("where", "by")
        | ("where", "in")
        | ("mean", "while")
        | ("never", "theless")
        | ("none", "theless")
        | ("never", "the")
        | ("none", "the")
        | ("al", "though")
        | ("al", "ready")
        | ("al", "together")
        | ("al", "most")
        | ("al", "ways")
        | ("al", "so")
        | ("to", "gether")
        | ("be", "cause")
        | ("be", "come")
        | ("be", "came")
        | ("be", "comes")
        | ("be", "coming")
        | ("be", "fore")
        | ("be", "forehand")
        | ("be", "hind")
        | ("be", "low")
        | ("be", "tween")
        | ("be", "yond")
        | ("be", "sides")
        | ("to", "day")
        | ("to", "night")
        | ("to", "morrow")
        | ("under", "stand")
        | ("under", "standing")
        | ("under", "stood")
        | ("reinforce", "ment")
        | ("rein", "forcement")
        | ("rein", "force")
        | ("down", "load")
        | ("up", "load")
        | ("up", "date")
        | ("up", "grade")
        | ("out", "put")
        | ("in", "put")
        | ("feed", "back")
        | ("data", "base")
        | ("data", "set")
        | ("pass", "word")
        | ("key", "board")
        | ("on", "line")
        | ("off", "line")
        | ("back", "end")
        | ("front", "end")
        | ("life", "time")
        | ("time", "line")
        | ("note", "book")
        | ("screen", "shot")
        | ("time", "stamp")
        | ("work", "place")
        | ("work", "space")
        | ("work", "flow")
        | ("work", "load")
        | ("net", "work")
        | ("frame", "work")
        | ("gate", "way")
        | ("hard", "ware")
        | ("soft", "ware")
        | ("firm", "ware")
        | ("middle", "ware")
        | ("sound", "track")
        | ("voice", "print")
        | ("speech", "to")
        | ("text", "to")
        | ("in", "to")
        | ("on", "to")
        | ("up", "on") => return true,
        _ => {}
    }

    // 3. Common prefixes
    match l.as_str() {
        "un" => matches!(
            r.as_str(),
            "able"
                | "known"
                | "certain"
                | "expected"
                | "fortunate"
                | "fortunately"
                | "necessary"
                | "usual"
                | "available"
                | "limited"
                | "defined"
                | "der"
                | "derstand"
                | "derstanding"
                | "derstood"
                | "clear"
                | "safe"
                | "true"
                | "real"
                | "happy"
                | "fair"
                | "like"
                | "equal"
                | "easy"
                | "even"
                | "official"
                | "stable"
                | "sure"
                | "wanted"
                | "used"
                | "touched"
        ),
        "re" => matches!(
            r.as_str(),
            "inforce"
                | "inforcement"
                | "view"
                | "turn"
                | "start"
                | "cognition"
                | "cognize"
                | "peat"
                | "place"
                | "quire"
                | "cover"
                | "covery"
                | "lease"
                | "move"
                | "main"
                | "member"
                | "mind"
                | "port"
                | "quest"
                | "set"
                | "solve"
                | "sult"
                | "sume"
                | "tain"
                | "vise"
                | "vision"
                | "build"
                | "create"
                | "write"
                | "read"
                | "load"
                | "play"
                | "order"
                | "group"
                | "name"
                | "fresh"
                | "open"
                | "send"
                | "try"
        ),
        "dis" => matches!(
            r.as_str(),
            "connect"
                | "appear"
                | "cover"
                | "play"
                | "able"
                | "agree"
                | "card"
                | "charge"
                | "cuss"
                | "cussion"
                | "order"
                | "place"
                | "prove"
                | "tance"
                | "tinct"
                | "close"
                | "count"
                | "like"
                | "miss"
                | "mount"
                | "trust"
        ),
        "inter" => matches!(
            r.as_str(),
            "net" | "face" | "action" | "national" | "active" | "val" | "view" | "sect" | "nal"
        ),
        "sub" => matches!(
            r.as_str(),
            "agent" | "script" | "title" | "titles" | "system" | "set" | "string" | "ject" | "mit"
        ),
        "non" => r.len() >= 3,
        _ => false,
    }
}

/// Collapses split English words and subwords created across chunk boundaries or ASR tokenization.
pub fn collapse_asr_split_words(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }

    let mut result = text.to_owned();
    loop {
        let previous = result.clone();
        result = collapse_split_word_pass(&previous);
        if result == previous {
            break;
        }
    }
    result
}

fn collapse_split_word_pass(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut collapsed = Vec::with_capacity(characters.len());
    let mut i = 0;
    while i < characters.len() {
        if is_word_re_character(characters[i]) {
            let w1_start = i;
            while i < characters.len() && is_word_re_character(characters[i]) {
                i += 1;
            }
            let w1_end = i;
            let w1: String = characters[w1_start..w1_end].iter().collect();

            // Check what follows w1
            let ws_start = i;
            while i < characters.len() && characters[i].is_whitespace() {
                i += 1;
            }
            let ws_end = i;

            if ws_end > ws_start && i < characters.len() && is_word_re_character(characters[i]) {
                let w2_start = i;
                while i < characters.len() && is_word_re_character(characters[i]) {
                    i += 1;
                }
                let w2_end = i;
                let w2: String = characters[w2_start..w2_end].iter().collect();

                if is_split_word_pair(&w1, &w2) {
                    let merged = merge_word_casing(&w1, &w2);
                    collapsed.extend(merged.chars());
                    continue;
                } else {
                    collapsed.extend(&characters[w1_start..ws_end]);
                    i = w2_start;
                    continue;
                }
            } else {
                collapsed.extend(&characters[w1_start..ws_end]);
                continue;
            }
        } else {
            collapsed.push(characters[i]);
            i += 1;
        }
    }
    collapsed.into_iter().collect()
}

fn merge_word_casing(w1: &str, w2: &str) -> String {
    let all_caps = w1.chars().all(char::is_uppercase) && w2.chars().all(char::is_uppercase);
    if all_caps {
        format!("{}{}", w1, w2.to_uppercase())
    } else {
        format!("{}{}", w1, w2.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisable_transcript_builds_one_authoritative_snapshot() {
        let mut transcript =
            RevisableTranscript::new("we prove it by using a high performance numerical trick");
        assert_eq!(
            transcript.update(
                "a high performance computer with extremely powerful computing power",
                0.34,
            ),
            "we prove it by using a high performance computer with extremely powerful computing power"
        );
    }

    #[test]
    fn revisable_transcript_aligns_a_partial_final_word_after_a_leading_token() {
        let mut transcript = RevisableTranscript::new("we need comput");
        assert_eq!(
            transcript.update("the computer power", 0.34),
            "we need the computer power"
        );
    }

    #[test]
    fn revisable_transcript_replaces_a_corrected_cjk_tail() {
        let mut transcript =
            RevisableTranscript::new("证明的过程中，AI只是利用强悍的数分高带技巧。");
        assert_eq!(
            transcript.update("AI只是利用强悍和无比强大的算力。", 0.34),
            "证明的过程中，AI只是利用强悍和无比强大的算力。"
        );
    }

    #[test]
    fn collapses_adjacent_english_and_chinese_stutters_until_stable() {
        assert_eq!(remove_asr_stutters(" yes, YES "), "yes");
        assert_eq!(remove_asr_stutters("oh!oh!oh!"), "oh!");
        assert_eq!(remove_asr_stutters("two two devices"), "two devices");
        assert_eq!(remove_asr_stutters("对，对，对"), "对");
        assert_eq!(remove_asr_stutters("两个两个设备"), "两个设备");
        assert_eq!(remove_asr_stutters("对你好，对你好"), "对你好");
        assert_eq!(remove_asr_stutters("嗯嗯嗯"), "嗯");
        assert_eq!(remove_asr_stutters("yes, no, yes"), "yes, no, yes");
    }

    #[test]
    fn collapses_adjacent_russian_stutters() {
        assert_eq!(remove_asr_stutters(" да, ДА "), "да");
        assert_eq!(remove_asr_stutters("привет, привет"), "привет");
    }

    #[test]
    fn removes_multilingual_overlap_without_deleting_short_repetition() {
        assert_eq!(
            remove_transcript_overlap(
                "We need to cross the central street.",
                "the central street, then turn left."
            ),
            "then turn left."
        );
        assert_eq!(
            remove_transcript_overlap("今天我们去公园", "去公园然后吃饭"),
            "然后吃饭"
        );
        assert_eq!(remove_transcript_overlap("今天天气真好", "好"), "");
        assert_eq!(remove_transcript_overlap("今日はいい天気ですね", "ね"), "");
        assert_eq!(remove_transcript_overlap("ありがとうございます", "す"), "");
        assert_eq!(remove_transcript_overlap("I saw a dog", "dog"), "");
        assert_eq!(
            remove_transcript_overlap("yes", "yes, yes we can"),
            "yes, yes we can"
        );
        assert_eq!(
            remove_transcript_overlap("configuration", "configuration is ready"),
            "is ready"
        );
    }

    #[test]
    fn completed_asr_text_keeps_an_unpunctuated_tail_for_translation() {
        let pairs = translation_segment_pairs_for_final_text("First sentence. unfinished tail");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].source_text, "First sentence.");
        assert_eq!(pairs[1].source_text, "unfinished tail");
        assert_eq!(
            translation_segment_pairs_for_final_text("continuous speech without punctuation")[0]
                .source_text,
            "continuous speech without punctuation"
        );
        let long = format!("{}.", "a".repeat(80));
        let rebuilt = translation_segment_pairs_for_final_text(&long)
            .into_iter()
            .map(|pair| pair.source_text)
            .collect::<String>();
        assert_eq!(rebuilt, long);
    }

    #[test]
    fn russian_text_produces_valid_translation_segment_pairs() {
        let pairs = translation_segment_pairs_for_final_text_with_lang(" сюкаплеет ", "ru");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].source_text, "сюкаплеет");
        assert_eq!(pairs[0].translation_text, "сюкаплеет");

        let multi_pairs =
            translation_segment_pairs_for_final_text_with_lang("Сюжет. Закончено.", "ru");
        assert_eq!(multi_pairs.len(), 1);
        assert_eq!(multi_pairs[0].source_text, "Сюжет. Закончено.");
    }

    #[test]
    fn strips_only_edge_fillers_and_keeps_punctuation_without_fillers() {
        assert_eq!(strip_filler_edges(" 嗯，啊，今天很好。哦！ "), "今天很好");
        assert_eq!(strip_filler_edges("嗯，啊！"), "");
        assert_eq!(strip_filler_edges("..."), "...");
        assert!(is_filler_segment("嗯，啊！"));
        assert!(!is_filler_segment("..."));
        assert!(!is_filler_segment("啊，实际内容。哦"));
    }

    #[test]
    fn preserves_sentence_pairs_but_excludes_filler_only_segments() {
        let pairs = translation_segment_pairs_for_text("嗯，啊，Hello，world。哦！");
        assert_eq!(
            pairs,
            vec![TranslationSegmentPair {
                translation_text: "Hello，world。".into(),
                source_text: "嗯，啊，Hello，world。".into(),
            }]
        );
        assert_eq!(translation_segments_for_text("嗯！"), Vec::<String>::new());
    }

    #[test]
    fn keeps_short_comma_clauses_together_but_uses_comma_after_soft_limit() {
        assert_eq!(
            split_translation_segments("你好，世界。再见！"),
            vec!["你好，世界。", "再见！"]
        );

        let long_sentence = format!("{}，{}。", "a".repeat(30), "b".repeat(50));
        assert_eq!(
            split_translation_segments(&long_sentence),
            vec![
                format!("{}，", "a".repeat(30)),
                format!("{}。", "b".repeat(50))
            ]
        );
    }

    #[test]
    fn merges_only_consecutive_ultra_short_sentences() {
        assert_eq!(
            split_translation_segments("Twenty-two years old. Okay. Fine."),
            vec!["Twenty-two years old.", "Okay. Fine."]
        );
        assert_eq!(
            split_translation_segments("A long enough sentence stays separate. Okay."),
            vec!["A long enough sentence stays separate.", "Okay."]
        );
        assert_eq!(
            split_translation_segments("好。可以。今天的天气非常不错。"),
            vec!["好。可以。", "今天的天气非常不错。"]
        );
        assert_eq!(
            translation_segment_pairs_for_final_text("Okay. fine")[0].source_text,
            "Okay. fine"
        );
    }

    #[test]
    fn dotted_abbreviations_do_not_fragment_translation_segments() {
        assert_eq!(
            split_translation_segments(
                "The status is O.K. and the clock says 3 p.m. today. Next sentence."
            ),
            vec![
                "The status is O.K. and the clock says 3 p.m. today.",
                "Next sentence."
            ]
        );
        assert_eq!(
            split_translation_segments("The U.S. Army recognizes a Ph.D. degree."),
            vec!["The U.S. Army recognizes a Ph.D. degree."]
        );
        assert_eq!(
            split_translation_segments("Everything is O.K."),
            vec!["Everything is O.K."]
        );
    }

    #[test]
    fn structural_period_rules_cover_numbers_domains_initials_and_ellipsis() {
        assert_eq!(
            split_translation_segments(
                "Version 3.14 is hosted at example.org. J. K. Rowling agrees... Next."
            ),
            vec![
                "Version 3.14 is hosted at example.org.",
                "J. K. Rowling agrees...",
                "Next."
            ]
        );
        assert!(!ends_at_sentence_boundary("Everything remains O.K."));
        assert!(!ends_at_sentence_boundary("Meet me at 3 p.m."));
        assert!(ends_at_sentence_boundary("Everything remains okay."));
        assert!(ends_at_sentence_boundary("Wait..."));
    }

    #[test]
    fn ambiguous_abbreviation_followed_by_a_capital_is_kept_with_context() {
        assert_eq!(
            split_translation_segments("Meet at 5 p.m. Tomorrow we leave."),
            vec!["Meet at 5 p.m. Tomorrow we leave."]
        );
    }

    #[test]
    fn matches_python_terminal_boundary_filtering() {
        assert_eq!(
            split_translation_segments("unterminated tail"),
            Vec::<String>::new()
        );
        assert_eq!(
            split_translation_segments("full-width colon："),
            Vec::<String>::new()
        );
        assert_eq!(
            split_translation_segments("ascii colon:"),
            vec!["ascii colon:"]
        );
    }

    #[test]
    fn collapses_split_english_words_and_subwords() {
        assert_eq!(
            collapse_asr_split_words("worked real ly well"),
            "worked really well"
        );
        assert_eq!(
            collapse_asr_split_words(
                "So, literally, what reinforcement learning does is it goes to the ones that worked real ly well."
            ),
            "So, literally, what reinforcement learning does is it goes to the ones that worked really well."
        );
        assert_eq!(
            collapse_asr_split_words("reinforce ment learn ing"),
            "reinforcement learning"
        );
        assert_eq!(collapse_asr_split_words("nine ty-seven"), "ninety-seven");
        assert_eq!(
            collapse_asr_split_words("every thing can not be done with out you"),
            "everything cannot be done without you"
        );
        assert_eq!(
            collapse_asr_split_words("un der stand ing"),
            "understanding"
        );
        assert_eq!(collapse_asr_split_words("REAL LY GOOD"), "REALLY GOOD");
        assert_eq!(collapse_asr_split_words("Real ly Good"), "Really Good");
    }

    #[test]
    fn removes_transcript_overlap_with_split_and_partial_words() {
        assert_eq!(
            remove_transcript_overlap("So it worked real", "really well, then we left."),
            "well, then we left."
        );
        assert_eq!(
            remove_transcript_overlap(
                "the ones that worked real ly",
                "really well, and then we turned."
            ),
            "well, and then we turned."
        );
        assert_eq!(
            remove_transcript_overlap("reinforce ment", "reinforcement learning"),
            "learning"
        );
    }
}
