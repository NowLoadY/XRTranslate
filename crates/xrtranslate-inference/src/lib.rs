//! Native inference adapters for the Python-free XRTranslate backend.
//!
//! The crate deliberately speaks the OpenAI-compatible HTTP contract exposed
//! by `llama-server` instead of linking llama.cpp into the desktop process.
//! Its transport is a trait so session code can use a real [`ReqwestClient`]
//! while tests exercise the exact JSON contract without a running model.

#![forbid(unsafe_code)]

mod asr;
mod error;
mod http;
mod openai;
mod openai_asr;
mod qwen3;
mod qwen_audio_streaming;
mod translation;
mod tts;
mod tts_onnx;
mod wav;

pub use asr::AsrVocabularyBias;
pub use error::{InferenceError, TransportError};
pub use http::{AsyncHttpClient, HttpRequest, HttpResponse, ReqwestClient};
pub use openai::{ChatCompletion, OpenAiCompatibleClient};
pub use openai_asr::{OpenAiAsrAdapter, OpenAiAsrOptions};
pub use qwen_audio_streaming::{QwenAudioStreamingAdapter, QwenAudioStreamingOptions};
pub use qwen3::{AsrTranscript, Qwen3AsrAdapter, Qwen3AsrOptions, is_probable_asr_hallucination};
pub use translation::{
    PromptCondition, PromptGraphError, PromptLink, PromptMessage, PromptMessageRole, PromptNode,
    PromptNodeGraph, PromptNodeKind, PromptProviderTarget, PromptTemplateLibrary,
    PromptTemplateProfile, PromptTurn, PromptVariable, SurroundingSource, TranslationAdapter,
    TranslationOptions, TranslationPromptBlock, TranslationPromptContext, TranslationProvider,
    TranslationResult, build_translation_messages, is_probable_translation_context_leak,
};
pub use tts::SynthesizedPcm;
pub use tts_onnx::{
    Audio8ExecutionDevice, Audio8OnnxAdapter, Audio8SynthesisOptions, initialize_onnx_runtime,
    preload_onnx_cuda_libraries,
};
pub use wav::{PCM16_MONO_16KHZ_FORMAT, pcm16_mono_16khz_to_wav};
