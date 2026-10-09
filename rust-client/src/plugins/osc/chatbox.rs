use std::{collections::VecDeque, time::Instant};

use crate::presentation::speaker::compact_speaker_label;

use super::{
    runtime::{OscFormatMode, OscInputSource, OscMessageSeparator, OscSettings},
    sys_info::SystemMetrics,
};

const MAX_LINES: usize = 9;

#[derive(Clone)]
pub(super) struct HistoryMessage {
    pub(super) stream_id: u64,
    pub(super) source_kind: OscInputSource,
    pub(super) source: String,
    pub(super) translated: String,
    pub(super) additional_translations: Vec<(String, String)>,
    pub(super) speaker_id: String,
    pub(super) updated_at: Instant,
    pub(super) expires_at: Instant,
}

#[derive(Clone)]
pub(super) struct ManualMessage {
    pub(super) text: String,
    pub(super) updated_at: Instant,
    pub(super) expires_at: Instant,
}

pub(super) fn build_chatbox_text(
    history: &[HistoryMessage],
    live: &[HistoryMessage],
    manual_message: Option<&ManualMessage>,
    settings: &OscSettings,
    metrics: &SystemMetrics,
) -> String {
    let mut entries = history.iter().chain(live.iter()).collect::<VecDeque<_>>();
    // A stale live caption must not take precedence over a newer final result.
    entries
        .make_contiguous()
        .sort_by_key(|entry| entry.updated_at);
    while entries.len() > MAX_LINES {
        entries.pop_front();
    }

    if let Some(manual) = manual_message {
        let manual_raw = manual.text.trim();
        if !manual_raw.is_empty() {
            let (manual_limit, manual_lines) =
                entries
                    .back()
                    .map_or((settings.max_text_length, MAX_LINES), |latest| {
                        if latest.updated_at > manual.updated_at {
                            let latest = render_entry(latest, settings);
                            (
                                settings
                                    .max_text_length
                                    .saturating_sub(latest.chars().count() + 1),
                                MAX_LINES.saturating_sub(latest.lines().count()),
                            )
                        } else {
                            (settings.max_text_length, MAX_LINES)
                        }
                    });
            let manual_text = fit_lines(
                fit_prefixed_text(
                    settings.prefix_for(OscInputSource::Typing),
                    manual_raw,
                    manual_limit,
                ),
                manual_lines,
            );
            if !manual_text.is_empty() {
                let manual_len = manual_text.chars().count();
                if entries.is_empty() || manual_len >= settings.max_text_length {
                    return manual_text;
                }
                let available_for_asr = settings.max_text_length.saturating_sub(manual_len + 1);
                if available_for_asr == 0 {
                    return manual_text;
                }
                let asr_text = fit_asr_entries(
                    &mut entries,
                    available_for_asr,
                    MAX_LINES.saturating_sub(manual_text.lines().count()),
                    settings,
                );
                return if asr_text.is_empty() {
                    manual_text
                } else {
                    format!("{asr_text}\n{manual_text}")
                };
            }
        }
    }

    let prefix = settings.header_config.render_text(metrics);
    let suffix = settings.footer_config.render_text(metrics);

    // In persistent mode the decorations remain visible after message TTLs
    // expire; ordinary messages still disappear from `history`/`live`.
    if entries.is_empty() {
        return if settings.persistent_banners {
            fit_decorations(&prefix, &suffix, settings.max_text_length)
        } else {
            String::new()
        };
    }

    while let Some(first) = entries.front() {
        let combined = compose_chatbox(
            &prefix,
            &render_entries(entries.iter().copied(), settings),
            &suffix,
        );

        if combined.chars().count() <= settings.max_text_length
            && combined.lines().count() <= MAX_LINES
        {
            return combined;
        }
        if entries.len() > 1 {
            entries.pop_front();
        } else {
            return fit_single_entry(first, &prefix, &suffix, settings);
        }
    }

    String::new()
}

fn fit_asr_entries(
    entries: &mut VecDeque<&HistoryMessage>,
    limit: usize,
    line_limit: usize,
    settings: &OscSettings,
) -> String {
    if line_limit == 0 {
        return String::new();
    }
    while let Some(first) = entries.front() {
        let rendered = render_entries(entries.iter().copied(), settings);
        if rendered.chars().count() <= limit && rendered.lines().count() <= line_limit {
            return rendered;
        }
        if entries.len() > 1 {
            entries.pop_front();
        } else {
            if !first.additional_translations.is_empty() {
                return fit_lines(fit_multilingual_entry(first, settings, limit), line_limit);
            }
            let rendered = render_entry(first, settings);
            let label = entry_prefix(first, settings);
            return fit_lines(
                if !label.is_empty() && rendered.starts_with(&label) {
                    fit_prefixed_text(&label, &rendered[label.len()..], limit)
                } else {
                    trim_text(&rendered, limit)
                },
                line_limit,
            );
        }
    }
    String::new()
}

fn render_entries<'a>(
    entries: impl Iterator<Item = &'a HistoryMessage>,
    settings: &OscSettings,
) -> String {
    if settings.message_separator == OscMessageSeparator::NewLine {
        return entries
            .map(|entry| render_entry(entry, settings))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(OscMessageSeparator::NewLine.value());
    }

    let mut sources = Vec::new();
    let mut targets = Vec::new();
    let mut additional: Vec<(String, Vec<String>)> = Vec::new();
    for entry in entries {
        let source = sanitize_chatbox_segment(&entry.source);
        let target = sanitize_chatbox_segment(&entry.translated);
        let speaker = settings
            .show_speaker_number
            .then(|| compact_speaker_label(&entry.speaker_id))
            .flatten();

        if settings.format_mode == OscFormatMode::TargetOnly {
            let text = if target.is_empty() { source } else { target };
            if !text.is_empty() {
                targets.push(with_speaker(
                    &with_source_prefix(&text, entry.source_kind, settings),
                    speaker.as_deref(),
                ));
            }
        } else if !source.is_empty() && !target.is_empty() && source != target {
            sources.push(with_speaker(
                &with_source_prefix(&source, entry.source_kind, settings),
                speaker.as_deref(),
            ));
            targets.push(target);
        } else if let Some(text) = (!target.is_empty())
            .then_some(target)
            .or_else(|| (!source.is_empty()).then_some(source))
        {
            match settings.format_mode {
                OscFormatMode::BilingualTargetFirst => {
                    targets.push(with_speaker(
                        &with_source_prefix(&text, entry.source_kind, settings),
                        speaker.as_deref(),
                    ));
                }
                OscFormatMode::BilingualSourceFirst | OscFormatMode::Inline => {
                    sources.push(with_speaker(
                        &with_source_prefix(&text, entry.source_kind, settings),
                        speaker.as_deref(),
                    ));
                }
                OscFormatMode::TargetOnly => unreachable!(),
            }
        }
        for (language, text) in &entry.additional_translations {
            let text = sanitize_chatbox_segment(text);
            if text.is_empty() {
                continue;
            }
            if let Some((_, texts)) = additional.iter_mut().find(|(key, _)| key == language) {
                texts.push(text);
            } else {
                additional.push((language.clone(), vec![text]));
            }
        }
    }

    let sources = sources.join(" ");
    let mut lines = vec![targets.join(" ")];
    lines.extend(additional.into_iter().map(|(_, texts)| texts.join(" ")));
    match settings.format_mode {
        OscFormatMode::BilingualTargetFirst => lines.push(sources),
        OscFormatMode::BilingualSourceFirst | OscFormatMode::Inline => lines.insert(0, sources),
        OscFormatMode::TargetOnly => {}
    }
    lines
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(if settings.format_mode == OscFormatMode::Inline {
            " | "
        } else {
            "\n"
        })
}

fn with_speaker(text: &str, speaker: Option<&str>) -> String {
    speaker.map_or_else(|| text.to_string(), |label| format!("[{label}] {text}"))
}

fn with_source_prefix(text: &str, source: OscInputSource, settings: &OscSettings) -> String {
    let prefix = prefixed_label(settings.prefix_for(source));
    if prefix.is_empty() {
        text.to_string()
    } else {
        format!("{prefix}{text}")
    }
}

fn compose_chatbox(prefix: &str, content: &str, suffix: &str) -> String {
    [prefix, content, suffix]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fit_decorations(prefix: &str, suffix: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let mut prefix = prefix.to_string();
    let mut suffix = suffix.to_string();
    while decoration_length(&prefix, &suffix) > limit {
        if !suffix.is_empty() {
            suffix = trim_text(&suffix, suffix.chars().count().saturating_sub(1));
        } else if !prefix.is_empty() {
            prefix = trim_text(&prefix, prefix.chars().count().saturating_sub(1));
        } else {
            break;
        }
    }
    fit_lines(compose_chatbox(&prefix, &suffix, ""), MAX_LINES)
}

pub(super) fn render_entry(entry: &HistoryMessage, settings: &OscSettings) -> String {
    let (parts, separator) = entry_parts(entry, settings);
    parts
        .into_iter()
        .map(|(label, text)| format!("{label}{text}"))
        .collect::<Vec<_>>()
        .join(separator)
}

fn entry_parts(
    entry: &HistoryMessage,
    settings: &OscSettings,
) -> (Vec<(String, String)>, &'static str) {
    let source = sanitize_chatbox_segment(&entry.source);
    let translated = sanitize_chatbox_segment(&entry.translated);
    let show_source = !source.is_empty()
        && !translated.is_empty()
        && source != translated
        && settings.format_mode != OscFormatMode::TargetOnly;
    let mut parts = Vec::new();
    if show_source && settings.format_mode != OscFormatMode::BilingualTargetFirst {
        parts.push((String::new(), source.clone()));
    }
    let main = if translated.is_empty() {
        &source
    } else {
        &translated
    };
    if !main.is_empty() {
        parts.push((String::new(), main.clone()));
    }
    for (_, text) in &entry.additional_translations {
        let text = sanitize_chatbox_segment(text);
        if !text.is_empty() {
            parts.push((String::new(), text));
        }
    }
    if show_source && settings.format_mode == OscFormatMode::BilingualTargetFirst {
        parts.push((String::new(), source));
    }
    if let Some((label, _)) = parts.first_mut() {
        label.insert_str(0, &entry_prefix(entry, settings));
    }
    let separator = if settings.format_mode == OscFormatMode::Inline {
        " | "
    } else {
        "\n"
    };
    (parts, separator)
}

fn fit_multilingual_entry(entry: &HistoryMessage, settings: &OscSettings, limit: usize) -> String {
    let (parts, separator) = entry_parts(entry, settings);
    let overhead = parts
        .iter()
        .map(|(label, _)| label.chars().count())
        .sum::<usize>()
        + parts.len().saturating_sub(1) * separator.chars().count();
    if overhead >= limit || parts.is_empty() {
        return render_entry(entry, settings).chars().take(limit).collect();
    }
    // Divide the remaining Unicode character budget among languages, giving
    // unused space from short lines back to the others. Extra targets cannot
    // evict the primary translation merely by being longer.
    let mut lengths = vec![0; parts.len()];
    let needed = parts
        .iter()
        .map(|(_, text)| text.chars().count())
        .collect::<Vec<_>>();
    let mut remaining = limit - overhead;
    while remaining > 0 {
        let mut grew = false;
        for (length, needed) in lengths.iter_mut().zip(&needed) {
            if *length < *needed && remaining > 0 {
                *length += 1;
                remaining -= 1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    parts
        .into_iter()
        .zip(lengths)
        .map(|((label, text), length)| format!("{label}{}", trim_text(&text, length)))
        .collect::<Vec<_>>()
        .join(separator)
}

/// Sanitizes a text segment for informal chatbox display:
/// 1. Removes all fullwidth / halfwidth Chinese periods (`。`, `｡`), replacing intra-sentence periods with spaces if needed.
/// 2. Strips trailing punctuation symbols that do not appear in informal chat typing (e.g. periods, commas, semicolons, colons across scripts).
/// 3. Preserves expressive emotion punctuation (e.g. `?`, `!`, `~`, `？`, `！`, `～`).
pub(super) fn sanitize_chatbox_segment(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut cleaned = String::with_capacity(trimmed.len());
    let mut prev_is_space = false;
    let chars: Vec<char> = trimmed.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        if c == '。' || c == '｡' {
            let has_prev = i > 0 && !chars[i - 1].is_whitespace();
            let has_next = i + 1 < chars.len() && !chars[i + 1].is_whitespace();
            if has_prev && has_next && !prev_is_space {
                cleaned.push(' ');
                prev_is_space = true;
            }
        } else {
            prev_is_space = c.is_whitespace();
            cleaned.push(c);
        }
    }

    strip_trailing_chat_punctuation(&cleaned)
}

pub(super) fn strip_trailing_chat_punctuation(text: &str) -> String {
    let trimmed = text.trim();
    let trimmed_end = trimmed.trim_end_matches(|c: char| {
        matches!(
            c,
            '.' | '。'
                | '｡'
                | ',' | '，'
                | '、' | '､'
                | ';' | '；'
                | ':' | '：'
                | '۔' // Arabic Full Stop
                | '։' // Armenian Full Stop
                | '՝' // Armenian Comma
                | '।' // Devanagari Danda
                | '॥' // Devanagari Double Danda
                | '።' // Ethiopic Full Stop
                | '၊' // Myanmar Comma
                | '။' // Myanmar Full Stop
                | '᠃' // Mongolian Full Stop
                | '᠂' // Mongolian Comma
                | '༌' | '།' | '༎' // Tibetan
        ) || c.is_whitespace()
    });
    trimmed_end.to_string()
}

fn fit_single_entry(
    entry: &HistoryMessage,
    prefix: &str,
    suffix: &str,
    settings: &OscSettings,
) -> String {
    let rendered = render_entry(entry, settings);
    let limit = settings.max_text_length;
    let mut prefix = prefix;
    let mut suffix = suffix;

    // Preserve speech before decorations when space is limited.
    while decoration_length(prefix, suffix) >= limit {
        if !suffix.is_empty() {
            suffix = "";
        } else if !prefix.is_empty() {
            prefix = "";
        } else {
            break;
        }
    }
    let content_limit = limit.saturating_sub(decoration_length(prefix, suffix));
    let entry_label = entry_prefix(entry, settings);
    let content = if !entry.additional_translations.is_empty() {
        fit_multilingual_entry(entry, settings, content_limit)
    } else if !entry_label.is_empty() && rendered.starts_with(&entry_label) {
        fit_prefixed_text(&entry_label, &rendered[entry_label.len()..], content_limit)
    } else {
        trim_text(&rendered, content_limit)
    };
    fit_lines(compose_chatbox(prefix, &content, suffix), MAX_LINES)
}

fn fit_lines(text: String, limit: usize) -> String {
    if limit == 0 {
        String::new()
    } else if text.lines().count() > limit {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        text
    }
}

fn entry_prefix(entry: &HistoryMessage, settings: &OscSettings) -> String {
    let speaker = settings
        .show_speaker_number
        .then(|| compact_speaker_label(&entry.speaker_id))
        .flatten()
        .map(|label| format!("[{label}] "))
        .unwrap_or_default();
    format!(
        "{speaker}{}",
        prefixed_label(settings.prefix_for(entry.source_kind))
    )
}

fn decoration_length(prefix: &str, suffix: &str) -> usize {
    let text = prefix.chars().count() + suffix.chars().count();
    let separators = usize::from(!prefix.is_empty()) + usize::from(!suffix.is_empty());
    text + separators
}

fn trim_text(text: &str, limit: usize) -> String {
    let value = text.trim();
    if limit == 0 {
        return String::new();
    }
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= limit {
        return value.into();
    }
    let tail = chars[chars.len() - limit..].iter().collect::<String>();
    for marker in [
        "。", "！", "？", ".", "!", "?", ";", ":", "；", "，", ",", " ",
    ] {
        if let Some(index) = tail.find(marker) {
            let next = index + marker.len();
            if next < tail.len() {
                return tail[next..].trim_start().into();
            }
        }
    }
    tail.trim_start().into()
}

fn sanitize_prefix(text: &str) -> String {
    let mut value = text
        .trim()
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>();
    value.truncate(
        value
            .char_indices()
            .nth(super::runtime::MAX_PREFIX_LENGTH)
            .map_or(value.len(), |(i, _)| i),
    );
    value
}

fn fit_prefixed_text(prefix: &str, text: &str, limit: usize) -> String {
    let prefix = prefixed_label(prefix);
    let text = sanitize_chatbox_segment(text);
    if prefix.is_empty() {
        return trim_text(&text, limit);
    }
    if limit <= prefix.chars().count() {
        return trim_text(&prefix, limit);
    }
    let content_limit = limit - prefix.chars().count();
    let content = trim_text(&text, content_limit);
    format!("{prefix}{content}")
}

fn prefixed_label(text: &str) -> String {
    let prefix = sanitize_prefix(text);
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix} ")
    }
}
