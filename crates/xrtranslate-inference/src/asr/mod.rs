//! Provider-neutral ASR types and concrete provider adapters.
//!
//! Shared callers depend on the types re-exported here. Provider modules own
//! authentication, transport, wire formats, and provider-specific limits.

mod providers;
mod types;

#[cfg(feature = "sensevoice")]
pub use providers::SenseVoiceAdapter;
pub use providers::{
    OpenAiAsrAdapter, Qwen3AsrAdapter, QwenAudioFlashAdapter, QwenAudioStreamingAdapter,
    is_probable_asr_hallucination,
};
pub use types::{AsrOptions, AsrTranscript, AsrVocabularyBias};
