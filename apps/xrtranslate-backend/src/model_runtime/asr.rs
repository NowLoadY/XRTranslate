//! ASR provider profiles and provider-erased adapter dispatch.

use xrtranslate_assets::{
    AsrDelivery, AsrPromptStyle, ModelCapability, ModelFileRole, ModelRuntime, ResolvedModelAsset,
    manifests_for_capability,
};
use xrtranslate_inference::{
    AsrTranscript, AsrVocabularyBias, InferenceError, OpenAiAsrAdapter, OpenAiAsrOptions,
    Qwen3AsrAdapter, Qwen3AsrOptions, QwenAudioStreamingAdapter, QwenAudioStreamingOptions,
    ReqwestClient, SenseVoiceAdapter,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AsrProfile {
    LocalCatalog,
    Qwen3Remote,
    OpenAiAudio,
    QwenAudioStreaming,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeAsrOptions {
    /// Canonical code from the shared catalogue; None selects automatic ASR.
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
    AudioChat(Qwen3AsrAdapter<ReqwestClient>),
    OpenAi(OpenAiAsrAdapter<ReqwestClient>),
    QwenAudioStreaming(QwenAudioStreamingAdapter),
    SenseVoice(SenseVoiceAdapter),
}

impl AsrProfile {
    pub(super) fn registered(provider: &str, transport: &str) -> Option<Self> {
        if matches!(transport, "local" | "onnx-cpu") {
            return manifests_for_capability(ModelCapability::Asr)
                .any(|manifest| manifest.provider == provider)
                .then_some(Self::LocalCatalog);
        }
        if provider == "qwen-audio-streaming" && transport == "websocket" {
            return Some(Self::QwenAudioStreaming);
        }
        if provider == "qwen" || provider == "qwen-intl" {
            return Some(Self::Qwen3Remote);
        }
        if transport == "openai" {
            return Some(Self::OpenAiAudio);
        }
        None
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<NativeAsrAdapter, InferenceError> {
        match self {
            Self::LocalCatalog => Err(InferenceError::InvalidConfiguration {
                field: "asr.model_asset.runtime",
                message: "local ASR adapters must be constructed from the model card".into(),
            }),
            Self::Qwen3Remote => Qwen3AsrAdapter::with_bearer_token(
                http,
                endpoint,
                model,
                api_key.unwrap_or_default(),
            )
            .map(NativeAsrAdapter::AudioChat),
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

pub(super) fn local_adapter(
    runtime: ModelRuntime,
    asset: &ResolvedModelAsset,
    http: ReqwestClient,
    endpoint: &str,
) -> Result<NativeAsrAdapter, InferenceError> {
    let path = |role| {
        super::model_file(asset, role).map_err(|message| InferenceError::InvalidConfiguration {
            field: "asr.model_asset",
            message,
        })
    };
    match runtime {
        ModelRuntime::LlamaAudioChat {
            model_alias,
            delivery: AsrDelivery::Utterance,
            prompt_style: AsrPromptStyle::QwenAsr,
            ..
        } => Qwen3AsrAdapter::new(http, endpoint, model_alias).map(NativeAsrAdapter::AudioChat),
        ModelRuntime::SherpaOfflineAsr {
            delivery: AsrDelivery::Utterance,
        } => Ok(NativeAsrAdapter::SenseVoice(SenseVoiceAdapter::new(
            path(ModelFileRole::Weights)?,
            path(ModelFileRole::Tokenizer)?,
        ))),
        _ => Err(InferenceError::InvalidConfiguration {
            field: "asr.model_asset.runtime",
            message: "this ASR execution mode is not supported by the current pipeline".into(),
        }),
    }
}

impl NativeAsrAdapter {
    pub(crate) async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: NativeAsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        match self {
            Self::AudioChat(adapter) => {
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
            Self::SenseVoice(adapter) => {
                adapter
                    .transcribe_pcm16(pcm, options.language.as_deref())
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Exercises the same language codes sent by the pipeline, including its
    /// constrained recovery after an automatic recognition attempt.
    #[tokio::test]
    #[ignore = "requires downloaded SenseVoiceSmall model and audio fixture"]
    async fn sensevoice_pipeline_language_fixture_smoke() {
        use crate::language::{AdaptiveLanguageRoute, AutoDecision};

        let root =
            PathBuf::from(std::env::var_os("SENSEVOICE_TEST_DIR").expect("SENSEVOICE_TEST_DIR"));
        let wave = sherpa_onnx::Wave::read(root.join("en.wav").to_str().unwrap()).unwrap();
        assert_eq!(wave.sample_rate(), 16_000);
        let pcm = wave
            .samples()
            .iter()
            .flat_map(|sample| ((*sample * 32767.0) as i16).to_le_bytes())
            .collect::<Vec<_>>();
        let adapter = SenseVoiceAdapter::new(root.join("model.int8.onnx"), root.join("tokens.txt"));
        let automatic = adapter.transcribe_pcm16(&pcm, None).await.unwrap();
        assert!(!automatic.text.trim().is_empty());

        let mut route = AdaptiveLanguageRoute::default();
        route.configure("auto", "zh,en");
        let AutoDecision::Retry { language, .. } =
            route.classify(automatic.language.as_deref(), &automatic.text)
        else {
            panic!("expected constrained recovery for unlabelled automatic output");
        };
        let recovered = adapter
            .transcribe_pcm16(&pcm, Some(language.code()))
            .await
            .unwrap();
        assert_eq!(recovered.text, automatic.text);
        let explicit = adapter.transcribe_pcm16(&pcm, Some("en")).await.unwrap();
        assert_eq!(explicit.text, recovered.text);
        assert_eq!(explicit.language.as_deref(), Some("en"));
    }
}
