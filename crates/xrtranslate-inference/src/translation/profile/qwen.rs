use serde_json::Value;
use xrtranslate_prompt::PromptProviderTarget;

use super::{TranslationProfile, output::clean_openai_compatible};
use crate::translation::TranslationOptions;

pub(super) static PROFILE: TranslationProfile = TranslationProfile {
    target: PromptProviderTarget::OpenAiCompatible,
    temperature: 0.7,
    apply_sampling,
    clean_output: clean_openai_compatible,
};

fn apply_sampling(payload: &mut Value, _options: &TranslationOptions) {
    // DashScope Qwen-MT requires single-turn user messages and rejects system-role messages.
    // Merge any system messages with the user message so Prompt Studio rules are preserved.
    if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
        let mut system_contents = Vec::new();
        let mut user_contents = Vec::new();
        for msg in messages.drain(..) {
            if let Some(role) = msg.get("role").and_then(Value::as_str) {
                if let Some(content) = msg.get("content").and_then(Value::as_str) {
                    if role == "system" {
                        if !content.trim().is_empty() {
                            system_contents.push(content.trim().to_owned());
                        }
                    } else if !content.trim().is_empty() {
                        user_contents.push(content.trim().to_owned());
                    }
                }
            }
        }
        let merged_user_content = if system_contents.is_empty() {
            user_contents.join("\n\n")
        } else if user_contents.is_empty() {
            system_contents.join("\n\n")
        } else {
            format!(
                "{}\n\n{}",
                system_contents.join("\n\n"),
                user_contents.join("\n\n")
            )
        };
        messages.push(serde_json::json!({
            "role": "user",
            "content": merged_user_content,
        }));
    }
}
