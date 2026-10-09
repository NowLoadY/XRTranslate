//! Host-side contract between translation infrastructure and feature plugins.
//!
//! This module deliberately contains no concrete plugin imports. Plugins
//! describe how they want to use a translation session and may subscribe to
//! the resulting translation results; the network/audio implementation
//! remains unaware of every plugin.

mod owner;
mod plugin_session;
mod request;
mod result;
mod subscriber;

pub use owner::{PluginSessionOwner, TranslationSessionOwner};
pub use plugin_session::{PluginSessionBinding, SessionOutputPolicy, TranslationSessionPlugin};
pub(crate) use request::{TranslationInput, TranslationTask};
pub(crate) use result::TranslationEventAdapter;
pub use result::{TranslationEvent, TranslationOutcome, TranslationSegment};
pub use subscriber::{
    AudioSourceFilter, CaptionUpdate, HostOutputEvent, HostOutputSubscriber, SessionEventSubscriber,
};
