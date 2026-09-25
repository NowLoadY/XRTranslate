use std::collections::HashSet;

use xrtranslate_prompt::{PromptMessage, TranslationPromptContext};

use crate::openai::remove_completion_markers;

pub(super) fn clean_contextual(text: &str) -> String {
    clean_shared(text)
}

pub(super) fn clean_openai_compatible(text: &str) -> String {
    let text = clean_shared(text);
    for label in ["translation:", "translated text:"] {
        if text.len() >= label.len() && text[..label.len()].eq_ignore_ascii_case(label) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_cleanup_only_strips_generic_translation_labels() {
        assert_eq!(
            clean_openai_compatible("Translation: bonjour <|im_end|>"),
            "bonjour"
        );
        assert_eq!(
            clean_contextual("Translation: bonjour <|im_end|>"),
            "Translation: bonjour"
        );
    }

    #[test]
    fn detects_copied_glossary_instead_of_translation() {
        let leaked = "Baptiste,Baptiste\nBastion,Bastion\nBrigitte,Brigitte\nMercy,Mercy";
        assert!(is_probable_translation_context_leak(
            "卢西奥。",
            leaked,
            Some("## Terminology\nBaptiste,Baptiste\nBastion,Bastion")
        ));
        assert!(is_probable_translation_context_leak(
            "hello",
            "## Terminology\nhello,你好",
            None
        ));
        assert!(is_probable_translation_context_leak(
            "你玩莱因哈特吗？",
            "Do you play Reinhardt?\nEND CURRENT INPUT",
            None
        ));
    }

    #[test]
    fn accepts_concise_translation_with_a_terminology_match() {
        assert!(!is_probable_translation_context_leak(
            "I love Mercy.",
            "我喜欢天使。",
            Some("## Terminology\n天使,Mercy")
        ));
    }

    fn messages(contents: &[&str]) -> Vec<PromptMessage> {
        contents
            .iter()
            .map(|content| PromptMessage {
                role: xrtranslate_prompt::PromptMessageRole::User,
                content: (*content).into(),
            })
            .collect()
    }

    #[test]
    fn detects_dynamic_custom_prompt_echo_without_builtin_markers() {
        let prompt = messages(&[
            "Rewrite the current utterance as fluent conversational French. Return only the final sentence.",
            "Current input: hello",
        ]);
        assert_eq!(
            translation_output_rejection(
                "hello",
                "Rewrite the current utterance as fluent conversational French. Return only the final sentence.",
                &prompt,
                &TranslationPromptContext::default(),
            ),
            Some("echoed a rendered prompt fragment")
        );
    }

    #[test]
    fn detects_unicode_prompt_echo_by_normalized_overlap() {
        let instruction = "请把当前内容翻译成自然流畅的日语，只输出最终译文，不要解释翻译过程。";
        let prompt = messages(&[&format!("{instruction}\n当前输入：早上好")]);
        assert!(
            translation_output_rejection(
                "早上好",
                "请把当前内容翻译成自然流畅的日语，只输出最终译文，不要解释。",
                &prompt,
                &TranslationPromptContext::default(),
            )
            .is_some()
        );
    }

    #[test]
    fn excludes_current_input_from_prompt_echo_evidence() {
        let source = "VRChat";
        let prompt = messages(&["Translate into Japanese. Current input: VRChat"]);
        assert_eq!(
            translation_output_rejection(
                source,
                source,
                &prompt,
                &TranslationPromptContext::default(),
            ),
            None
        );
    }

    #[test]
    fn accepts_normal_translation_even_when_it_uses_prompt_vocabulary() {
        let prompt = messages(&[
            "Translate the current input into natural French suitable for a gaming chat. Return only the translation.",
        ]);
        assert_eq!(
            translation_output_rejection(
                "See you in VRChat tonight!",
                "On se retrouve sur VRChat ce soir !",
                &prompt,
                &TranslationPromptContext::default(),
            ),
            None
        );
    }

    #[test]
    fn excludes_runtime_reference_blocks_independently_of_prompt_layout() {
        let expected = "We should meet beside the fountain after the game.";
        let mut context = TranslationPromptContext::default();
        context.recent_turns.push(xrtranslate_prompt::PromptTurn {
            turn_id: None,
            speaker_id: "S1".into(),
            source_language: "Chinese".into(),
            target_language: "English".into(),
            source_text: "游戏结束后在喷泉旁见。".into(),
            translated_text: expected.into(),
        });
        let history = context.reference_blocks_for_quality_checks().remove(0);
        let prompt = messages(&[&format!(
            "Use history only when relevant.\n{history}\nReturn one English sentence."
        )]);

        assert_eq!(
            translation_output_rejection("同上。", expected, &prompt, &context),
            None
        );
    }
}
