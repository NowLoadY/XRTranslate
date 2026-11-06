//! Shared translation-output contract; no UI, inference, or plugin ownership.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CaptureSource {
    #[default]
    Microphone,
    SystemAudio,
    Both,
}

impl CaptureSource {
    pub const fn routes(self) -> &'static [Self] {
        match self {
            Self::Microphone => &[Self::Microphone],
            Self::SystemAudio => &[Self::SystemAudio],
            Self::Both => &[Self::Microphone, Self::SystemAudio],
        }
    }
}

/// Per-output audio selection. Text results are independent of audio capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSourceFilter {
    pub microphone: bool,
    pub system_audio: bool,
}

impl Default for AudioSourceFilter {
    fn default() -> Self {
        Self {
            microphone: true,
            system_audio: true,
        }
    }
}

impl AudioSourceFilter {
    pub fn allows(self, source: CaptureSource, is_text: bool) -> bool {
        is_text
            || match source {
                CaptureSource::Microphone => self.microphone,
                CaptureSource::SystemAudio => self.system_audio,
                // Live capture expands Both into separate routes. A legacy mixed
                // caption cannot exclude either source, so requires both enabled.
                CaptureSource::Both => self.microphone && self.system_audio,
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageFormat {
    BilingualSourceFirst, // Source \n Target
    BilingualTargetFirst, // Target \n Source
    Inline,               // Source | Target
    TargetOnly,           // Target only
}

impl MessageFormat {
    pub fn label_key(&self) -> &'static str {
        match self {
            Self::BilingualSourceFirst => "Bilingual: original first",
            Self::BilingualTargetFirst => "Bilingual: translation first",
            Self::Inline => "Bilingual: one line",
            Self::TargetOnly => "Translation only",
        }
    }
}

pub fn message_parts(
    source: &str,
    target: &str,
    additional: impl IntoIterator<Item = impl AsRef<str>>,
    mode: MessageFormat,
) -> (Vec<String>, &'static str) {
    let source = source.trim();
    let target = target.trim();
    let show_source = !source.is_empty()
        && !target.is_empty()
        && source != target
        && mode != MessageFormat::TargetOnly;
    let mut parts = Vec::new();
    if show_source && mode != MessageFormat::BilingualTargetFirst {
        parts.push(source.to_owned());
    }
    let main = if target.is_empty() && mode != MessageFormat::TargetOnly {
        source
    } else {
        target
    };
    if !main.is_empty() {
        parts.push(main.to_owned());
    }
    parts.extend(additional.into_iter().filter_map(|text| {
        let text = text.as_ref().trim();
        (!text.is_empty()).then(|| text.to_owned())
    }));
    if show_source && mode == MessageFormat::BilingualTargetFirst {
        parts.push(source.to_owned());
    }
    (
        parts,
        if mode == MessageFormat::Inline {
            " | "
        } else {
            "\n"
        },
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub stream: u64,
    pub turn: String,
    pub segment: u32,
    pub source: String,
    pub translation: String,
    pub audio_source: CaptureSource,
    pub is_typing: bool,
    pub speaker: String,
    pub additional: Vec<String>,
}
