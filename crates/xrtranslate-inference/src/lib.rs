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
mod translation;
mod tts;
mod wav;

#[cfg(feature = "sensevoice")]
pub use asr::SenseVoiceAdapter;
pub use asr::{
    AsrOptions, AsrTranscript, AsrVocabularyBias, OpenAiAsrAdapter, Qwen3AsrAdapter,
    QwenAudioFlashAdapter, QwenAudioStreamingAdapter, is_probable_asr_hallucination,
};
pub use error::{InferenceError, TransportError};
pub use http::{AsyncHttpClient, HttpRequest, HttpResponse, MultipartBody, ReqwestClient};
pub use openai::{ChatCompletion, OpenAiCompatibleClient};
pub use translation::{
    PromptCondition, PromptGraphError, PromptLink, PromptMessage, PromptMessageRole, PromptNode,
    PromptNodeGraph, PromptNodeKind, PromptTarget, PromptTemplateLibrary,
    PromptTemplateProfile, PromptTurn, PromptVariable, SurroundingSource, TranslationAdapter,
    TranslationOptions, TranslationPromptBlock, TranslationPromptContext, TranslationPromptProfile,
    TranslationResult, build_translation_messages, is_probable_translation_context_leak,
};
pub use tts::{
    Audio8ExecutionDevice, Audio8OnnxAdapter, Audio8SynthesisOptions, OnnxExecutionDevice,
    OpenVoiceBaseVoice, OpenVoiceOnnxAdapter, OpenVoiceSynthesisOptions, RemoteTtsAdapter,
    SynthesizedPcm, initialize_onnx_runtime, preload_onnx_cuda_libraries,
};
pub use wav::{PCM16_MONO_16KHZ_FORMAT, pcm16_mono_16khz_to_wav};
