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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_vocabulary_is_independent_from_prompt_text_mode() {
        let delivery = AsrPromptPolicy::new(AsrPromptMode::None, None, true, 4)
            .delivery(
                &PromptNodeGraph::builtin_default(),
                "auto",
                "en, zh",
                &AsrPromptContext {
                    vocabulary: vec![" XRTranslate ".into(), "VRChat".into()],
                    mode: xrtranslate_prompt::PromptMode::Ordinary,
                },
            )
            .unwrap();

        assert_eq!(delivery.instruction_prompt, None);
        assert_eq!(delivery.context_bias, None);
        assert_eq!(
            delivery.vocabulary_bias,
            vec![
                AsrVocabularyBias {
                    text: "XRTranslate".into(),
                    weight: 4,
                },
                AsrVocabularyBias {
                    text: "VRChat".into(),
                    weight: 4,
                },
            ]
        );
    }

    #[test]
    fn context_limit_does_not_truncate_structured_vocabulary() {
        let delivery = AsrPromptPolicy::new(AsrPromptMode::ContextBias, Some(11), true, 5)
            .delivery(
                &PromptNodeGraph::builtin_default(),
                "auto",
                "en, zh",
                &AsrPromptContext {
                    vocabulary: vec!["XRTranslate".into(), "VRChat".into()],
                    mode: xrtranslate_prompt::PromptMode::Ordinary,
                },
            )
            .unwrap();

        assert_eq!(delivery.instruction_prompt, None);
        assert_eq!(delivery.context_bias.as_deref(), Some("XRTranslate"));
        assert_eq!(delivery.vocabulary_bias.len(), 2);
        assert!(delivery.prompt_trace.is_some());
    }
}
