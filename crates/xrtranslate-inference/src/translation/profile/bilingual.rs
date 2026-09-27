use serde_json::{Value, json};
use xrtranslate_engine::language::SupportedLanguage;
use xrtranslate_prompt::{PromptNodeGraph, PromptProviderTarget};

use super::{TranslationProfile, output::clean_openai_compatible};
use crate::InferenceError;
use crate::translation::TranslationOptions;

// Match the checkpoint's sentence-only training template and greedy decoding.
pub(super) static PROFILE: TranslationProfile = TranslationProfile {
    target: PromptProviderTarget::Hunyuan,
    graph,
    temperature: 0.0,
    apply_sampling,
    clean_output: clean_openai_compatible,
};

fn graph(options: &TranslationOptions) -> Result<&PromptNodeGraph, InferenceError> {
    let target = SupportedLanguage::parse(&options.target_language)
        .map(|language| language.name())
        .unwrap_or(&options.target_language);
    PromptNodeGraph::builtin_bilingual(target).map_err(|error| {
        InferenceError::InvalidConfiguration {
            field: "target_language",
            message: error.to_string(),
        }
    })
}

fn apply_sampling(payload: &mut Value, _options: &TranslationOptions) {
    payload["top_p"] = Value::from(1.0);
    payload["repeat_penalty"] = Value::from(1.0);
    payload["chat_template_kwargs"] = json!({"enable_thinking": false});
}
