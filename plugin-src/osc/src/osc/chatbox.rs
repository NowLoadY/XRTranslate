use crate::text_output::{LyricsText, SystemMetrics, message_parts};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Instant,
};
use xrtranslate_plugin::text::TextViewport;

use super::{
    banners::{self, Banners},
    pet::{Activity, theater::Theater},
};
use crate::compact_speaker_label;

use super::runtime::{
    BannerContentType, ChatboxRegions, OscFormatMode, OscInputSource, OscMessageSeparator,
    OscSettings,
};

const MAX_ENTRIES: usize = 9;

#[cfg(test)]
#[path = "pet/chatbox_tests.rs"]
mod pet_tests;

#[path = "chatbox_lyrics.rs"]
mod idle_lyrics;

#[path = "pet/theater/layout.rs"]
mod idle_theater;

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Block {
    Header,
    Footer,
    Manual(Instant),
    Caption(u64, Instant, usize),
}

struct ScrollState {
    text: String,
    budget: usize,
    started: Instant,
    visible: bool,
}

#[derive(Default)]
pub(super) struct ChatboxRenderer {
    blocks: BTreeMap<Block, ScrollState>,
    banners: Banners,
    theater: Theater,
    pub(super) regions: ChatboxRegions,
}

impl ChatboxRenderer {
    pub(super) fn render(
        &mut self,
        history: &[HistoryMessage],
        live: &[HistoryMessage],
        manual_message: Option<&ManualMessage>,
        settings: &OscSettings,
        metrics: &SystemMetrics,
        lyrics: &LyricsText,
        now: Instant,
    ) -> String {
        self.regions = ChatboxRegions::default();
        self.theater
            .begin_frame(
                [&settings.header_config, &settings.footer_config].map(|banner| {
                    (banner.content_type == BannerContentType::TextPet).then_some(banner.pet)
                }),
            );
        for state in self.blocks.values_mut() {
            state.visible = false;
        }
        let text = compose_frame(
            history,
            live,
            manual_message,
            settings,
            metrics,
            lyrics,
            &mut Frame {
                renderer: self,
                now,
            },
        );
        // Evicted, expired and no-longer-overflowing blocks own no animation.
        self.blocks.retain(|_, state| state.visible);
        self.theater.end_frame();
        text
    }

    pub(super) fn needs_refresh(&self) -> bool {
        !self.blocks.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.blocks.clear();
        self.banners = Banners::default();
        self.theater = Theater::default();
        self.regions = ChatboxRegions::default();
    }

    pub(super) fn pause_ambient(&mut self) {
        self.theater.pause();
    }
}

struct Frame<'a> {
    renderer: &'a mut ChatboxRenderer,
    now: Instant,
}

impl Frame<'_> {
    fn banner(
        &mut self,
        block: Block,
        text: &str,
        budget: usize,
        kind: BannerContentType,
    ) -> String {
        if kind == BannerContentType::TextPet {
            if budget >= text.chars().count() {
                return text.to_owned();
            }
            self.renderer.banners.fit_pet(
                usize::from(block == Block::Footer),
                budget.min(super::pet::MAX_WIDTH),
            )
        } else if kind == BannerContentType::Lyrics {
            TextViewport::new(text, budget).start().to_owned()
        } else {
            self.text(block, text, budget, None)
        }
    }

    fn compose(&mut self, header: &str, body: &str, footer: &str, lines: usize) -> String {
        if lines == 0 {
            return String::new();
        }
        let mut regions = ChatboxRegions {
            header: header.into(),
            body: body.into(),
            footer: footer.into(),
            unified_lyrics: false,
            pet_stage: false,
        };
        let mut text = compose_chatbox(header, body, footer);
        if text.lines().count() > lines {
            // Compact each region before composing so preview boundaries still
            // describe the text sent to the game after enforcing the line limit.
            for region in [&mut regions.header, &mut regions.body, &mut regions.footer] {
                *region = region.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            text = compose_chatbox(&regions.header, &regions.body, &regions.footer);
            if text.lines().count() > lines {
                text = fit_lines(text, lines);
                regions = ChatboxRegions {
                    body: text.clone(),
                    ..Default::default()
                };
            }
        }
        self.renderer.regions = regions;
        text
    }

    fn text(
        &mut self,
        block: Block,
        text: &str,
        budget: usize,
        expires_at: Option<Instant>,
    ) -> String {
        let viewport = TextViewport::new(text, budget);
        if budget == 0 || !viewport.overflows() {
            return viewport.start().to_owned();
        }
        let state = self
            .renderer
            .blocks
            .entry(block)
            .or_insert_with(|| ScrollState {
                text: text.to_owned(),
                budget,
                started: self.now,
                visible: true,
            });
        if state.text != text || state.budget != budget {
            state.text = text.to_owned();
            state.budget = budget;
            state.started = self.now;
        }
        state.visible = true;
        viewport
            .scroll(
                self.now.saturating_duration_since(state.started),
                expires_at.map(|end| end.saturating_duration_since(state.started)),
            )
            .to_owned()
    }
}

fn compose_frame(
    history: &[HistoryMessage],
    live: &[HistoryMessage],
    manual_message: Option<&ManualMessage>,
    settings: &OscSettings,
    metrics: &SystemMetrics,
    lyrics: &LyricsText,
    frame: &mut Frame<'_>,
) -> String {
    let mut entries = history
        .iter()
        .chain(live.iter())
        // Hidden recognition previews must not consume caption space.
        .filter(|entry| !render_entry(entry, settings).is_empty())
        .collect::<VecDeque<_>>();
    // A stale live caption must not take precedence over a newer final result.
    entries
        .make_contiguous()
        .sort_by_key(|entry| entry.updated_at);
    while entries.len() > MAX_ENTRIES {
        entries.pop_front();
    }

    let split_lyrics = settings.header_config.content_type == BannerContentType::Lyrics
        && settings.footer_config.content_type == BannerContentType::Lyrics;
    let idle_music = entries.is_empty()
        && manual_message.is_none_or(|manual| manual.text.trim().is_empty())
        && settings.idle_content == super::runtime::IdleContent::Lyrics;
    let (header_lyric, footer_lyric) = if idle_music {
        (lyrics.title.as_str(), lyrics.title.as_str())
    } else if split_lyrics {
        (lyrics.title.as_str(), lyrics.lyric.as_str())
    } else {
        (lyrics.preferred(), lyrics.preferred())
    };
    let bounded = |text: &str| {
        TextViewport::new(text, settings.lyric_budget().unwrap_or(0))
            .start()
            .to_owned()
    };
    let activity = Activity {
        speaking: live.iter().any(|entry| {
            entry.source_kind == OscInputSource::Microphone
                && (!entry.source.is_empty() || !entry.translated.is_empty())
                && frame.now.saturating_duration_since(entry.updated_at)
                    < std::time::Duration::from_secs(3)
        }),
        latest_message: history
            .iter()
            .filter(|entry| {
                entry.source_kind == OscInputSource::Microphone && !entry.translated.is_empty()
            })
            .map(|entry| entry.updated_at)
            .max(),
        music: !lyrics.is_empty(),
    };
    let [prefix, suffix] = frame.renderer.banners.render(
        [&settings.header_config, &settings.footer_config],
        settings.text_limit(),
        metrics,
        [&bounded(header_lyric), &bounded(footer_lyric)],
        activity,
        frame.now,
    );

    if let Some(manual) = manual_message.filter(|manual| !manual.text.trim().is_empty()) {
        // Manual text and translations share the same space below/above lyrics.
        // Keep their established priority; decorations may not consume the body.
        let mut prefix = prefix.as_str();
        let mut suffix = suffix.as_str();
        while decoration_length(prefix, suffix, settings) >= settings.text_limit() {
            if !suffix.is_empty() {
                suffix = "";
            } else if !prefix.is_empty() {
                prefix = "";
            } else {
                break;
            }
        }
        let limit = settings
            .text_limit()
            .saturating_sub(decoration_length(prefix, suffix, settings));
        let lines = settings
            .line_limit()
            .saturating_sub(prefix.lines().count() + suffix.lines().count());
        let body = fit_manual_entries(&mut entries, manual, settings, limit, lines, frame);
        return frame.compose(prefix, &body, suffix, settings.line_limit());
    }

    // In persistent mode the decorations remain visible after message TTLs
    // expire; ordinary messages still disappear from `history`/`live`.
    if entries.is_empty() {
        let header = if settings.persistent_banners || settings.header_config.content_type.is_live()
        {
            prefix.as_str()
        } else {
            ""
        };
        let footer = if settings.persistent_banners || settings.footer_config.content_type.is_live()
        {
            suffix.as_str()
        } else {
            ""
        };
        if idle_music && !lyrics.is_empty() {
            let text = idle_lyrics::render(header, footer, lyrics, settings, frame);
            if !frame.renderer.regions.body.is_empty()
                || ![&settings.header_config, &settings.footer_config]
                    .iter()
                    .any(|config| config.content_type == BannerContentType::TextPet)
            {
                return text;
            }
        }
        if live.is_empty()
            && let Some(text) =
                idle_theater::render(header, footer, settings, activity.music, frame)
        {
            return text;
        }
        return fit_decorations(header, footer, settings, frame);
    }

    while let Some(first) = entries.front() {
        let body = render_entries(entries.iter().copied(), settings);
        let combined = compose_chatbox(&prefix, &body, &suffix);

        if body.chars().count() + decoration_length(&prefix, &suffix, settings)
            <= settings.text_limit()
            && combined.lines().count() <= settings.line_limit()
        {
            return frame.compose(&prefix, &body, &suffix, settings.line_limit());
        }
        if entries.len() > 1 {
            entries.pop_front();
        } else {
            return fit_single_entry(first, &prefix, &suffix, settings, frame);
        }
    }

    String::new()
}

fn fit_manual_entries(
    entries: &mut VecDeque<&HistoryMessage>,
    manual: &ManualMessage,
    settings: &OscSettings,
    limit: usize,
    lines: usize,
    frame: &mut Frame<'_>,
) -> String {
    let (manual_limit, manual_lines) = entries.back().map_or((limit, lines), |latest| {
        if latest.updated_at > manual.updated_at {
            let latest = render_entry(latest, settings);
            (
                limit.saturating_sub(latest.chars().count() + 1),
                lines.saturating_sub(latest.lines().count()),
            )
        } else {
            (limit, lines)
        }
    });
    if manual_lines == 0 || manual_limit == 0 {
        return fit_asr_entries(entries, limit, lines, settings, frame);
    }
    let manual_text = fit_lines(
        fit_prefixed_text(
            settings.prefix_for(OscInputSource::Typing),
            &sanitize_chatbox_segment(&manual.text),
            manual_limit,
            Block::Manual(manual.updated_at),
            Some(manual.expires_at),
            frame,
        ),
        manual_lines,
    );
    if manual_text.is_empty() {
        return fit_asr_entries(entries, limit, lines, settings, frame);
    }
    // Allocate from full content, not the current window (which may trim spaces).
    // Otherwise one scrolling block would resize and restart its neighbours.
    let manual_full = format!(
        "{}{}",
        prefixed_label(settings.prefix_for(OscInputSource::Typing)),
        sanitize_chatbox_segment(&manual.text)
    );
    let manual_budget = manual_full.chars().count().min(manual_limit);
    let manual_lines = fit_lines(manual_full, manual_lines).lines().count();
    let available = limit.saturating_sub(manual_budget + 1);
    if entries.is_empty() || available == 0 {
        return manual_text;
    }
    let captions = fit_asr_entries(
        entries,
        available,
        lines.saturating_sub(manual_lines),
        settings,
        frame,
    );
    if captions.is_empty() {
        manual_text
    } else {
        format!("{captions}\n{manual_text}")
    }
}

fn fit_asr_entries(
    entries: &mut VecDeque<&HistoryMessage>,
    limit: usize,
    line_limit: usize,
    settings: &OscSettings,
    frame: &mut Frame<'_>,
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
            return fit_lines(
                fit_multilingual_entry(first, settings, limit, frame),
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
        let mut decorated_target = target.clone();
        settings.append_microphone_suffix(&mut decorated_target, entry.source_kind);
        let speaker = settings
            .show_speaker_number
            .then(|| compact_speaker_label(&entry.speaker_id))
            .flatten();

        if settings.format_mode == OscFormatMode::TargetOnly {
            if !target.is_empty() {
                targets.push(with_speaker(
                    &with_source_prefix(&decorated_target, entry.source_kind, settings),
                    speaker.as_deref(),
                ));
            }
        } else if !source.is_empty() && !target.is_empty() && source != target {
            sources.push(with_speaker(
                &with_source_prefix(&source, entry.source_kind, settings),
                speaker.as_deref(),
            ));
            targets.push(decorated_target);
        } else if let Some(text) = (!target.is_empty())
            .then_some(decorated_target)
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
            let mut text = sanitize_chatbox_segment(text);
            if text.is_empty() {
                continue;
            }
            settings.append_microphone_suffix(&mut text, entry.source_kind);
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

fn fit_decorations(
    prefix: &str,
    suffix: &str,
    settings: &OscSettings,
    frame: &mut Frame<'_>,
) -> String {
    let limit = settings.text_limit();
    let lines = settings.line_limit();
    if limit == 0 || lines == 0 {
        return String::new();
    }
    let pet_budget = banners::pet_budget([&settings.header_config, &settings.footer_config], limit);
    let prefix_budget = settings
        .header_config
        .reserved_chars(prefix, pet_budget)
        .min(limit);
    let suffix_budget = limit.saturating_sub(prefix_budget + usize::from(prefix_budget > 0));
    let prefix = frame.banner(
        Block::Header,
        prefix,
        prefix_budget,
        settings.header_config.content_type,
    );
    let suffix = frame.banner(
        Block::Footer,
        suffix,
        suffix_budget,
        settings.footer_config.content_type,
    );
    frame.compose(&prefix, "", &suffix, lines)
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
    let target = sanitize_chatbox_segment(&entry.translated);
    let (mut texts, separator) = message_parts(
        &source,
        &target,
        entry
            .additional_translations
            .iter()
            .map(|(_, text)| sanitize_chatbox_segment(text)),
        settings.format_mode,
    );
    // Classify the original before decoration, so a same-language translation
    // does not suddenly become a second bilingual line just because of a suffix.
    let original = if !source.is_empty() && settings.format_mode != OscFormatMode::TargetOnly {
        if target.is_empty() {
            Some(0)
        } else if source != target {
            Some(
                if settings.format_mode == OscFormatMode::BilingualTargetFirst {
                    texts.len() - 1
                } else {
                    0
                },
            )
        } else {
            None
        }
    } else {
        None
    };
    for (index, text) in texts.iter_mut().enumerate() {
        if Some(index) != original {
            settings.append_microphone_suffix(text, entry.source_kind);
        }
    }
    let mut parts = texts
        .into_iter()
        .map(|text| (String::new(), text))
        .collect::<Vec<_>>();
    if let Some((label, _)) = parts.first_mut() {
        *label = entry_prefix(entry, settings);
    }
    (parts, separator)
}

fn fit_multilingual_entry(
    entry: &HistoryMessage,
    settings: &OscSettings,
    limit: usize,
    frame: &mut Frame<'_>,
) -> String {
    let (parts, separator) = entry_parts(entry, settings);
    let overhead = parts
        .iter()
        .map(|(label, _)| label.chars().count())
        .sum::<usize>()
        + parts.len().saturating_sub(1) * separator.chars().count();
    if overhead >= limit || parts.is_empty() {
        let rendered = render_entry(entry, settings);
        let label = entry_prefix(entry, settings);
        return fit_prefixed_text(
            &label,
            rendered.strip_prefix(&label).unwrap_or(&rendered),
            limit,
            Block::Caption(entry.stream_id, entry.updated_at, 0),
            Some(entry.expires_at),
            frame,
        );
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
        .enumerate()
        .map(|(index, ((label, text), length))| {
            format!(
                "{label}{}",
                frame.text(
                    Block::Caption(entry.stream_id, entry.updated_at, index),
                    &text,
                    length,
                    Some(entry.expires_at)
                )
            )
        })
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
    frame: &mut Frame<'_>,
) -> String {
    let limit = settings.text_limit();
    let mut prefix = prefix;
    let mut suffix = suffix;

    // Preserve speech before decorations when space is limited.
    while decoration_length(prefix, suffix, settings) >= limit {
        if !suffix.is_empty() {
            suffix = "";
        } else if !prefix.is_empty() {
            prefix = "";
        } else {
            break;
        }
    }
    let content_limit = limit.saturating_sub(decoration_length(prefix, suffix, settings));
    let content = fit_multilingual_entry(entry, settings, content_limit, frame);
    frame.compose(prefix, &content, suffix, settings.line_limit())
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

fn decoration_length(prefix: &str, suffix: &str, settings: &OscSettings) -> usize {
    let budget = banners::pet_budget(
        [&settings.header_config, &settings.footer_config],
        settings.text_limit(),
    );
    let text = settings.header_config.reserved_chars(prefix, budget)
        + settings.footer_config.reserved_chars(suffix, budget);
    let separators = usize::from(!prefix.is_empty()) + usize::from(!suffix.is_empty());
    text + separators
}

fn sanitize_prefix(text: &str) -> String {
    let value = text
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
    TextViewport::new(&value, super::runtime::MAX_PREFIX_LENGTH)
        .start()
        .to_owned()
}

fn fit_prefixed_text(
    prefix: &str,
    text: &str,
    limit: usize,
    block: Block,
    expires_at: Option<Instant>,
    frame: &mut Frame<'_>,
) -> String {
    let prefix = prefixed_label(prefix);
    if prefix.is_empty() {
        return frame.text(block, text, limit, expires_at);
    }
    if limit <= prefix.chars().count() {
        return TextViewport::new(&prefix, limit).start().to_owned();
    }
    let content_limit = limit - prefix.chars().count();
    let content = frame.text(block, text, content_limit, expires_at);
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
    use super::*;
    use std::time::Duration;

    fn build_chatbox_text(
        history: &[HistoryMessage],
        live: &[HistoryMessage],
        manual: Option<&ManualMessage>,
        settings: &OscSettings,
        metrics: &SystemMetrics,
        lyrics: &LyricsText,
    ) -> String {
        ChatboxRenderer::default().render(
            history,
            live,
            manual,
            settings,
            metrics,
            lyrics,
            Instant::now(),
        )
    }

    fn caption(now: Instant, text: &str) -> HistoryMessage {
        HistoryMessage {
            stream_id: 1,
            source_kind: OscInputSource::Microphone,
            source: String::new(),
            translated: text.into(),
            additional_translations: Vec::new(),
            speaker_id: String::new(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        }
    }

    #[test]
    fn microphone_suffix_is_literal_and_only_decorates_translations_in_all_layouts() {
        let now = Instant::now();
        let mut message = caption(now, "Hello.");
        message.source = "你好".into();
        message.additional_translations = vec![("ja".into(), "こんにちは".into())];
        for separator in [OscMessageSeparator::NewLine, OscMessageSeparator::Space] {
            for mode in [
                OscFormatMode::TargetOnly,
                OscFormatMode::Inline,
                OscFormatMode::BilingualSourceFirst,
                OscFormatMode::BilingualTargetFirst,
            ] {
                for source in [
                    OscInputSource::Microphone,
                    OscInputSource::SystemAudio,
                    OscInputSource::Typing,
                    OscInputSource::Unknown,
                ] {
                    message.source_kind = source;
                    let mut settings = OscSettings {
                        microphone_suffix_enabled: true,
                        microphone_suffix: " 喵。".into(),
                        message_separator: separator,
                        format_mode: mode,
                        ..Default::default()
                    };
                    for _ in 0..2 {
                        let text = build_chatbox_text(
                            std::slice::from_ref(&message),
                            &[],
                            None,
                            &settings,
                            &SystemMetrics::default(),
                            &LyricsText::default(),
                        );
                        if source == OscInputSource::Microphone {
                            assert!(
                                text.contains("Hello 喵。"),
                                "{mode:?} / {separator:?}: {text}"
                            );
                            assert!(text.contains("こんにちは 喵。"));
                            assert_eq!(text.matches(" 喵。").count(), 2);
                            assert!(!text.contains("你好 喵。"));
                        } else {
                            assert!(!text.contains("喵"));
                        }
                    }
                    settings.microphone_suffix_enabled = false;
                    assert!(
                        !build_chatbox_text(
                            std::slice::from_ref(&message),
                            &[],
                            None,
                            &settings,
                            &SystemMetrics::default(),
                            &LyricsText::default()
                        )
                        .contains("喵")
                    );
                }
            }
        }
        assert_eq!(message.translated, "Hello.");
    }

    #[test]
    fn suffix_does_not_create_empty_messages_or_duplicate_same_language_originals() {
        let now = Instant::now();
        let settings = OscSettings {
            microphone_suffix_enabled: true,
            microphone_suffix: "喵".into(),
            ..Default::default()
        };
        let mut message = caption(now, "你好");
        message.source = "你好".into();
        for separator in [OscMessageSeparator::NewLine, OscMessageSeparator::Space] {
            let settings = OscSettings {
                message_separator: separator,
                ..settings.clone()
            };
            assert_eq!(
                build_chatbox_text(
                    std::slice::from_ref(&message),
                    &[],
                    None,
                    &settings,
                    &SystemMetrics::default(),
                    &LyricsText::default()
                ),
                "🎤 你好喵"
            );
            let pending = HistoryMessage {
                translated: String::new(),
                ..message.clone()
            };
            assert_eq!(
                build_chatbox_text(
                    &[pending],
                    &[],
                    None,
                    &settings,
                    &SystemMetrics::default(),
                    &LyricsText::default()
                ),
                "🎤 你好"
            );
            assert!(
                build_chatbox_text(
                    &[caption(now, "")],
                    &[],
                    None,
                    &settings,
                    &SystemMetrics::default(),
                    &LyricsText::default()
                )
                .is_empty()
            );
        }
        let manual = ManualMessage {
            text: "Hello".into(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        assert_eq!(
            build_chatbox_text(
                &[],
                &[],
                Some(&manual),
                &settings,
                &SystemMetrics::default(),
                &LyricsText::default()
            ),
            "💬 Hello"
        );
    }

    #[test]
    fn suffix_uses_message_budget_and_scrolls_away_with_its_owner() {
        let now = Instant::now();
        let message = caption(now, "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText::default();
        for limit in [0, 1, 2, 3, 5, 10, 24, 144] {
            let settings = OscSettings {
                microphone_suffix_enabled: true,
                microphone_suffix: "喵。".into(),
                max_text_length: limit,
                ..Default::default()
            };
            let mut renderer = ChatboxRenderer::default();
            for seconds in [0, 5, 13] {
                let text = renderer.render(
                    std::slice::from_ref(&message),
                    &[],
                    None,
                    &settings,
                    &metrics,
                    &lyrics,
                    now + Duration::from_secs(seconds),
                );
                assert!(text.chars().count() <= limit);
                if seconds == 13 && limit >= 5 {
                    assert!(text.ends_with("喵。"), "{text}");
                }
            }
            assert!(
                renderer
                    .render(
                        &[],
                        &[],
                        None,
                        &settings,
                        &metrics,
                        &lyrics,
                        now + Duration::from_secs(15)
                    )
                    .is_empty()
            );
            assert!(renderer.regions.body.is_empty());
            assert!(!renderer.needs_refresh());
        }
    }

    #[test]
    fn selected_languages_scroll_independently_without_moving_banners_or_labels() {
        let now = Instant::now();
        let mut settings = OscSettings {
            max_text_length: 40,
            ..Default::default()
        };
        settings.header_config.content_type = BannerContentType::CustomText;
        settings.header_config.custom_text = "HEADER".into();
        settings.footer_config.content_type = BannerContentType::CustomText;
        settings.footer_config.custom_text = "FOOTER".into();
        let mut message = caption(now, "abcdefghijklmnopqrstuvwxyz");
        message.source = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".into();
        message.additional_translations = vec![(
            "ja".into(),
            "あいうえおかきくけこさしすせそたちつてとなにぬねの".into(),
        )];
        let old = caption(now - Duration::from_secs(1), "EVICTED");
        let messages = [old, message.clone()];
        let mut renderer = ChatboxRenderer::default();
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText::default();
        let first = renderer.render(&messages, &[], None, &settings, &metrics, &lyrics, now);
        assert!(first.contains("🎤 ABC"));
        assert!(first.contains("\nabc"));
        assert!(first.contains("\nあいう"));
        for step in 0..=23 {
            let text = renderer.render(
                &messages,
                &[],
                None,
                &settings,
                &metrics,
                &lyrics,
                now + Duration::from_millis(step * 600),
            );
            assert!(text.starts_with("HEADER\n🎤 "));
            assert!(text.ends_with("\nFOOTER"));
            assert!(!text.contains("EVICTED"));
            assert!(text.chars().count() <= settings.text_limit());
            assert_eq!(text.lines().count(), 5);
        }
        let tail = renderer.render(
            &messages,
            &[],
            None,
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(14),
        );
        assert!(tail.contains("XYZ\n"));
        assert!(tail.contains("xyz\n"));
        assert!(tail.contains("ぬねの\n"));
        assert_ne!(first, tail);
    }

    #[test]
    fn manual_scroll_keeps_newer_caption_space_and_resets_on_content_or_budget_changes() {
        let now = Instant::now();
        let mut settings = OscSettings {
            max_text_length: 22,
            ..Default::default()
        };
        let mut manual = ManualMessage {
            text: "AA BB CC DD EE FF GG HH II JJ KK LL MM NN OO PP".into(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        let speech = [caption(now + Duration::from_millis(1), "固定译文")];
        let mut renderer = ChatboxRenderer::default();
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText::default();
        let first = renderer.render(
            &speech,
            &[],
            Some(&manual),
            &settings,
            &metrics,
            &lyrics,
            now,
        );
        for second in 1..=13 {
            let text = renderer.render(
                &speech,
                &[],
                Some(&manual),
                &settings,
                &metrics,
                &lyrics,
                now + Duration::from_secs(second),
            );
            assert!(text.starts_with("🎤 固定译文\n💬 "));
            assert!(text.chars().count() <= 22);
        }
        let tail = renderer.render(
            &speech,
            &[],
            Some(&manual),
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(13),
        );
        assert!(tail.ends_with("OO PP"));
        assert_ne!(first, tail);

        settings.max_text_length = 24;
        let resized = renderer.render(
            &speech,
            &[],
            Some(&manual),
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(13),
        );
        assert!(resized.contains("💬 AA BB"));
        manual.text = "新内容需要重新从开头展示而不是继承旧内容的滚动位置".into();
        let replaced = renderer.render(
            &speech,
            &[],
            Some(&manual),
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(14),
        );
        assert!(replaced.contains("💬 新内容"));
        assert!(renderer.needs_refresh());
        assert!(
            renderer
                .render(
                    &[],
                    &[],
                    None,
                    &settings,
                    &metrics,
                    &lyrics,
                    now + Duration::from_secs(15)
                )
                .is_empty()
        );
        assert!(!renderer.needs_refresh());
    }

    #[test]
    fn persistent_banner_scrolls_only_inside_its_remaining_budget() {
        let now = Instant::now();
        let mut settings = OscSettings {
            max_text_length: 12,
            persistent_banners: true,
            ..Default::default()
        };
        settings.header_config.content_type = BannerContentType::CustomText;
        settings.header_config.custom_text = "固定".into();
        settings.footer_config.content_type = BannerContentType::CustomText;
        settings.footer_config.custom_text = "ABCDEFGHIJKLMNOPQRST".into();
        let mut renderer = ChatboxRenderer::default();
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText::default();
        let first = renderer.render(&[], &[], None, &settings, &metrics, &lyrics, now);
        let later = renderer.render(
            &[],
            &[],
            None,
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(6),
        );
        assert_eq!(first, "固定\nABCDEFGHI");
        assert_eq!(later, "固定\nEFGHIJKLM");
        renderer.clear();
        assert!(!renderer.needs_refresh());
        assert_eq!(
            renderer.render(
                &[],
                &[],
                None,
                &settings,
                &metrics,
                &lyrics,
                now + Duration::from_secs(7)
            ),
            first
        );
    }

    #[test]
    fn already_windowed_lyrics_are_not_scrolled_again_beside_a_custom_banner() {
        let now = Instant::now();
        let mut settings = OscSettings {
            max_text_length: 144,
            persistent_banners: true,
            ..Default::default()
        };
        settings.header_config.content_type = BannerContentType::CustomText;
        settings.header_config.custom_text = "H".repeat(130);
        settings.footer_config.content_type = BannerContentType::Lyrics;
        let lyrics = LyricsText {
            title: String::new(),
            lyric: "🎵 ABCDEFGHIJKLMNOPQRST".into(),
            ..Default::default()
        };
        let metrics = SystemMetrics::default();
        let mut renderer = ChatboxRenderer::default();
        let first = renderer.render(&[], &[], None, &settings, &metrics, &lyrics, now);
        let later = renderer.render(
            &[],
            &[],
            None,
            &settings,
            &metrics,
            &lyrics,
            now + Duration::from_secs(6),
        );
        assert_eq!(first, later);
        assert!(first.ends_with("\n🎵 ABCDEFGHIJK"));
        assert_eq!(first.chars().count(), 144);
        assert!(!renderer.needs_refresh());
    }

    #[test]
    fn every_scroll_frame_respects_small_budgets_and_line_limits() {
        let now = Instant::now();
        let mut message = caption(now, &"译文👩‍💻e\u{301} ".repeat(30));
        message.source = "原文🇨🇳\n".repeat(30);
        message.additional_translations = (0..10)
            .map(|i| (i.to_string(), "extra translation ".repeat(20)))
            .collect();
        let manual = ManualMessage {
            text: "手动👩‍💻消息\n".repeat(30),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText {
            title: "🎵 歌名".into(),
            lyric: "🎵 歌词".into(),
            ..Default::default()
        };
        for limit in [0, 1, 2, 3, 4, 8, 12, 24, 48, 144] {
            for format_mode in [
                OscFormatMode::TargetOnly,
                OscFormatMode::BilingualSourceFirst,
                OscFormatMode::BilingualTargetFirst,
                OscFormatMode::Inline,
            ] {
                let mut settings = OscSettings {
                    max_text_length: limit,
                    format_mode,
                    ..Default::default()
                };
                settings.header_config.content_type = BannerContentType::Lyrics;
                settings.footer_config.content_type = BannerContentType::Lyrics;
                for manual in [None, Some(&manual)] {
                    let mut renderer = ChatboxRenderer::default();
                    for second in [0, 3, 7, 13] {
                        let text = renderer.render(
                            std::slice::from_ref(&message),
                            &[],
                            manual,
                            &settings,
                            &metrics,
                            &lyrics,
                            now + Duration::from_secs(second),
                        );
                        assert!(text.chars().count() <= limit, "limit {limit}: {text:?}");
                        assert!(text.lines().count() <= settings.line_limit());
                        assert_eq!(
                            text,
                            compose_chatbox(
                                &renderer.regions.header,
                                &renderer.regions.body,
                                &renderer.regions.footer,
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lyrics_split_between_banners_with_a_single_region_fallback() {
        let metrics = SystemMetrics::default();
        for persistent_banners in [false, true] {
            for (header, footer, lyric, expected) in [
                (true, true, "🎵 当前歌词", "🎵 歌名 · 歌手\n🎵 当前歌词"),
                (true, true, "", "🎵 歌名 · 歌手"),
                (true, false, "🎵 当前歌词", "🎵 当前歌词"),
                (false, true, "🎵 当前歌词", "🎵 当前歌词"),
                (true, false, "", "🎵 歌名 · 歌手"),
                (false, true, "", "🎵 歌名 · 歌手"),
            ] {
                let mut settings = OscSettings {
                    persistent_banners,
                    ..Default::default()
                };
                if header {
                    settings.header_config.content_type = BannerContentType::Lyrics;
                }
                if footer {
                    settings.footer_config.content_type = BannerContentType::Lyrics;
                }
                let lyrics = LyricsText {
                    title: "🎵 歌名 · 歌手".into(),
                    lyric: lyric.into(),
                    ..Default::default()
                };
                assert_eq!(
                    build_chatbox_text(&[], &[], None, &settings, &metrics, &lyrics),
                    expected,
                );
                assert!(
                    build_chatbox_text(&[], &[], None, &settings, &metrics, &LyricsText::default())
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn split_lyrics_surround_speech_and_manual_text_within_the_budget() {
        let now = Instant::now();
        let mut settings = OscSettings::default();
        settings.header_config.content_type = BannerContentType::Lyrics;
        settings.footer_config.content_type = BannerContentType::Lyrics;
        let metrics = SystemMetrics::default();
        let lyrics = LyricsText {
            title: "🎵 歌名 · 歌手".into(),
            lyric: "🎵 当前歌词".into(),
            ..Default::default()
        };
        let speech = HistoryMessage {
            stream_id: 1,
            source_kind: OscInputSource::Microphone,
            source: String::new(),
            translated: "译文".into(),
            additional_translations: Vec::new(),
            speaker_id: String::new(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        let manual = ManualMessage {
            text: "手动消息".into(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        assert_eq!(
            build_chatbox_text(
                std::slice::from_ref(&speech),
                &[],
                None,
                &settings,
                &metrics,
                &lyrics
            ),
            "🎵 歌名 · 歌手\n🎤 译文\n🎵 当前歌词",
        );
        assert_eq!(
            build_chatbox_text(&[], &[], Some(&manual), &settings, &metrics, &lyrics),
            "🎵 歌名 · 歌手\n💬 手动消息\n🎵 当前歌词",
        );

        let lyrics = LyricsText {
            title: format!("🎵 {}", "歌名".repeat(50)),
            lyric: format!("🎵 {}", "歌词".repeat(50)),
            ..Default::default()
        };
        for limit in [0, 1, 6, 24, 48, 144] {
            settings.max_text_length = limit;
            for manual in [None, Some(&manual)] {
                let text = build_chatbox_text(
                    std::slice::from_ref(&speech),
                    &[],
                    manual,
                    &settings,
                    &metrics,
                    &lyrics,
                );
                assert!(text.chars().count() <= limit, "limit {limit}: {text}");
                assert!(text.lines().count() <= settings.line_limit());
            }
        }
    }

    #[test]
    fn target_only_waits_for_translation_without_evicting_visible_history() {
        let now = Instant::now();
        let recognized = HistoryMessage {
            stream_id: 1,
            source_kind: OscInputSource::Microphone,
            source: "Recognized source".into(),
            translated: String::new(),
            additional_translations: Vec::new(),
            speaker_id: String::new(),
            updated_at: now,
            expires_at: now + Duration::from_secs(15),
        };
        let metrics = SystemMetrics::default();
        for separator in [OscMessageSeparator::NewLine, OscMessageSeparator::Space] {
            let settings = OscSettings {
                format_mode: OscFormatMode::TargetOnly,
                message_separator: separator,
                ..Default::default()
            };
            for empty_target in ["", "  ", "。"] {
                let preview = HistoryMessage {
                    translated: empty_target.into(),
                    ..recognized.clone()
                };
                assert!(render_entry(&preview, &settings).is_empty());
                assert!(
                    build_chatbox_text(
                        &[],
                        &[preview],
                        None,
                        &settings,
                        &metrics,
                        &LyricsText::default()
                    )
                    .is_empty()
                );
            }
            let translated = HistoryMessage {
                translated: "译文".into(),
                ..recognized.clone()
            };
            assert_eq!(
                build_chatbox_text(
                    &[],
                    &[translated.clone()],
                    None,
                    &settings,
                    &metrics,
                    &LyricsText::default()
                ),
                "🎤 译文"
            );
            let previews = vec![recognized.clone(); MAX_ENTRIES];
            assert_eq!(
                build_chatbox_text(
                    &[translated],
                    &previews,
                    None,
                    &settings,
                    &metrics,
                    &LyricsText::default()
                ),
                "🎤 译文"
            );
            let additional = HistoryMessage {
                additional_translations: vec![("de".into(), "Übersetzung".into())],
                ..recognized.clone()
            };
            let text = build_chatbox_text(
                &[],
                &[additional],
                None,
                &settings,
                &metrics,
                &LyricsText::default(),
            );
            assert!(text.contains("Übersetzung"));
            assert!(!text.contains(&recognized.source));
            let bilingual = OscSettings {
                format_mode: OscFormatMode::BilingualSourceFirst,
                ..settings
            };
            assert!(
                build_chatbox_text(
                    &[],
                    &[recognized.clone()],
                    None,
                    &bilingual,
                    &metrics,
                    &LyricsText::default()
                )
                .contains(&recognized.source)
            );
        }
    }
}
