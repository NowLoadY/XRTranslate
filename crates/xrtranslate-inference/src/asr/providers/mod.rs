mod openai_chat;
mod qwen3;
mod qwen_audio;
mod qwen_audio_flash;
mod qwen_audio_streaming;
#[cfg(feature = "sherpa")]
mod sherpa;

#[cfg(feature = "sherpa")]
pub use sherpa::SherpaOfflineAsrAdapter;

pub use openai_chat::OpenAiAsrAdapter;
pub use qwen_audio_flash::QwenAudioFlashAdapter;
pub use qwen_audio_streaming::QwenAudioStreamingAdapter;
pub use qwen3::{Qwen3AsrAdapter, is_probable_asr_hallucination};
