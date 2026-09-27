//! Fixed sentence-translation templates for checkpoints without instruction following.
use std::sync::OnceLock;

use super::GraphBuilder;
use crate::{
    PromptGraphError, PromptMessageRole, PromptNodeGraph, PromptNodePage, PromptProviderTarget,
    PromptVariable,
};

impl PromptNodeGraph {
    /// Haidass's training format. This graph deliberately has no style or reference inputs.
    /// https://huggingface.co/spaces/umeiko/haidass-translate-app/blob/main/app.py
    pub fn builtin_bilingual(target_language: &str) -> Result<&'static Self, PromptGraphError> {
        static TO_CHINESE: OnceLock<PromptNodeGraph> = OnceLock::new();
        static TO_ENGLISH: OnceLock<PromptNodeGraph> = OnceLock::new();
        let (graph, instruction) = match target_language {
            "Chinese" => (
                &TO_CHINESE,
                "Translate the following text from English to Simplified Chinese.\n{0}",
            ),
            "English" => (&TO_ENGLISH, "请将以下简体中文翻译成英文。\n{0}"),
            _ => {
                return Err(PromptGraphError::new(
                    "bilingual translation requires Chinese or English as the target",
                ));
            }
        };
        Ok(graph.get_or_init(|| {
            let mut builder = GraphBuilder::default();
            let page = PromptNodePage::Shared;
            builder.variable("bilingual-input", page, PromptVariable::CurrentInput);
            builder.compose(
                "bilingual-prompt",
                page,
                "Translation",
                instruction,
                &["bilingual-input"],
            );
            for (id, target) in [
                ("bilingual-hunyuan-request", PromptProviderTarget::Hunyuan),
                (
                    "bilingual-openai-request",
                    PromptProviderTarget::OpenAiCompatible,
                ),
            ] {
                builder.output(id, target, &[(PromptMessageRole::User, "bilingual-prompt")]);
            }
            builder.finish()
        }))
    }
}
