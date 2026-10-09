use std::collections::BTreeSet;

use crate::PromptGraphError;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ComposePart {
    Text(String),
    Input(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RenderedPart {
    Text(String),
    Input(String),
}

pub fn compose_input_indexes(text: &str) -> Result<Vec<u8>, PromptGraphError> {
    let mut indexes = BTreeSet::new();
    for part in parse_compose_text(text)? {
        if let ComposePart::Input(index) = part {
            indexes.insert(index);
        }
    }
    Ok(indexes.into_iter().collect())
}

pub(crate) fn render_compose_text(
    text: &str,
    mut value: impl FnMut(u8) -> Option<String>,
) -> Result<String, PromptGraphError> {
    let mut parts = Vec::new();
    for part in parse_compose_text(text)? {
        match part {
            ComposePart::Text(text) => parts.push(RenderedPart::Text(text)),
            ComposePart::Input(index) => {
                let Some(value) = value(index) else {
                    return Err(PromptGraphError::new(format!(
                        "compose input {{{index}}} is not connected"
                    )));
                };
                parts.push(RenderedPart::Input(value));
            }
        }
    }
    if let Some(joined) = render_slot_join(&parts) {
        return Ok(joined);
    }
    Ok(parts
        .into_iter()
        .map(|part| match part {
            RenderedPart::Text(text) | RenderedPart::Input(text) => text,
        })
        .collect())
}

fn render_slot_join(parts: &[RenderedPart]) -> Option<String> {
    if parts.is_empty() || parts.len().is_multiple_of(2) {
        return None;
    }
    let mut values = Vec::new();
    let mut separator = None::<&str>;
    for (index, part) in parts.iter().enumerate() {
        match (index % 2, part) {
            (0, RenderedPart::Input(value)) => {
                if !value.is_empty() {
                    values.push(value.as_str());
                }
            }
            (1, RenderedPart::Text(text)) if text.trim().is_empty() => {
                if let Some(existing) = separator {
                    if existing != text {
                        return None;
                    }
                } else {
                    separator = Some(text);
                }
            }
            _ => return None,
        }
    }
    Some(values.join(separator.unwrap_or_default()))
}

fn parse_compose_text(text: &str) -> Result<Vec<ComposePart>, PromptGraphError> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '{' if chars.get(index + 1) == Some(&'{') => {
                text.push('{');
                index += 2;
            }
            '}' if chars.get(index + 1) == Some(&'}') => {
                text.push('}');
                index += 2;
            }
            '{' => {
                let Some(digit) = chars.get(index + 1).and_then(|value| value.to_digit(10)) else {
                    return Err(PromptGraphError::new(
                        "compose placeholders must use {0} through {9}; escape a literal brace as {{ or }}",
                    ));
                };
                if digit > u32::from(crate::PromptNodeGraph::MAX_COMPOSE_INPUT_INDEX)
                    || chars.get(index + 2) != Some(&'}')
                {
                    return Err(PromptGraphError::new(
                        "compose placeholders must use {0} through {9}",
                    ));
                }
                if !text.is_empty() {
                    parts.push(ComposePart::Text(std::mem::take(&mut text)));
                }
                parts.push(ComposePart::Input(digit as u8));
                index += 3;
            }
            '}' => {
                return Err(PromptGraphError::new(
                    "unmatched } in compose text; escape a literal brace as }}",
                ));
            }
            value => {
                text.push(value);
                index += 1;
            }
        }
    }
    if !text.is_empty() {
        parts.push(ComposePart::Text(text));
    }
    Ok(parts)
}
