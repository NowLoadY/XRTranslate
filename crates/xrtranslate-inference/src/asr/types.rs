/// A completed ASR transcription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrTranscript {
    /// Language label reported or retained by the adapter, when available.
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
