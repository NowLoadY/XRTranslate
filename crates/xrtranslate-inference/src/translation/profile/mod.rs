mod bilingual;
mod contextual;
mod openai_compatible;
mod output;
mod qwen;

use serde_json::{Value, json};
use xrtranslate_prompt::{
    PromptExecutionTrace, PromptMessage, PromptMessageRole, PromptNodeGraph, PromptProviderTarget,
};

use crate::InferenceError;

use super::{TranslationOptions, TranslationProvider};

pub use output::is_probable_translation_context_leak;
pub(super) use output::translation_output_rejection;

pub(super) struct TranslationProfile {
    target: PromptProviderTarget,
    graph: fn(&TranslationOptions) -> Result<&PromptNodeGraph, InferenceError>,
    temperature: f64,
    apply_sampling: fn(&mut Value, &TranslationOptions),
    clean_output: fn(&str) -> String,
}

impl TranslationProfile {
    pub(super) fn temperature(&self) -> f64 {
        self.temperature
    }

    pub(super) fn build_prompt(
        &self,
        source_text: &str,
        options: &TranslationOptions,
    ) -> Result<RenderedTranslationPrompt, InferenceError> {
        render_prompt((self.graph)(options)?, self.target, source_text, options)
    }

    pub(super) fn apply_sampling(&self, payload: &mut Value, options: &TranslationOptions) {
        (self.apply_sampling)(payload, options);
    }

    pub(super) fn clean_output(&self, text: &str) -> String {
        (self.clean_output)(text)
    }
}

pub(super) fn registered(provider: TranslationProvider) -> &'static TranslationProfile {
    match provider {
        TranslationProvider::Contextual => &contextual::PROFILE,
        TranslationProvider::Bilingual => &bilingual::PROFILE,
        TranslationProvider::OpenAiCompatible => &openai_compatible::PROFILE,
        TranslationProvider::Qwen => &qwen::PROFILE,
    }
}

pub fn build_translation_messages(
    provider: TranslationProvider,
    source_text: &str,
    options: &TranslationOptions,
) -> Result<Value, InferenceError> {
    registered(provider)
        .build_prompt(source_text, options)
        .map(|prompt| prompt.messages_json())
}

pub(super) struct RenderedTranslationPrompt {
    pub(super) messages: Vec<PromptMessage>,
    pub(super) trace: PromptExecutionTrace,
}

impl RenderedTranslationPrompt {
    pub(super) fn messages_json(&self) -> Value {
        Value::Array(
            self.messages
                .iter()
                .map(|message| {
                    json!({
                        "role": match message.role {
                            PromptMessageRole::System => "system",
                            PromptMessageRole::User => "user",
                        },
                        "content": message.content,
                    })
                })
                .collect(),
        )
    }
}

fn configured_graph(options: &TranslationOptions) -> Result<&PromptNodeGraph, InferenceError> {
    Ok(&options.prompt_graph)
}

fn render_prompt(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
    source_text: &str,
    options: &TranslationOptions,
) -> Result<RenderedTranslationPrompt, InferenceError> {
    let execution = graph
        .render_with_trace(
            target,
            source_text,
            &options.source_language,
            &options.target_language,
            &options.prompt_context,
        )
        .map_err(|error| InferenceError::InvalidConfiguration {
            field: "prompt_graph",
            message: error.to_string(),
        })?;
    Ok(RenderedTranslationPrompt {
        messages: execution.render.messages,
        trace: execution.trace,
    })
}
