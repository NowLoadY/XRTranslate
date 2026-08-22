//! Shared prompt graph domain for the desktop editor, wire protocol, and
//! inference adapters.

#![forbid(unsafe_code)]

mod builtin;
mod context;
mod execution;
mod library;
mod schema;
mod template;

pub use context::{
    AsrPromptContext, PromptTurn, SurroundingSource, TranslationPromptBlock,
    TranslationPromptContext,
};
pub use execution::{
    PromptExecution, PromptExecutionTrace, PromptMessage, PromptNodeTrace, PromptRender,
};
pub use library::{PromptTemplateLibrary, PromptTemplateProfile};
pub use schema::{
    PromptCondition, PromptGraphError, PromptLink, PromptMessageRole, PromptNode, PromptNodeGraph,
    PromptNodeKind, PromptNodePage, PromptProviderTarget, PromptVariable,
};
pub use template::compose_input_indexes;
