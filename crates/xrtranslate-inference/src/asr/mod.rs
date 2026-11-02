//! Provider-neutral ASR types and concrete provider adapters.
//!
//! Shared callers depend on the types re-exported here. Provider modules own
//! authentication, transport, wire formats, and provider-specific limits.

mod audio;
mod providers;
mod quality;
mod types;

#[cfg(feature = "sherpa")]
pub use providers::SherpaOfflineAsrAdapter;
pub use providers::{
    DoubaoWebAsrAdapter, OpenAiAsrAdapter, Qwen3AsrAdapter, QwenAudioFlashAdapter,
    QwenAudioStreamingAdapter,
};
pub use quality::is_probable_asr_hallucination;
pub use types::{AsrOptions, AsrTranscript, AsrVocabularyBias};
