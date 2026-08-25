//! ASR provider profiles and provider-erased adapter dispatch.

use xrtranslate_assets::ModelAssetId;
use xrtranslate_inference::{
    AsrTranscript, AsrVocabularyBias, InferenceError, OpenAiAsrAdapter, OpenAiAsrOptions,
    Qwen3AsrAdapter, Qwen3AsrOptions, QwenAudioStreamingAdapter, QwenAudioStreamingOptions,
    ReqwestClient,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AsrProfile {
    Qwen3Local,
    OpenAiAudio,
    QwenAudioStreaming,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeAsrOptions {
    pub(crate) language: Option<String>,
    pub(crate) instruction_prompt: Option<String>,
    pub(crate) context_bias: Option<String>,
    pub(crate) vocabulary_bias: Vec<AsrVocabularyBias>,
    pub(crate) max_tokens: u32,
}

/// Provider-erased ASR adapter consumed by the generic pipeline. New native
/// ASR families add one dispatch variant here without leaking their options
/// into session processing.
#[derive(Clone, Debug)]
pub(crate) enum NativeAsrAdapter {
    Qwen3(Qwen3AsrAdapter<ReqwestClient>),
    OpenAi(OpenAiAsrAdapter<ReqwestClient>),
    QwenAudioStreaming(QwenAudioStreamingAdapter),
}

impl AsrProfile {
    pub(super) fn registered(provider: &str, transport: &str) -> Option<Self> {
        if provider == "qwen-audio-streaming" && transport == "websocket" {
            return Some(Self::QwenAudioStreaming);
        }
        if transport == "openai" {
            return Some(Self::OpenAiAudio);
        }
        match provider {
            "qwen3-gguf" => Some(Self::Qwen3Local),
            _ => None,
        }
    }

    pub(super) const fn default_asset(self) -> ModelAssetId {
        match self {
            Self::Qwen3Local => ModelAssetId::Qwen3AsrGguf,
            Self::OpenAiAudio | Self::QwenAudioStreaming => ModelAssetId::Qwen3AsrGguf,
        }
    }

    pub(super) fn model_alias<'a>(self, configured: &'a str) -> &'a str {
        match self {
            Self::Qwen3Local => "qwen3-asr",
            Self::OpenAiAudio | Self::QwenAudioStreaming => configured,
        }
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<NativeAsrAdapter, InferenceError> {
        match self {
            Self::Qwen3Local => {
                Qwen3AsrAdapter::new(http, endpoint, model).map(NativeAsrAdapter::Qwen3)
            }
            Self::OpenAiAudio => OpenAiAsrAdapter::with_bearer_token(
                http,
                endpoint,
                model,
                api_key.unwrap_or_default(),
            )
            .map(NativeAsrAdapter::OpenAi),
            Self::QwenAudioStreaming => {
                QwenAudioStreamingAdapter::new(endpoint, model, api_key.unwrap_or_default())
                    .map(NativeAsrAdapter::QwenAudioStreaming)
            }
        }
    }
}

impl NativeAsrAdapter {
    pub(crate) async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: NativeAsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        match self {
            Self::Qwen3(adapter) => {
                adapter
                    .transcribe_pcm16(
                        pcm,
                        Qwen3AsrOptions {
                            language: options.language,
                            context_bias: options.context_bias,
                            vocabulary_bias: options.vocabulary_bias,
                            instruction_prompt: options.instruction_prompt,
                            max_tokens: options.max_tokens,
                        },
                    )
                    .await
            }
            Self::OpenAi(adapter) => {
                adapter
                    .transcribe_pcm16(
                        pcm,
                        OpenAiAsrOptions {
                            language: options.language,
                            instruction_prompt: options.instruction_prompt,
                            max_tokens: options.max_tokens,
                        },
                    )
                    .await
            }
            Self::QwenAudioStreaming(adapter) => {
                adapter
                    .transcribe_pcm16(
                        pcm,
                        QwenAudioStreamingOptions {
                            language: options.language,
                            context_bias: options.context_bias,
                            vocabulary_bias: options.vocabulary_bias,
                        },
                    )
                    .await
            }
        }
    }
}
