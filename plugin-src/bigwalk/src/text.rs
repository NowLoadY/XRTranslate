use unicode_segmentation::UnicodeSegmentation;
use xrtranslate_plugin::translation::{MessageFormat, message_parts};
// Game chat already supplies the player's identity. Keep text literal and each
// language on one line; reserve space for every language before shortening any.
pub(crate) fn chat_text<'a>(
    source: &str,
    translated: &str,
    additional: impl IntoIterator<Item = &'a str>,
    format: MessageFormat,
    limit: usize,
) -> String {
    let (parts, separator) = message_parts(
        &plain_text(source),
        &plain_text(translated),
        additional.into_iter().map(plain_text),
        format,
    );
    if parts.is_empty() {
        return String::new();
    }
    let mut remaining = limit.saturating_sub(separator.encode_utf16().count() * (parts.len() - 1));
    let mut budgets = vec![0; parts.len()];
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|&i| parts[i].encode_utf16().count());
    for (position, &i) in order.iter().enumerate() {
        budgets[i] = parts[i]
            .encode_utf16()
            .count()
            .min(remaining / (parts.len() - position));
        remaining -= budgets[i];
    }
    parts
        .iter()
        .zip(budgets)
        .map(|(part, budget)| shorten(part, budget))
        .collect::<Vec<_>>()
        .join(separator)
}

fn plain_text(text: &str) -> String {
    text.chars()
        .take(4096)
        .map(|ch| match ch {
            '<' => '‹',
            '>' => '›',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn shorten(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text.to_owned();
    }
    if limit == 0 {
        return String::new();
    }
    let mut left = limit - 1;
    let mut result = String::new();
    for grapheme in text.graphemes(true) {
        let length = grapheme.encode_utf16().count();
        if length > left {
            break;
        }
        result.push_str(grapheme);
        left -= length;
    }
    result.truncate(result.trim_end().len());
    result.push('…');
    result
}
