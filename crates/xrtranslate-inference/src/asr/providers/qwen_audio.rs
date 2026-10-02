use std::collections::BTreeMap;

use crate::{AsrVocabularyBias, InferenceError};

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
