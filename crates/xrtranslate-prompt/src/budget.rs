use crate::{
    PromptGraphError, PromptNodeGraph, PromptRender, TranslationPromptContext,
    TranslationPromptProfile, estimated_tokens,
};

impl PromptRender {
    /// Estimate the complete request, including message framing and a template
    /// allowance. This is shared by input segmentation and final request fitting.
    pub fn estimated_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|message| estimated_tokens(&message.content) + 8)
            .sum::<usize>()
            .saturating_add(64)
    }

    /// Size a window for this rendered request, including any selected reference
    /// context and the caller's output allowance. Leave proportional headroom
    /// for the heuristic tokenizer and small changes in subsequent inputs;
    /// rounding to 256 tokens avoids suggesting an unnecessarily large power of two.
    pub fn context_window_recommendation(&self, output_tokens: u32) -> ContextWindowRecommendation {
        let required_tokens = self
            .estimated_tokens()
            .saturating_add(output_tokens as usize);
        let with_headroom = required_tokens.saturating_add(required_tokens.div_ceil(5));
        ContextWindowRecommendation {
            required_tokens,
            recommended_tokens: with_headroom.div_ceil(256).saturating_mul(256),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextWindowRecommendation {
    pub required_tokens: usize,
    /// An estimate for the request, not a claim about the model's supported maximum.
    pub recommended_tokens: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct TextTranslationBudget {
    pub input_tokens: usize,
    pub output_tokens: u32,
}

impl TextTranslationBudget {
    /// Short drafts keep a small allowance; longer drafts can use the reserved
    /// output space without changing the model's realtime defaults.
    pub fn output_limit(self, text: &str, minimum: u32) -> u32 {
        let estimate = estimated_tokens(text).saturating_mul(2).saturating_add(64);
        minimum
            .max(u32::try_from(estimate).unwrap_or(u32::MAX))
            .min(self.output_tokens)
    }
}

impl PromptNodeGraph {
    /// Estimated capacity shared by the composer and text task segmentation.
    pub fn translation_input_budget(
        &self,
        profile: TranslationPromptProfile,
        source: &str,
        target: &str,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Result<usize, PromptGraphError> {
        self.text_translation_budget(profile, source, target, context_tokens, output_tokens)
            .map(|budget| budget.input_tokens)
    }

    /// Share the window between source/context and translation after reserving
    /// the active prompt, including its style. A configured output allowance is
    /// a floor, not a fixed input ceiling for longer text.
    pub fn text_translation_budget(
        &self,
        profile: TranslationPromptProfile,
        source: &str,
        target: &str,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Result<TextTranslationBudget, PromptGraphError> {
        let prompt = self.render(
            profile.target(),
            "x",
            source,
            target,
            &TranslationPromptContext::default(),
        )?;
        let overhead = prompt.estimated_tokens().max(256);
        let available = (context_tokens as usize).saturating_sub(overhead);
        let output_tokens = (output_tokens as usize)
            .max(available.div_ceil(2))
            .min(available);
        Ok(TextTranslationBudget {
            input_tokens: available - output_tokens,
            output_tokens: output_tokens as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_grows_with_context_without_the_realtime_output_ceiling() {
        let graph = PromptNodeGraph::default();
        let budget = |context, output| {
            graph
                .text_translation_budget(
                    TranslationPromptProfile::ChatMessages,
                    "English",
                    "Chinese",
                    context,
                    output,
                )
                .unwrap()
        };
        let original = budget(2048, 256);
        let expanded = budget(4700, 256);
        assert!(original.input_tokens > 256);
        assert!(expanded.input_tokens > original.input_tokens);
        assert!(budget(8192, 256).input_tokens > expanded.input_tokens);
        assert!(expanded.output_tokens as usize >= expanded.input_tokens);
        assert!(expanded.input_tokens + expanded.output_tokens as usize <= 4700 - 256);
        assert!(budget(2048, 1024).input_tokens <= 768);
        assert_eq!(budget(256, 256).input_tokens, 0);
    }

    #[test]
    fn longer_requests_receive_the_output_space_reserved_by_the_input_budget() {
        let graph = PromptNodeGraph::default();
        let budget = graph
            .text_translation_budget(
                TranslationPromptProfile::ChatMessages,
                "English",
                "Chinese",
                4700,
                256,
            )
            .unwrap();
        assert_eq!(budget.output_limit("Hello.", 256), 256);
        assert!(budget.output_limit(&"word ".repeat(300), 256) > 256);
        let full_input = "中".repeat(budget.input_tokens);
        assert_eq!(budget.output_limit(&full_input, 256), budget.output_tokens);
        assert_eq!(
            graph
                .translation_input_budget(
                    TranslationPromptProfile::ChatMessages,
                    "English",
                    "Chinese",
                    4700,
                    256,
                )
                .unwrap(),
            budget.input_tokens
        );
    }

    #[test]
    fn style_space_is_reserved_before_dividing_the_window() {
        let mut graph = PromptNodeGraph::default();
        let budget = |graph: &PromptNodeGraph| {
            graph
                .translation_input_budget(
                    TranslationPromptProfile::ChatMessages,
                    "English",
                    "Chinese",
                    4700,
                    256,
                )
                .unwrap()
        };
        let plain = budget(&graph);
        assert!(graph.set_style_text(&"Keep the original meaning. ".repeat(50)));
        assert!(budget(&graph) < plain);
    }
}
