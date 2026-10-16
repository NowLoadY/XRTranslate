//! Completed translations delivered once to Big Walk's native chat bridge.
mod runtime;
mod ui;

use crate::{
    plugins::text_output::{MessageFormat, message_parts},
    session_coordinator::AudioSourceFilter,
    ui::components::text_composer::TextComposer,
};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) use runtime::{BigWalkSnapshot, ConnectionStatus};
pub(crate) use ui::{BigWalkPageContext, BigWalkUiAction};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BigWalkSettings {
    pub enabled: bool,
    pub audio_sources: AudioSourceFilter,
    pub format: MessageFormat,
}

impl Default for BigWalkSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            audio_sources: AudioSourceFilter {
                microphone: true,
                system_audio: false,
            },
            format: MessageFormat::TargetOnly,
        }
    }
}

pub(crate) struct BigWalkPlugin {
    settings: BigWalkSettings,
    composer: TextComposer,
    host_enabled: bool,
    runtime: runtime::Runtime,
}

impl BigWalkPlugin {
    pub(crate) fn new(settings: BigWalkSettings, host_enabled: bool) -> Self {
        let runtime = runtime::Runtime::new(&settings, host_enabled);
        Self {
            settings,
            composer: TextComposer::default(),
            host_enabled,
            runtime,
        }
    }

    pub(crate) fn publisher(&self) -> runtime::Publisher {
        self.runtime.publisher()
    }

    pub(crate) fn settings(&self) -> &BigWalkSettings {
        &self.settings
    }

    pub(crate) fn set_host_enabled(&mut self, enabled: bool) {
        self.host_enabled = enabled;
        self.apply_settings();
    }

    pub(crate) fn apply_settings(&mut self) {
        self.runtime.configure(&self.settings, self.host_enabled);
    }

    pub(crate) fn snapshot(&self) -> BigWalkSnapshot {
        self.runtime.snapshot()
    }

    pub(crate) fn send_manual(&self, text: &str) -> bool {
        self.runtime.send_manual(text)
    }

    pub(crate) fn clear(&self) {
        self.runtime.clear();
    }

    pub(crate) fn draft_input_mut(&mut self) -> &mut String {
        &mut self.composer.text
    }
}

// Game chat already supplies the player's identity. Keep text literal and each
// language on one line; reserve space for every language before shortening any.
fn chat_text<'a>(
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
