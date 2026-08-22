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
