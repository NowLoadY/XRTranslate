//! Provider-neutral checks on completed recognition results.

#[must_use]
pub fn is_probable_asr_hallucination(
    text: &str,
    sample_count: usize,
    sample_rate: u32,
    request_context: Option<&str>,
    echo_candidates: &[String],
) -> bool {
    let text = text.trim();
    if text.is_empty() || sample_rate == 0 {
        return false;
    }

    let lowercase = text.to_lowercase();
    let has_context = request_context.is_some_and(|instruction| !instruction.trim().is_empty());
    if lowercase.contains("# asr lexicon")
        || lowercase.contains("asr_lexicon")
        || lowercase.contains("<asr_context")
        || (has_context && lowercase.starts_with("vocabulary:"))
        || (has_context
            && (lowercase.contains("# asr context")
                || lowercase.contains("## language order")
                || lowercase.contains("## terminology")
                || lowercase.contains("## recent bilingual history")))
        || (has_context
            && text.starts_with('{')
            && (lowercase.contains("\"kind\"") || lowercase.contains("\"terms\"")))
    {
        return true;
    }

    let normalized_output = normalized_words(text);
    // Repeated speech is legitimate. History alone cannot prove a prompt echo;
    // the same words must also have been supplied in this recognition request.
    if spoken_unit_count(text) >= 2
        && request_context
            .is_some_and(|context| normalized_words(context).contains(&normalized_output))
        && echo_candidates
            .iter()
            .map(|candidate| normalized_words(candidate))
            .any(|candidate| !candidate.is_empty() && candidate == normalized_output)
    {
        return true;
    }

    let seconds = sample_count as f64 / f64::from(sample_rate);
    let maximum_units = ((seconds * 7.0).ceil() as usize + 1).max(4);
    spoken_unit_count(text) > maximum_units
}

fn normalized_words(text: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_space = true;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            normalized.push(character);
            previous_was_space = false;
        } else if !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }
    normalized.trim().to_owned()
}

fn spoken_unit_count(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for character in text.chars() {
        if is_logographic_or_syllabic(character) {
            count += 1;
            in_word = false;
        } else if character.is_alphanumeric() || character == '\'' {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    count
}

fn is_logographic_or_syllabic(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}
