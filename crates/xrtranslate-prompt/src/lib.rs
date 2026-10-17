//! Shared prompt graph domain for the desktop editor, wire protocol, and
//! inference adapters.

#![forbid(unsafe_code)]

mod builtin;
mod context;
mod execution;
mod library;
mod profile;
mod schema;
mod template;
mod translation_style;

pub use context::{
    AsrPromptContext, ContextFacts, PromptTurn, SurroundingSource, TaskContext, TranslationPromptBlock,
    TranslationPromptContext,
};
pub use execution::{
    PromptExecution, PromptExecutionTrace, PromptMessage, PromptNodeTrace, PromptRender,
};
pub use library::{PromptMode, PromptTemplateLibrary, PromptTemplateProfile};
pub use profile::TranslationPromptProfile;
pub use schema::{
    PromptCondition, PromptGraphDomain, PromptGraphError, PromptLink, PromptMessageRole,
    PromptNode, PromptNodeGraph, PromptNodeKind, PromptNodePage, PromptTarget,
    PromptSystemValue, PromptTextComparison, PromptValueType, PromptVariable, condition_label,
    system_value_label,
};
pub use template::compose_input_indexes;
pub use translation_style::{TranslationStyle, TranslationStylePreset};
