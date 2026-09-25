use serde_json::Value;
use xrtranslate_prompt::PromptProviderTarget;

use super::{TranslationProfile, output::clean_openai_compatible};
use crate::translation::TranslationOptions;

// A bilingual checkpoint uses deterministic decoding and the shared
// single-user translation prompt graph.
pub(super) static PROFILE: TranslationProfile = TranslationProfile {
    target: PromptProviderTarget::Hunyuan,
    temperature: 0.0,
    apply_sampling,
    clean_output: clean_openai_compatible,
};

fn apply_sampling(payload: &mut Value, _options: &TranslationOptions) {
    payload["top_p"] = Value::from(1.0);
}
