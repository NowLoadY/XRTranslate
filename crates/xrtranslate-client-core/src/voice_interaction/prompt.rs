//! One finite edit: fixed task wording, frozen reference facts and the latest request.
use serde::Serialize;
use xrtranslate_engine::language::SupportedLanguage;

const TASK: &str = "Apply change_request to current_message. Preserve its meaning, language, tone and previous edits except where the request asks to change them. Use original_speech and original_translation_language only as references. Return only the complete revised message.";

#[derive(Serialize)]
struct Inputs<'a> {
    original_speech: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_translation_language: Option<&'static str>,
    current_message: &'a str,
    change_request: &'a str,
}

pub(super) fn build(source: &str, current: &str, target_language: &str, request: &str) -> String {
    let inputs = Inputs {
        original_speech: source,
        original_translation_language: SupportedLanguage::parse(target_language)
            .map(SupportedLanguage::name),
        current_message: current,
        change_request: request,
    };
    // JSON quoting keeps literal quotes, braces, tags and newlines inside their
    // input fields. Ask still receives its existing single prompt string.
    let inputs = serde_json::to_string_pretty(&inputs).expect("edit inputs contain only strings");
    format!("{TASK}\n\n{inputs}")
}

pub(super) fn accept(text: String) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("No edited message was returned".into());
    }
    let json = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```\n"))
        .and_then(|body| body.strip_suffix("\n```"))
        .unwrap_or(text);
    let echoed_inputs = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .is_some_and(|value| {
            ["original_speech", "current_message", "change_request"]
                .iter()
                .all(|key| value.get(key).is_some())
        });
    if text.contains(TASK) || echoed_inputs {
        return Err("The model repeated the editing request instead of returning a message".into());
    }
    // Editing may intentionally produce quotes, several sentences or mixed
    // languages. OSC owns send limits; no second model call repairs this result.
    Ok(text.to_owned())
}
