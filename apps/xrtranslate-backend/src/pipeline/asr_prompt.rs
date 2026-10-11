//! Provider-neutral ASR prompt, context-bias, and vocabulary delivery policy.
//!
//! This module translates declared provider capabilities into one request
//! payload. It does not own retries or language-route decisions.

use xrtranslate_config::AsrPromptMode;
use xrtranslate_inference::AsrVocabularyBias;
use xrtranslate_prompt::{
    AsrPromptContext, PromptExecutionTrace, PromptNodeGraph, PromptNodeKind, PromptProviderTarget,
};

use super::{InferenceFailure, language_name, normalized_code};

#[derive(Clone, Debug)]
pub(super) struct AsrPromptPolicy {
    mode: AsrPromptMode,
    context_max_chars: Option<usize>,
    supports_vocabulary_bias: bool,
    vocabulary_weight: u8,
}

#[derive(Clone, Debug, Default)]
pub(super) struct AsrPromptDelivery {
    pub(super) instruction_prompt: Option<String>,
    pub(super) context_bias: Option<String>,
    pub(super) vocabulary_bias: Vec<AsrVocabularyBias>,
    pub(super) prompt_trace: Option<PromptExecutionTrace>,
}

impl AsrPromptPolicy {
    pub(super) fn supports_recognition_bias(&self) -> bool {
        self.mode != AsrPromptMode::None || self.supports_vocabulary_bias
    }

    pub(super) const fn new(
        mode: AsrPromptMode,
        context_max_chars: Option<usize>,
        supports_vocabulary_bias: bool,
        vocabulary_weight: u8,
    ) -> Self {
        Self {
            mode,
            context_max_chars,
            supports_vocabulary_bias,
            vocabulary_weight,
        }
    }

    pub(super) fn delivery(
        &self,
        graph: &PromptNodeGraph,
        source_language: &str,
        expected_languages: &str,
        context: &AsrPromptContext,
    ) -> Result<AsrPromptDelivery, InferenceFailure> {
        let mut delivery = AsrPromptDelivery::default();
        if self.supports_vocabulary_bias {
            delivery.vocabulary_bias = context
                .vocabulary
                .iter()
                .map(|term| term.trim())
                .filter(|term| !term.is_empty())
                .map(|term| AsrVocabularyBias {
                    text: term.to_owned(),
                    weight: self.vocabulary_weight,
                })
                .collect();
            // Prefer the lexical context field when available. For a
            // vocabulary-only model, keep a short, lightly weighted phrase.
            if self.mode == AsrPromptMode::None && !context.recent_phrase.is_empty() {
                let phrase = context.recent_phrase.trim();
                let text = if phrase.is_ascii() {
                    let mut words = phrase.split_whitespace().rev().take(5).collect::<Vec<_>>();
                    words.reverse();
                    words.join(" ")
                } else {
                    let start = phrase
                        .char_indices()
                        .rev()
                        .nth(11)
                        .map_or(0, |(offset, _)| offset);
                    phrase[start..].to_owned()
                };
                if !delivery
                    .vocabulary_bias
                    .iter()
                    .any(|term| term.text == text)
                {
                    delivery
                        .vocabulary_bias
                        .push(AsrVocabularyBias { text, weight: 1 });
                }
            }
        }

        let target = match self.mode {
            AsrPromptMode::None => return Ok(delivery),
            AsrPromptMode::Instruction => PromptProviderTarget::AsrInstruction,
            AsrPromptMode::ContextBias if !context.has_recognition_context() => {
                return Ok(delivery);
            }
            AsrPromptMode::ContextBias => PromptProviderTarget::AsrContextBias,
        };
        let bounded_context = self
            .context_max_chars
            .map(|limit| context.bounded_recognition_context(limit));
        let render_context = bounded_context.as_ref().unwrap_or(context);
        if target == PromptProviderTarget::AsrContextBias
            && !render_context.has_recognition_context()
        {
            return Ok(delivery);
        }
        if !graph.nodes.iter().any(|node| {
            matches!(node.kind, PromptNodeKind::Request { target: request_target, .. } if request_target == target)
        }) {
            return Ok(delivery);
        }
        let source_language = if source_language.eq_ignore_ascii_case("auto") {
            "auto".to_owned()
        } else {
            language_name(&normalized_code(source_language)).to_owned()
        };
        let expected_languages = expected_languages
            .split(',')
            .map(normalized_code)
            .map(|code| language_name(&code).to_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let rendered = graph
            .render_asr_with_trace(
                target,
                &source_language,
                &expected_languages,
                render_context,
            )
            .map_err(|error| {
                InferenceFailure::runtime(format!("cannot render {target:?} ASR prompt: {error}"))
            })?;
        delivery.prompt_trace = Some(rendered.trace.clone());
        let text = rendered
            .render
            .messages
            .into_iter()
            .map(|message| message.content.trim().to_owned())
            .filter(|content| !content.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.is_empty() {
            return Err(InferenceFailure::runtime(format!(
                "{target:?} ASR prompt rendered no text"
            )));
        }
        if target == PromptProviderTarget::AsrContextBias
            && self
                .context_max_chars
                .is_some_and(|limit| text.chars().count() > limit)
        {
            return Err(InferenceFailure::runtime(format!(
                "{target:?} ASR context exceeds the provider character limit"
            )));
        }
        match target {
            PromptProviderTarget::AsrInstruction => delivery.instruction_prompt = Some(text),
            PromptProviderTarget::AsrContextBias => delivery.context_bias = Some(text),
            _ => unreachable!("target was selected from ASR-only prompt modes"),
        }
        Ok(delivery)
    }
}

impl AsrPromptDelivery {
    pub(super) fn quality_context(&self) -> Option<String> {
        self.instruction_prompt
            .as_ref()
            .or(self.context_bias.as_ref())
            .cloned()
    }
}
