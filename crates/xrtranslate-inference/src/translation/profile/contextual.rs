use serde_json::Value;
use xrtranslate_prompt::PromptProviderTarget;

use super::{TranslationProfile, output::clean_contextual};
use crate::translation::TranslationOptions;

const TEMPERATURE: f64 = 0.7;
const TOP_P: f64 = 0.6;
const TOP_K: u64 = 20;
const REPEAT_PENALTY: f64 = 1.05;

pub(super) static PROFILE: TranslationProfile = TranslationProfile {
    target: PromptProviderTarget::Hunyuan,
    temperature: TEMPERATURE,
    apply_sampling,
    clean_output: clean_contextual,
};

fn apply_sampling(payload: &mut Value, options: &TranslationOptions) {
    payload["top_p"] = Value::from(TOP_P);
    payload["top_k"] = Value::from(TOP_K);
    payload["repeat_penalty"] = Value::from(REPEAT_PENALTY);
    payload["repeat_last_n"] = Value::from(options.context_window_tokens.max(1));
    payload["min_p"] = Value::from(0.0);
}
