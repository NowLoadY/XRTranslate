//! Shared session contracts and host event adapters.
mod result;
mod subscriber;
pub(crate) use result::TranslationEventAdapter;
pub use subscriber::{CaptionUpdate, HostOutputEvent, HostOutputSubscriber};
pub use xrtranslate_client_core::session_coordinator::*;
