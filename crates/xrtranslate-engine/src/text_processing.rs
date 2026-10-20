//! Text normalization shared by the native ASR and translation pipeline.
//!
//! The functions in this module mirror and extend the ASR and translation
//! processing paths. In particular, translation segmentation retains the original
//! source segment for display while stripping filler edges and validating content
//! across all supported languages (including Cyrillic, Latin script with accents,
//! CJK, Kana, Hangul, etc.) based on the active translation task.

mod sentence_boundary;

pub use sentence_boundary::{ends_at_sentence_boundary, sentence_end_offsets};

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
        if overlap_ratio <= 0.0 {
            // A pause can close the audio window without ending the sentence.
            // No audio was repeated, so preserve the entire preceding phrase.
            append_revision_text(&mut self.stable, &self.hypothesis);
            self.hypothesis = hypothesis;
            return self.text();
        }

        let previous_tokens = revision_tokens(&self.hypothesis);
        let next_tokens = revision_tokens(&hypothesis);
        // Audio duration does not tell us which words were repeated. Without
        // a confirmed text overlap, preserve the entire preceding hypothesis.
        let commit_end = revision_alignment_start(&previous_tokens, &next_tokens)
            .map_or(self.hypothesis.len(), |index| previous_tokens[index].start);
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
    let maximum = previous.len().min(next.len()).min(LIMIT);
    // Only a contiguous suffix-to-prefix match can replace existing words.
    // A gapped subsequence can anchor on an earlier repeated word ("more")
    // and silently discard everything between it and the actual overlap.
    (1..=maximum).rev().find_map(|count| {
        let start = previous.len() - count;
        let suffix = &previous[start..];
        let distinctive = count >= 2 || suffix[0].normalized.chars().count() >= 4;
        let matched = suffix
            .iter()
            .zip(next)
            .enumerate()
            .all(|(index, (left, right))| {
                left.normalized == right.normalized
                // The old window may end halfway through a word. Allow the
                // new window to complete it, never to shorten an existing word.
                || (index + 1 == count
                    && left.normalized.chars().count() >= 2
                    && right.normalized.starts_with(&left.normalized))
            });
        (distinctive && matched).then_some(start)
    })
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

/// Produces the same sentence spans for text input and recognized speech.
///
/// Commas do not split a sentence, and short sentences stay separate.
/// An unfinished tail is retained; live callers decide when it is ready to translate.
pub fn translation_segment_pairs_for_text_with_lang(
    text: &str,
    source_lang: &str,
) -> Vec<TranslationSegmentPair> {
    let mut start = 0;
    sentence_end_offsets(text)
        .into_iter()
        .chain(std::iter::once(text.len()))
        .filter_map(|end| {
            let source = &text[start..end];
            start = end;
            translation_pair_with_lang(source.trim(), source_lang)
        })
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

/// Produces only the cleaned source strings supplied to the translation model.
pub fn translation_segments_for_text(text: &str) -> Vec<String> {
    translation_segments_for_text_with_lang(text, "auto")
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
            // Standalone words such as "able", "less" and "ship" are not
            // evidence of a tokenization error ("be able", "spend less").
            "ly" | "ing" | "ed" | "ment" | "tion" | "sion" | "ness" | "ible" | "ful" | "ity"
            | "ive" | "ty" | "th" | "eth" | "ize" | "ise" => return true,
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
mod sentence_segmentation_tests {
    use super::translation_segment_pairs_for_text_with_lang;

    #[test]
    fn standalone_words_are_not_mistaken_for_suffix_fragments() {
        for text in [
            "We should be able to spend less.",
            "They own a ship and she ate lunch.",
            "The next ward has a quiet neighborhood.",
        ] {
            assert_eq!(super::collapse_asr_split_words(text), text);
        }
        assert_eq!(super::collapse_asr_split_words("walk ing"), "walking");
    }

    #[test]
    fn long_sentences_keep_lists_and_dependent_clauses_together() {
        for (language, text) in [
            (
                "en",
                "The workshop teaches careful observation, reliable measurement, and practical problem solving.",
            ),
            (
                "en",
                "Although the delivery arrived later than expected, the staff finished checking every package before closing.",
            ),
            (
                "zh",
                "会议结束以后，我们先核对每一份材料，记录所有需要进一步讨论的问题，再根据负责人的意见调整工作安排，确认每个参与者都理解自己的任务，然后按照已经商定的顺序逐项完成。",
            ),
            (
                "en",
                "The plan has three stages: inspect the equipment; record the measurements; discuss the results with everyone.",
            ),
        ] {
            assert!(text.chars().count() > 72);
            let segments = translation_segment_pairs_for_text_with_lang(text, language);
            assert_eq!(segments.len(), 1, "{text}");
            assert_eq!(segments[0].source_text, text);
        }
    }

    #[test]
    fn sentence_boundaries_preserve_repetition_quotes_and_the_unfinished_tail() {
        let segments = translation_segment_pairs_for_text_with_lang(
            "Wait. Wait. \"Ready?\" Final note without a period",
            "en",
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.source_text.as_str())
                .collect::<Vec<_>>(),
            [
                "Wait.",
                "Wait.",
                "\"Ready?\"",
                "Final note without a period"
            ]
        );
    }

    #[test]
    fn decimals_initials_and_dotted_abbreviations_stay_with_their_sentence() {
        let segments = translation_segment_pairs_for_text_with_lang(
            "The U.S.A. office quoted 3.14 per unit. J. K. Reed accepted. O.K.",
            "en",
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.source_text.as_str())
                .collect::<Vec<_>>(),
            [
                "The U.S.A. office quoted 3.14 per unit.",
                "J. K. Reed accepted.",
                "O.K.",
            ]
        );
    }
}
