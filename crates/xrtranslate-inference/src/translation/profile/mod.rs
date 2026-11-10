mod bilingual;
mod chat_messages;
mod contextual;
mod output;
#[cfg(test)]
mod tests;

use serde_json::{Value, json};
use xrtranslate_prompt::{
    PromptExecution, PromptExecutionTrace, PromptMessage, PromptNodeGraph, PromptTarget,
    estimated_tokens,
};

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
    let mut recommendation = None;
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
        // Retain the complete request's requirement before dropping optional
        // references, so the suggestion can also preserve the selected context.
        let recommendation = *recommendation.get_or_insert_with(|| {
            execution
                .render
                .context_window_recommendation(options.max_tokens)
        });
        if execution.render.estimated_tokens() <= input_limit {
            break execution;
        }
        if !context.reduce_reference_context() {
            return Err(InferenceError::InvalidConfiguration {
                field: "context_window_tokens",
                message: context_window_diagnostic(
                    graph,
                    target,
                    source_text,
                    options,
                    &execution,
                    recommendation.recommended_tokens,
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

fn context_window_diagnostic(
    graph: &PromptNodeGraph,
    target: PromptTarget,
    source_text: &str,
    options: &TranslationOptions,
    execution: &PromptExecution,
    recommended_tokens: usize,
) -> String {
    let input_limit = options
        .context_window_tokens
        .saturating_sub(options.max_tokens) as usize;
    // A minimal nonempty input uses the same rendering contract as a real
    // request. Inspect the active mode/target, not saved but inactive styles.
    let prompt_too_long = graph
        .render(
            target,
            "x",
            &options.source_language,
            &options.target_language,
            &options.prompt_context.without_reference_context(),
        )
        .is_ok_and(|prompt| prompt.estimated_tokens() > input_limit);
    let (cause, alternative) = if prompt_too_long {
        (
            "The active translation prompt/style is too long even for a short input.",
            "shorten the active style or prompt in Prompt Studio",
        )
    } else {
        (
            "The current input is too long for the space left by the active translation prompt/style.",
            "split or shorten the current input, or shorten the active style in Prompt Studio",
        )
    };
    let style = graph
        .translation_style
        .as_ref()
        .and_then(|style| execution.trace.node(&style.node_id))
        .filter(|node| !node.output.trim().is_empty())
        .map(|node| {
            format!(
                " Active style: about {} tokens.",
                estimated_tokens(&node.output)
            )
        })
        .unwrap_or_default();
    let required = execution
        .render
        .context_window_recommendation(options.max_tokens);
    format!(
        "{cause} Current text: about {} tokens.{style} After removing optional reference context, \
         this request still needs about {} input + {} output = {} tokens; the configured window is {}. \
         Suggested context length for this full request: {recommended_tokens} tokens \
         (including the selected reference context and 20% headroom). \
         If supported by the model, set Settings > Models > Translation Model > Context length \
         to {recommended_tokens} and restart XRTranslate; otherwise {alternative} or use a model with a larger context window.",
        estimated_tokens(source_text.trim()),
        execution.render.estimated_tokens(),
        options.max_tokens,
        required.required_tokens,
        options.context_window_tokens,
    )
}
