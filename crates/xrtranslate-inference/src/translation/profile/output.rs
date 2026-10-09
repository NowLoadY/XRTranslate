use std::collections::HashSet;

use xrtranslate_prompt::{PromptMessage, TranslationPromptContext};

use crate::openai::remove_completion_markers;

pub(super) fn clean_contextual(text: &str) -> String {
    clean_shared(text)
}

pub(super) fn clean_openai_compatible(text: &str) -> String {
    let text = clean_shared(text);
    for label in ["translation:", "translated text:"] {
        if text
            .get(..label.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(label))
        {
            return text[label.len()..].trim().to_owned();
        }
    }
    text
}

fn clean_shared(text: &str) -> String {
    let text = remove_completion_markers(text);
    strip_current_input_artifacts(text.trim())
}

fn strip_current_input_artifacts(text: &str) -> String {
    let mut output = Vec::new();
    for line in text.lines() {
        let normalized = line
            .trim()
            .trim_matches(|character: char| character == '-' || character.is_whitespace())
            .to_ascii_lowercase();
        if normalized == "end current input" {
            break;
        }
        if normalized == "begin current input" || normalized == "current input:" {
            continue;
        }
        output.push(line);
    }
    output.join("\n").trim().to_owned()
}

#[must_use]
pub fn is_probable_translation_context_leak(
    source_text: &str,
    translated_text: &str,
    prompt_context: Option<&str>,
) -> bool {
    let output = translated_text.trim();
    if output.is_empty() {
        return false;
    }

    let folded = output.to_ascii_lowercase();
    const CONTEXT_MARKERS: [&str; 8] = [
        "# translation context",
        "## language order",
        "## terminology",
        "## recent bilingual history",
        "begin reference context",
        "end reference context",
        "begin current input",
        "end current input",
    ];
    if CONTEXT_MARKERS.iter().any(|marker| folded.contains(marker)) {
        return true;
    }

    let glossary_rows = output
        .lines()
        .filter(|line| {
            let cells = line
                .split(',')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .count();
            cells >= 2 && line.chars().count() <= 240
        })
        .take(3)
        .count();
    if glossary_rows >= 3 {
        return true;
    }

    if let Some(context) = prompt_context
        .map(str::trim)
        .filter(|context| !context.is_empty())
    {
        let copied_lines = output
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.chars().count() >= 4 && context.lines().any(|row| row.trim() == *line)
            })
            .take(3)
            .count();
        if copied_lines >= 3 {
            return true;
        }

        let source_chars = source_text.trim().chars().count().max(1);
        let output_chars = output.chars().count();
        if output_chars > (source_chars.saturating_mul(10) + 64).max(192) {
            return true;
        }
    }

    false
}

/// Validates a cleaned translation against the exact messages rendered for
/// this request. Runtime values are removed before prompt-echo comparison so
/// unchanged source text and reference translations do not masquerade as an
/// instruction leak.
pub(in crate::translation) fn translation_output_rejection(
    source_text: &str,
    translated_text: &str,
    prompt_messages: &[PromptMessage],
    prompt_context: &TranslationPromptContext,
) -> Option<&'static str> {
    let reference = prompt_context.reference_text_for_quality_checks();
    if is_probable_translation_context_leak(source_text, translated_text, reference.as_deref()) {
        return Some("copied translation reference context");
    }

    let output = normalized_characters(translated_text);
    if output.len() < 24 {
        return None;
    }

    let mut prompt = normalized_characters(
        &prompt_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    remove_all_sequences(&mut prompt, &normalized_characters(source_text));
    for reference_block in prompt_context.reference_blocks_for_quality_checks() {
        remove_all_sequences(&mut prompt, &normalized_characters(&reference_block));
    }

    if prompt.len() < 24 {
        return None;
    }
    if contains_sequence(&prompt, &output) {
        return Some("echoed a rendered prompt fragment");
    }

    const SHINGLE_WIDTH: usize = 5;
    let prompt_shingles = prompt.windows(SHINGLE_WIDTH).collect::<HashSet<&[char]>>();
    let output_shingles = output.windows(SHINGLE_WIDTH).collect::<Vec<_>>();
    let copied = output_shingles
        .iter()
        .filter(|shingle| prompt_shingles.contains(*shingle))
        .count();
    if copied >= 18 && copied * 100 >= output_shingles.len() * 76 {
        return Some("substantially reproduced the rendered prompt");
    }

    let copied_lines = translated_text
        .lines()
        .map(normalized_characters)
        .filter(|line| line.len() >= 12 && contains_sequence(&prompt, line))
        .collect::<Vec<_>>();
    let copied_characters = copied_lines.iter().map(Vec::len).sum::<usize>();
    if copied_lines.iter().any(|line| line.len() >= 48)
        || (copied_lines.len() >= 2 && copied_characters >= 40)
    {
        return Some("copied multiple rendered prompt lines");
    }

    None
}

fn normalized_characters(text: &str) -> Vec<char> {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn contains_sequence(haystack: &[char], needle: &[char]) -> bool {
    !needle.is_empty()
        && needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn remove_all_sequences(value: &mut Vec<char>, sequence: &[char]) {
    if sequence.len() < 2 || sequence.len() > value.len() {
        return;
    }
    while let Some(start) = value
        .windows(sequence.len())
        .position(|window| window == sequence)
    {
        value.drain(start..start + sequence.len());
    }
}
