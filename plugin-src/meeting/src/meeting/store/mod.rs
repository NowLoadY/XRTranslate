//! Transactional SQLite persistence for meeting capture.
//!
//! The public surface stays small while models, Markdown export, schema
//! migration, and repository operations remain independently understandable.

mod export;
mod model;
mod repository;
mod schema;
mod translation;

pub use export::render_markdown;
pub use model::*;
pub use repository::MeetingStore;
pub(super) use translation::TranslationWindow;
