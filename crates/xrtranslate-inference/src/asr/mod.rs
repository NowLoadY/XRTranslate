//! Provider-neutral ASR types and concrete provider adapters.
//!
//! Shared callers depend on the types re-exported here. Provider modules own
//! authentication, transport, wire formats, and provider-specific limits.

mod providers;
mod types;

pub use providers::{
    OpenAiAsrAdapter, OpenAiAsrOptions, Qwen3AsrAdapter, Qwen3AsrOptions,
    QwenAudioStreamingAdapter, QwenAudioStreamingOptions, is_probable_asr_hallucination,
};
pub use types::{AsrTranscript, AsrVocabularyBias};
