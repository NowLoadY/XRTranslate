use std::collections::BTreeMap;

use crate::{AsrVocabularyBias, InferenceError};

pub(super) const MAX_CONTEXT_CHARS: usize = 400;
pub(super) const MAX_VOCABULARY_ENTRIES: usize = 2_000;

pub(super) fn validated_vocabulary(
    vocabulary: &[AsrVocabularyBias],
) -> Result<BTreeMap<String, u8>, InferenceError> {
    let mut result = BTreeMap::new();
    let mut discarded_entries = 0usize;
    for item in vocabulary {
        let text = item.text.trim();
        if text.is_empty() {
            discarded_entries += 1;
            continue;
        }
        if !matches!(item.weight, 1..=5 | 50) {
            return Err(InferenceError::InvalidConfiguration {
                field: "vocabulary_bias.weight",
                message: "must be between 1 and 5, or exactly 50".into(),
            });
        }
        if text.is_ascii() {
            if text.split_ascii_whitespace().count() > 7 {
                discarded_entries += 1;
                continue;
            }
        } else if text.chars().count() > 15 {
            discarded_entries += 1;
            continue;
        }
        if !result.contains_key(text) && result.len() >= MAX_VOCABULARY_ENTRIES {
            discarded_entries += 1;
            continue;
        }
        result.insert(text.to_owned(), item.weight);
    }
    let mut super_hot_words = 0usize;
    result.retain(|_, weight| {
        if *weight != 50 {
            return true;
        }
        super_hot_words += 1;
        let keep = super_hot_words <= 50;
        discarded_entries += usize::from(!keep);
        keep
    });
    if discarded_entries > 0 {
        tracing::warn!(
            discarded_entries,
            "discarded ASR vocabulary entries outside the provider contract"
        );
    }
    Ok(result)
}

pub(super) fn map_language(
    connector: xrtranslate_assets::remote::RemoteConnector,
    model: &str,
    language: Option<&str>,
) -> Result<(Option<String>, Option<String>), InferenceError> {
    let Some(language) = crate::asr::types::parse_language(language)? else {
        return Ok((None, None));
    };
    let mut code = language.base_code();
    if let Some(card) = xrtranslate_assets::remote::remote_model(connector, model) {
        if let Some((_, canonical)) = card
            .language_aliases
            .iter()
            .find(|(alias, _)| alias == code)
        {
            code = canonical;
        }
        if !card.languages.is_empty() && !card.languages.iter().any(|supported| supported == code) {
            return Err(InferenceError::InvalidConfiguration {
                field: "asr.language",
                message: format!("{} is not supported by {model}", language.code()),
            });
        }
    }
    Ok((Some(code.to_owned()), Some(language.code().to_owned())))
}
