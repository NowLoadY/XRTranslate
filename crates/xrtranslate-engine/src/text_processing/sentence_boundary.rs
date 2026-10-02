/// Returns whether completed text ends at a reliable sentence boundary.
///
/// A trailing period in a dotted abbreviation is deliberately considered
/// provisional here. Consumers with no lookahead, such as live captions,
/// should wait for more text instead of rolling at `O.K.` or `p.m.`.
pub fn ends_at_sentence_boundary(text: &str) -> bool {
    let text = text.trim_end();
    sentence_end_offsets(text).last().copied() == Some(text.len())
}

/// Byte offsets immediately after reliable sentence endings, including closing
/// quotes or brackets. The unfinished tail is deliberately excluded.
pub fn sentence_end_offsets(text: &str) -> Vec<usize> {
    let characters = text.chars().collect::<Vec<_>>();
    let offsets = text
        .char_indices()
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    let mut endings = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let boundary = match characters[index] {
            '.' => period_is_boundary(&characters, index, false),
            '!' | '?' | '。' | '！' | '？' => true,
            _ => false,
        };
        if boundary {
            index += 1;
            while characters.get(index).is_some_and(|character| {
                matches!(
                    character,
                    '!' | '?'
                        | '。'
                        | '！'
                        | '？'
                        | '"'
                        | '\''
                        | '”'
                        | '’'
                        | '」'
                        | '』'
                        | '）'
                        | ')'
                        | ']'
                        | '】'
                )
            }) {
                index += 1;
            }
            endings.push(offsets.get(index).copied().unwrap_or(text.len()));
        } else {
            index += 1;
        }
    }
    endings
}

pub(super) fn is_translation_boundary(characters: &[char], index: usize) -> bool {
    let character = characters[index];
    if character == '.' {
        return period_is_boundary(characters, index, true);
    }
    super::HARD_TRANSLATION_BOUNDARIES.contains(&character)
}

fn period_is_boundary(
    characters: &[char],
    index: usize,
    abbreviation_at_end_is_boundary: bool,
) -> bool {
    let previous = index.checked_sub(1).map(|index| characters[index]);
    let next = characters.get(index + 1).copied();

    // Only the last dot in an ellipsis can be a boundary. A lowercase
    // continuation (`well... maybe`) keeps it inside the same sentence.
    if previous == Some('.') || next == Some('.') {
        if next == Some('.') {
            return false;
        }
        return next_non_whitespace(characters, index + 1)
            .is_none_or(|character| !character.is_lowercase());
    }

    if previous.is_some_and(|character| character.is_ascii_digit())
        && next.is_some_and(|character| character.is_ascii_digit())
    {
        return false;
    }

    let left_component_len = previous_alphanumeric_len(characters, index);
    if is_internal_dotted_abbreviation_period(characters, index, left_component_len) {
        return false;
    }
    if let (Some(left), Some(right)) = (previous, next)
        && left.is_alphanumeric()
        && right.is_alphanumeric()
        && (left_component_len == 1 || right.is_lowercase())
    {
        // Initialisms (`O.K`), domains, versions, and similar compact tokens.
        return false;
    }

    if is_dotted_abbreviation_ending(characters, index) {
        return abbreviation_at_end_is_boundary
            && next_non_whitespace(characters, index + 1).is_none();
    }

    // A single-letter token before another alphabetic token is normally a
    // personal initial or a spaced initialism (`J. K. Rowling`).
    if left_component_len == 1
        && previous.is_some_and(|character| character.is_alphabetic())
        && next_non_whitespace(characters, index + 1)
            .is_some_and(|character| character.is_alphabetic())
    {
        return false;
    }

    true
}

fn is_internal_dotted_abbreviation_period(
    characters: &[char],
    period: usize,
    left_component_len: usize,
) -> bool {
    if !(1..=3).contains(&left_component_len) {
        return false;
    }
    let right_component_len = characters[period + 1..]
        .iter()
        .take_while(|character| character.is_alphabetic())
        .count();
    (1..=3).contains(&right_component_len)
        && (left_component_len == 1 || right_component_len == 1)
        && characters.get(period + 1 + right_component_len) == Some(&'.')
}

fn previous_alphanumeric_len(characters: &[char], end: usize) -> usize {
    characters[..end]
        .iter()
        .rev()
        .take_while(|character| character.is_alphanumeric())
        .count()
}

fn next_non_whitespace(characters: &[char], start: usize) -> Option<char> {
    characters[start..]
        .iter()
        .copied()
        .find(|character| !character.is_whitespace())
}

/// Recognizes dotted tokens from their shape rather than a vocabulary. Every
/// alphabetic component must be short, covering `O.K.`, `p.m.`, `U.S.A.`, and
/// `Ph.D.` while excluding ordinary dotted prose and most host names.
fn is_dotted_abbreviation_ending(characters: &[char], final_period: usize) -> bool {
    let start = characters[..final_period]
        .iter()
        .rposition(|character| !character.is_alphabetic() && *character != '.')
        .map_or(0, |index| index + 1);
    let token = &characters[start..final_period];
    let mut component_len = 0;
    let mut components = 0;
    let mut saw_period = false;

    for &character in token {
        if character.is_alphabetic() {
            component_len += 1;
            continue;
        }
        if character != '.' || component_len == 0 || component_len > 3 {
            return false;
        }
        components += 1;
        component_len = 0;
        saw_period = true;
    }

    saw_period && components >= 1 && (1..=3).contains(&component_len)
}
