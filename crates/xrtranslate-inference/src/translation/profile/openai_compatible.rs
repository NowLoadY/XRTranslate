use xrtranslate_prompt::PromptProviderTarget;

use super::{TranslationProfile, output::clean_openai_compatible};

pub(super) static PROFILE: TranslationProfile = TranslationProfile {
    target: PromptProviderTarget::OpenAiCompatible,
    graph: super::configured_graph,
    temperature: 0.7,
    apply_sampling: |_, _| {},
    clean_output: clean_openai_compatible,
};
