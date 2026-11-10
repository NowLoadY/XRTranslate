mod bilingual;
mod chat_messages;
mod contextual;
mod output;

use serde_json::{Value, json};
use xrtranslate_prompt::{PromptExecutionTrace, PromptMessage, PromptNodeGraph, PromptTarget};

use crate::InferenceError;

use super::{TranslationOptions, TranslationPromptProfile};

pub use output::is_probable_translation_context_leak;
pub(super) use output::translation_output_rejection;

pub(super) struct TranslationProfile {
    target: PromptTarget,
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

pub(super) fn registered(provider: TranslationPromptProfile) -> &'static TranslationProfile {
    match provider {
        TranslationPromptProfile::Contextual => &contextual::PROFILE,
        TranslationPromptProfile::Bilingual => &bilingual::PROFILE,
        TranslationPromptProfile::ChatMessages => &chat_messages::PROFILE,
    }
}

pub fn build_translation_messages(
    provider: TranslationPromptProfile,
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
    pub(super) context: xrtranslate_prompt::TranslationPromptContext,
}

impl RenderedTranslationPrompt {
    pub(super) fn messages_json(&self) -> Value {
        json!(self.messages)
    }
}

fn configured_graph(options: &TranslationOptions) -> Result<&PromptNodeGraph, InferenceError> {
    Ok(&options.prompt_graph)
}

fn render_prompt(
    graph: &PromptNodeGraph,
    target: PromptTarget,
    source_text: &str,
    options: &TranslationOptions,
) -> Result<RenderedTranslationPrompt, InferenceError> {
    let mut context = options.prompt_context.clone();
    let input_limit = options
        .context_window_tokens
        .saturating_sub(options.max_tokens) as usize;
    let execution = loop {
        let execution = graph
            .render_with_trace(
                target,
                source_text,
                &options.source_language,
                &options.target_language,
                &context,
            )
            .map_err(|error| InferenceError::InvalidConfiguration {
                field: "prompt_graph",
                message: error.to_string(),
            })?;
        if execution.render.estimated_tokens() <= input_limit {
            break execution;
        }
        if !context.reduce_reference_context() {
            return Err(InferenceError::InvalidConfiguration {
                field: "context_window_tokens",
                message: format!(
                    "The translation prompt and current input need about {} tokens, with {} reserved for output; the model window is {}. Shorten the input or prompt, or increase the model context window.",
                    execution.render.estimated_tokens(),
                    options.max_tokens,
                    options.context_window_tokens,
                ),
            });
        }
    };
    Ok(RenderedTranslationPrompt {
        messages: execution.render.messages,
        trace: execution.trace,
        context,
    })
}
