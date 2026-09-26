/// A completed ASR transcription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrTranscript {
    /// Canonical language code(s) reported or retained by the adapter, when available.
    pub language: Option<String>,
    pub text: String,
}

/// One provider-neutral ASR vocabulary preference.
///
/// Each adapter validates and translates this weight into its provider's
/// native vocabulary-bias contract. It is deliberately separate from an ASR
/// instruction prompt and from unweighted recognition context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsrVocabularyBias {
    pub text: String,
    pub weight: u8,
}

/// All ASR providers share input parsing; only their wire representations and
/// supported-language restrictions differ. None/empty/auto mean detection.
pub(super) fn parse_language(
    value: Option<&str>,
) -> Result<Option<xrtranslate_engine::language::SupportedLanguage>, crate::InferenceError> {
    let Some(value) = value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
    else {
        return Ok(None);
    };
    xrtranslate_engine::language::SupportedLanguage::parse(value)
        .map(Some)
        .ok_or_else(|| crate::InferenceError::InvalidConfiguration {
            field: "asr.language",
            message: format!("unknown language {value:?}"),
        })
}
