use std::{collections::VecDeque, time::Instant};

use crate::presentation::speaker::compact_speaker_label;

use super::{
    runtime::{OscFormatMode, OscInputSource, OscMessageSeparator, OscSettings},
    sys_info::SystemMetrics,
};

#[derive(Clone)]
pub(super) struct HistoryMessage {
    pub(super) stream_id: u64,
    pub(super) source_kind: OscInputSource,
    pub(super) source: String,
    pub(super) translated: String,
    pub(super) speaker_id: String,
    pub(super) expires_at: Instant,
}

#[derive(Clone)]
pub(super) struct ManualMessage {
    pub(super) text: String,
    pub(super) expires_at: Instant,
}

pub(super) fn build_chatbox_text(
    history: &[HistoryMessage],
    live: &[HistoryMessage],
    manual_message: Option<&ManualMessage>,
    settings: &OscSettings,
    metrics: &SystemMetrics,
) -> String {
    let mut entries = history
        .iter()
        .chain(live.iter())
        .cloned()
        .collect::<VecDeque<_>>();
    while entries.len() > 9 {
        entries.pop_front();
    }

    if let Some(manual) = manual_message {
        let manual_raw = manual.text.trim();
        if !manual_raw.is_empty() {
            let manual_text = fit_prefixed_text(
                settings.prefix_for(OscInputSource::Typing),
                manual_raw,
                settings.max_text_length,
            );
            let manual_len = manual_text.chars().count();
            if entries.is_empty() || manual_len >= settings.max_text_length {
                return manual_text;
            }
            let available_for_asr = settings.max_text_length.saturating_sub(manual_len + 1);
            if available_for_asr == 0 {
                return manual_text;
            }
            let asr_text = fit_asr_entries(&mut entries, available_for_asr, settings);
            if asr_text.is_empty() {
                return manual_text;
            }
            return format!("{asr_text}\n{manual_text}");
        }
    }

    // Header and footer exist only while live messages remain and no manual message is active.
    if entries.is_empty() {
        return String::new();
    }

    let prefix = settings.header_config.render_text(metrics);
    let suffix = settings.footer_config.render_text(metrics);

    while let Some(first) = entries.front() {
        let combined = compose_chatbox(&prefix, &render_entries(entries.iter(), settings), &suffix);

        if combined.chars().count() <= settings.max_text_length {
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
    entries: &mut VecDeque<HistoryMessage>,
    limit: usize,
    settings: &OscSettings,
) -> String {
    while let Some(first) = entries.front() {
        let rendered = render_entries(entries.iter(), settings);
        if rendered.chars().count() <= limit {
            return rendered;
        }
        if entries.len() > 1 {
            entries.pop_front();
        } else {
            let rendered = render_entry(first, settings);
            let label = entry_prefix(first, settings);
            return if !label.is_empty() && rendered.starts_with(&label) {
                fit_prefixed_text(&label, &rendered[label.len()..], limit)
            } else {
                trim_text(&rendered, limit)
            };
        }
    }
    String::new()
}

fn render_entries<'a>(
    entries: impl Iterator<Item = &'a HistoryMessage>,
    settings: &OscSettings,
) -> String {
    if settings.format_mode == OscFormatMode::TargetOnly {
        return entries
            .map(|entry| render_entry(entry, settings))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(settings.message_separator.value());
    }
    if settings.message_separator == OscMessageSeparator::NewLine {
        return entries
            .map(|entry| render_entry(entry, settings))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(OscMessageSeparator::NewLine.value());
    }

    let mut sources = Vec::new();
    let mut targets = Vec::new();
    for entry in entries {
        let source = sanitize_chatbox_segment(&entry.source);
        let target = sanitize_chatbox_segment(&entry.translated);
        let speaker = settings
            .show_speaker_number
            .then(|| compact_speaker_label(&entry.speaker_id))
            .flatten();

        if !source.is_empty() && !target.is_empty() && source != target {
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
    }

    let sources = sources.join(" ");
    let targets = targets.join(" ");
    let (first, second, separator) = match settings.format_mode {
        OscFormatMode::BilingualTargetFirst => (targets, sources, "\n"),
        OscFormatMode::BilingualSourceFirst => (sources, targets, "\n"),
        OscFormatMode::Inline => (sources, targets, " | "),
        OscFormatMode::TargetOnly => unreachable!(),
    };
    [first, second]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(separator)
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

pub(super) fn render_entry(entry: &HistoryMessage, settings: &OscSettings) -> String {
    let source = sanitize_chatbox_segment(&entry.source);
    let translated = sanitize_chatbox_segment(&entry.translated);

    let core_text = match settings.format_mode {
        OscFormatMode::TargetOnly => {
            if !translated.is_empty() {
                translated
            } else {
                source
            }
        }
        OscFormatMode::BilingualTargetFirst => {
            if !source.is_empty() && !translated.is_empty() && source != translated {
                format!("{}\n{}", translated, source)
            } else if !translated.is_empty() {
                translated
            } else {
                source
            }
        }
        OscFormatMode::Inline => {
            if !source.is_empty() && !translated.is_empty() && source != translated {
                format!("{} | {}", source, translated)
            } else if !translated.is_empty() {
                translated
            } else {
                source
            }
        }
        OscFormatMode::BilingualSourceFirst => {
            if !source.is_empty() && !translated.is_empty() && source != translated {
                format!("{}\n{}", source, translated)
            } else if !translated.is_empty() {
                translated
            } else {
                source
            }
        }
    };

    if core_text.is_empty() {
        return String::new();
    }

    let prefix = prefixed_label(settings.prefix_for(entry.source_kind));
    let decorated = if prefix.is_empty() {
        core_text
    } else {
        format!("{prefix}{core_text}")
    };

    if settings.show_speaker_number
        && let Some(label) = compact_speaker_label(&entry.speaker_id)
    {
        format!("[{label}] {decorated}")
    } else {
        decorated
    }
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
    let content = if !entry_label.is_empty() && rendered.starts_with(&entry_label) {
        fit_prefixed_text(&entry_label, &rendered[entry_label.len()..], content_limit)
    } else {
        trim_text(&rendered, content_limit)
    };
    compose_chatbox(prefix, &content, suffix)
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::plugins::osc::runtime::{BannerConfig, BannerContentType};

    fn history_message(expires_at: Instant, text: &str) -> HistoryMessage {
        HistoryMessage {
            stream_id: 0,
            source_kind: OscInputSource::Unknown,
            source: text.into(),
            translated: String::new(),
            speaker_id: String::new(),
            expires_at,
        }
    }

    #[test]
    fn messages_are_compacted_by_language_and_evicted_as_pairs() {
        let now = Instant::now();
        let mut first = history_message(now + Duration::from_secs(10), "first source");
        first.translated = "first target".into();
        let mut second = history_message(now + Duration::from_secs(10), "second source");
        second.translated = "second target".into();
        let history = vec![first, second];
        let metrics = SystemMetrics::default();
        let mut settings = OscSettings {
            format_mode: OscFormatMode::BilingualSourceFirst,
            ..OscSettings::default()
        };

        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first source\nfirst target\nsecond source\nsecond target"
        );
        settings.message_separator = OscMessageSeparator::Space;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first source second source\nfirst target second target"
        );
        settings.format_mode = OscFormatMode::BilingualTargetFirst;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first target second target\nfirst source second source"
        );
        settings.format_mode = OscFormatMode::Inline;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first source second source | first target second target"
        );
        settings.max_text_length = 39;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "second source | second target"
        );
        settings.format_mode = OscFormatMode::TargetOnly;
        settings.max_text_length = 144;
        settings.message_separator = OscMessageSeparator::NewLine;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first target\nsecond target"
        );
        settings.message_separator = OscMessageSeparator::Space;
        assert_eq!(
            build_chatbox_text(&history, &[], None, &settings, &metrics),
            "first target second target"
        );
    }

    #[test]
    fn long_single_message_is_trimmed_without_silently_dropping_banners() {
        let now = Instant::now();
        let history = vec![history_message(now + Duration::from_secs(10), "0123456789")];
        let settings = OscSettings {
            max_text_length: 12,
            header_config: BannerConfig {
                content_type: BannerContentType::CustomText,
                custom_text: "H".into(),
                show_device_name: false,
            },
            footer_config: BannerConfig {
                content_type: BannerContentType::CustomText,
                custom_text: "F".into(),
                show_device_name: false,
            },
            ..OscSettings::default()
        };

        let text = build_chatbox_text(&history, &[], None, &settings, &SystemMetrics::default());
        assert_eq!(text, "H\n23456789\nF");
        assert_eq!(text.chars().count(), settings.max_text_length);
    }

    #[test]
    fn manual_message_occupies_bottom_and_shrinks_asr_space_without_banners() {
        let now = Instant::now();
        let history = vec![history_message(now + Duration::from_secs(10), "hello ASR")];
        let settings = OscSettings {
            max_text_length: 40,
            header_config: BannerConfig {
                content_type: BannerContentType::CustomText,
                custom_text: "HEADER".into(),
                show_device_name: false,
            },
            footer_config: BannerConfig {
                content_type: BannerContentType::CustomText,
                custom_text: "FOOTER".into(),
                show_device_name: false,
            },
            ..OscSettings::default()
        };
        let metrics = SystemMetrics::default();

        // 1. Without manual message: Header and footer are shown
        let normal_text = build_chatbox_text(&history, &[], None, &settings, &metrics);
        assert!(normal_text.contains("HEADER"));
        assert!(normal_text.contains("FOOTER"));
        assert!(normal_text.contains("hello ASR"));

        // 2. With manual message: Header and footer are suppressed, manual message is at bottom with the typing prefix
        let manual = ManualMessage {
            text: "typing note".into(),
            expires_at: now + Duration::from_secs(10),
        };
        let combined = build_chatbox_text(&history, &[], Some(&manual), &settings, &metrics);
        assert!(!combined.contains("HEADER"));
        assert!(!combined.contains("FOOTER"));
        assert_eq!(combined, "hello ASR\nTXT typing note");

        // 3. When manual message takes most space, ASR space shrinks accordingly
        let tight_settings = OscSettings {
            max_text_length: 20,
            ..settings
        };
        let tight_combined =
            build_chatbox_text(&history, &[], Some(&manual), &tight_settings, &metrics);
        assert_eq!(tight_combined, "ASR\nTXT typing note");
        assert!(tight_combined.chars().count() <= 20);
    }

    #[test]
    fn speaker_number_prefix_uses_the_assigned_voiceprint_id_and_can_be_disabled() {
        let mut entry = history_message(Instant::now(), "hello");
        entry.speaker_id = "speaker-02".into();
        let mut settings = OscSettings::default();

        assert_eq!(render_entry(&entry, &settings), "hello");
        settings.show_speaker_number = true;
        assert_eq!(render_entry(&entry, &settings), "[S2] hello");

        entry.speaker_id = "speaker-unknown".into();
        assert_eq!(render_entry(&entry, &settings), "[S?] hello");
    }

    #[test]
    fn source_prefixes_are_included_in_the_same_length_budget() {
        let mut entry = history_message(Instant::now(), "0123456789");
        entry.source_kind = OscInputSource::Microphone;
        let settings = OscSettings {
            microphone_prefix: "🎙️".into(),
            max_text_length: 8,
            ..OscSettings::default()
        };
        let text = build_chatbox_text(&[entry], &[], None, &settings, &SystemMetrics::default());
        assert_eq!(text, "🎙️ 56789");
        assert!(text.chars().count() <= settings.max_text_length);
    }

    #[test]
    fn typing_prefix_is_used_for_direct_manual_messages() {
        let settings = OscSettings {
            typing_prefix: "[chat]".into(),
            max_text_length: 12,
            ..OscSettings::default()
        };
        let manual = ManualMessage {
            text: "hello world".into(),
            expires_at: Instant::now(),
        };
        let text = build_chatbox_text(
            &[],
            &[],
            Some(&manual),
            &settings,
            &SystemMetrics::default(),
        );
        assert_eq!(text, "[chat] world");
        assert!(text.chars().count() <= settings.max_text_length);
    }

    #[test]
    fn removes_chinese_periods_and_trailing_non_chat_punctuation() {
        // Chinese period removal
        assert_eq!(sanitize_chatbox_segment("好的。"), "好的");
        assert_eq!(
            sanitize_chatbox_segment("好的。我知道了。"),
            "好的 我知道了"
        );
        assert_eq!(
            sanitize_chatbox_segment("好的，我知道了。"),
            "好的，我知道了"
        );
        assert_eq!(
            sanitize_chatbox_segment("好的。 我知道了。"),
            "好的 我知道了"
        );
        assert_eq!(sanitize_chatbox_segment("こんにちは｡"), "こんにちは");

        // Western period removal
        assert_eq!(sanitize_chatbox_segment("Hello."), "Hello");
        assert_eq!(
            sanitize_chatbox_segment("I am coming home."),
            "I am coming home"
        );
        assert_eq!(sanitize_chatbox_segment("Wait..."), "Wait");

        // Trailing commas, semicolons, colons
        assert_eq!(sanitize_chatbox_segment("你好，"), "你好");
        assert_eq!(sanitize_chatbox_segment("Hello, "), "Hello");
        assert_eq!(sanitize_chatbox_segment("First;"), "First");
        assert_eq!(sanitize_chatbox_segment("Title:"), "Title");

        // Preserves expressive chat punctuation
        assert_eq!(sanitize_chatbox_segment("真的吗？"), "真的吗？");
        assert_eq!(sanitize_chatbox_segment("Really?"), "Really?");
        assert_eq!(sanitize_chatbox_segment("太棒了！"), "太棒了！");
        assert_eq!(sanitize_chatbox_segment("Awesome!"), "Awesome!");
        assert_eq!(sanitize_chatbox_segment("好的~"), "好的~");

        // Multi-script full stops (Devanagari, Arabic, Armenian, Ethiopic, Myanmar, Tibetan)
        assert_eq!(sanitize_chatbox_segment("नमस्ते।"), "नमस्ते");
        assert_eq!(sanitize_chatbox_segment("आप कैसे हैं?"), "आप कैसे हैं?");
        assert_eq!(sanitize_chatbox_segment("Tiếng Việt!"), "Tiếng Việt!");
        assert_eq!(sanitize_chatbox_segment("مرحبا۔"), "مرحبا");
        assert_eq!(sanitize_chatbox_segment("Բարև։"), "Բարև");
        assert_eq!(sanitize_chatbox_segment("ሰላም።"), "ሰላም");
        assert_eq!(sanitize_chatbox_segment("မင်္ဂလာပါ။"), "မင်္ဂလာပါ");
    }

    #[test]
    fn render_entry_cleans_translation_segments_before_output() {
        let mut entry = history_message(Instant::now(), "I am going to sleep.");
        entry.translated = "我要去睡觉了。".into();
        let settings = OscSettings::default();

        assert_eq!(
            render_entry(&entry, &settings),
            "I am going to sleep\n我要去睡觉了"
        );
    }
}
