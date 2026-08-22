//! Audio8 prompt-text normalization.

use unicode_categories::UnicodeCategories;

pub(super) fn format_reference_text(text: &str) -> String {
    let text = clean_text(text);
    if has_speaker_tag(&text) {
        text
    } else {
        format!("<|speaker:0|>{text}")
    }
}

pub(super) fn clean_text(text: &str) -> String {
    let filtered = text
        .chars()
        .filter(|character| character.is_whitespace() || !character.is_other())
        .collect::<String>();
    let characters = filtered.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;
    while index < characters.len() {
        if !characters[index].is_whitespace() {
            output.push(characters[index]);
            index += 1;
            continue;
        }
        let begin = index;
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        let contains_line_break = characters[begin..index]
            .iter()
            .any(|character| is_line_break(*character));
        let left = output.chars().next_back();
        let right = characters.get(index).copied();
        if !(contains_line_break && left.is_some_and(is_cjk) && right.is_some_and(is_cjk))
            && !output.is_empty()
            && index < characters.len()
        {
            output.push(' ');
        }
    }
    output
}

pub(super) fn normalize_reference_transcript(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_speaker_tag(text: &str) -> bool {
    let mut remainder = text;
    while let Some(begin) = remainder.find("<|speaker:") {
        let value = &remainder[begin + "<|speaker:".len()..];
        let digits = value.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && value[digits..].starts_with("|>") {
            return true;
        }
        remainder = &value[digits.min(value.len())..];
    }
    false
}

fn is_line_break(character: char) -> bool {
    matches!(
        character,
        '\r' | '\n' | '\u{000b}' | '\u{000c}' | '\u{001c}'
            ..='\u{001e}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x11ff
            | 0x2e80..=0x2fdf
            | 0x3000..=0x303f
            | 0x3040..=0x30ff
            | 0x3100..=0x31ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xa960..=0xa97f
            | 0xac00..=0xd7a3
            | 0xd7b0..=0xd7ff
            | 0xf900..=0xfaff
            | 0xfe30..=0xfe4f
            | 0xff01..=0xff9f
            | 0x20000..=0x2fa1f
    )
}
