use xrtranslate_prompt::{PromptExecutionTrace, PromptNodeGraph, TranslationPromptContext};

/// Prompt style selected for a translation endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationProvider {
    /// Hy-MT2's direct, single-user-message instruction format.
    Hunyuan,
    /// Generic OpenAI-compatible instruction/messages format (including Groq).
    OpenAiCompatible,
    /// Qwen-MT translation format over DashScope OpenAI-compatible endpoint.
    Qwen,
}

/// Options that accompany a single source segment.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOptions {
    pub source_language: String,
    pub target_language: String,
    pub prompt_graph: PromptNodeGraph,
    pub prompt_context: TranslationPromptContext,
    /// Context capacity available to one endpoint request. Provider profiles
    /// use this when a sampling parameter needs the concrete slot window.
    pub context_window_tokens: u32,
    pub max_tokens: u32,
}

impl TranslationOptions {
    pub fn new(source_language: impl Into<String>, target_language: impl Into<String>) -> Self {
        Self {
            source_language: source_language.into(),
            target_language: target_language.into(),
            prompt_graph: PromptNodeGraph::builtin_default(),
            prompt_context: TranslationPromptContext::default(),
            context_window_tokens: 2_048,
            max_tokens: 256,
        }
    }
}

/// A translated segment after model-output cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationResult {
    pub text: String,
    pub prompt_trace: PromptExecutionTrace,
}
