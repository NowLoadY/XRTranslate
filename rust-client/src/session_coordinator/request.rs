use super::PluginSessionBinding;
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
}

pub(crate) enum TranslationInput {
    Live(CaptureSource),
    File {
        path: std::path::PathBuf,
        recognition: RecognitionSettings,
        options: AudioImportOptions,
    },
}
