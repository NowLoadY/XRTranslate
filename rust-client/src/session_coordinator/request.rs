use super::{PluginSessionBinding, TranslationSessionOwner};
use crate::{
    client_settings::{CaptureSource, RecognitionSettings},
    media_import::AudioImportOptions,
};
use xrtranslate_engine::language::LanguageSelection;

/// Complete task captured before service startup, including its language intent
/// and subscriber identity. Pending startup never reads the host language UI.
pub(crate) struct TranslationTask {
    pub languages: LanguageSelection,
    pub plugin: Option<PluginSessionBinding>,
    pub input: TranslationInput,
    pub profiles: Vec<(CaptureSource, RecognitionSettings)>,
}

pub(crate) enum TranslationInput {
    Text(String),
    Live(CaptureSource),
    /// Acquire only this task's live channels and reject overlapping owners.
    /// Uses the same capture and translation pipeline as ordinary live input.
    ExclusiveLive(CaptureSource),
    File {
        path: std::path::PathBuf,
        recognition: RecognitionSettings,
        options: AudioImportOptions,
    },
}

impl TranslationInput {
    pub(crate) fn live_source(&self) -> Option<CaptureSource> {
        match self {
            Self::Live(source) | Self::ExclusiveLive(source) => Some(*source),
            _ => None,
        }
    }
}

impl TranslationTask {
    pub fn text(
        text: String,
        languages: LanguageSelection,
        plugin: Option<PluginSessionBinding>,
    ) -> Self {
        Self {
            languages,
            plugin,
            input: TranslationInput::Text(text),
            profiles: Vec::new(),
        }
    }

    pub fn owner(&self) -> TranslationSessionOwner {
        self.plugin.as_ref().map_or_else(
            || TranslationSessionOwner::Host {
                capture_source: match &self.input {
                    TranslationInput::Text(_) => CaptureSource::Microphone,
                    TranslationInput::Live(source) | TranslationInput::ExclusiveLive(source) => {
                        *source
                    }
                    TranslationInput::File { .. } => CaptureSource::SystemAudio,
                },
            },
            |binding| TranslationSessionOwner::Plugin(binding.owner.clone()),
        )
    }
}
