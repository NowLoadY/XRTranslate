use crate::{
    PromptGraphError, PromptNodeGraph, TranslationPromptContext, TranslationPromptProfile,
    estimated_tokens,
};

impl PromptNodeGraph {
    /// Estimated capacity for one complete text translation. Reserve the configured
    /// output allowance and prompt overhead; the output limit also bounds the source
    /// so a large context window does not imply an equally large possible translation.
    pub fn translation_input_budget(
        &self,
        profile: TranslationPromptProfile,
        source: &str,
        target: &str,
        context_tokens: u32,
        output_tokens: u32,
    ) -> Result<usize, PromptGraphError> {
        let prompt = self.render(
            profile.target(),
            "x",
            source,
            target,
            &TranslationPromptContext::default(),
        )?;
        let overhead = prompt
            .messages
            .iter()
            .map(|message| estimated_tokens(&message.content) + 8)
            .sum::<usize>()
            .saturating_add(64)
            .max(256);
        Ok((context_tokens as usize)
            .saturating_sub(output_tokens as usize)
            .saturating_sub(overhead)
            .min(output_tokens as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_reserves_output_and_respects_its_limit() {
        let graph = PromptNodeGraph::default();
        let budget = |context, output| {
            graph
                .translation_input_budget(
                    TranslationPromptProfile::ChatMessages,
                    "English",
                    "Chinese",
                    context,
                    output,
                )
                .unwrap()
        };
        assert_eq!(budget(8192, 1024), 1024);
        assert!(budget(2048, 1024) <= 768);
        assert_eq!(budget(256, 256), 0);
    }
}
