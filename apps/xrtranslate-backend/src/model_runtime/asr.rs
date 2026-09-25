//! ASR provider profiles and provider-erased adapter dispatch.

use std::{collections::HashMap, path::PathBuf, sync::mpsc, thread};

use xrtranslate_assets::{
    AsrDelivery, AsrPromptStyle, ModelCapability, ModelFileRole, ModelRuntime, ResolvedModelAsset,
    manifests_for_capability,
};
use xrtranslate_inference::{
    AsrTranscript, AsrVocabularyBias, InferenceError, OpenAiAsrAdapter, OpenAiAsrOptions,
    Qwen3AsrAdapter, Qwen3AsrOptions, QwenAudioStreamingAdapter, QwenAudioStreamingOptions,
    ReqwestClient,
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

#[derive(Clone, Debug)]
pub(crate) struct SenseVoiceAdapter {
    requests: mpsc::Sender<SenseVoiceRequest>,
}

#[derive(Debug)]
struct SenseVoiceRequest {
    samples: Vec<f32>,
    language: String,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

impl SenseVoiceAdapter {
    pub(crate) fn new(model: PathBuf, tokens: PathBuf) -> Self {
        let (requests, receiver) = mpsc::channel::<SenseVoiceRequest>();
        thread::Builder::new()
            .name("sensevoice-asr".into())
            .spawn(move || {
                use sherpa_onnx::{
                    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
                };
                let mut recognizers: HashMap<String, OfflineRecognizer> = HashMap::new();
                for request in receiver {
                    let result = (|| {
                        if !recognizers.contains_key(&request.language) {
                            let mut config = OfflineRecognizerConfig::default();
                            config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                                model: Some(model.to_string_lossy().into_owned()),
                                language: Some(request.language.clone()),
                                use_itn: true,
                            };
                            config.model_config.tokens =
                                Some(tokens.to_string_lossy().into_owned());
                            config.model_config.provider = Some("cpu".into());
                            config.model_config.num_threads = 2;
                            let recognizer = OfflineRecognizer::create(&config)
                                .ok_or_else(|| "cannot create SenseVoice recognizer".to_owned())?;
                            recognizers.insert(request.language.clone(), recognizer);
                        }
                        let recognizer = &recognizers[&request.language];
                        let stream = recognizer.create_stream();
                        stream.accept_waveform(16_000, &request.samples);
                        recognizer.decode(&stream);
                        stream
                            .get_result()
                            .map(|result| result.text)
                            .ok_or_else(|| "SenseVoice returned no result".to_owned())
                    })();
                    let _ = request.reply.send(result);
                }
            })
            .expect("cannot start SenseVoice ASR worker");
        Self { requests }
    }

    async fn transcribe(
        &self,
        pcm: &[u8],
        language: Option<String>,
    ) -> Result<AsrTranscript, InferenceError> {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err(InferenceError::InvalidAudio {
                message: "SenseVoice requires nonempty 16-bit mono PCM".into(),
            });
        }
        let samples = pcm
            .chunks_exact(2)
            .map(|bytes| f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0)
            .collect();
        let normalized_language = match language.as_deref() {
            Some("zh-TW" | "zh-Hant") => "zh".to_owned(),
            Some(code) => code.to_owned(),
            None => "auto".to_owned(),
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        self.requests
            .send(SenseVoiceRequest {
                samples,
                language: normalized_language,
                reply,
            })
            .map_err(|error| InferenceError::InvalidResponse {
                endpoint: "sensevoice-onnx".into(),
                message: error.to_string(),
                body_preview: String::new(),
            })?;
        let text = result
            .await
            .map_err(|error| InferenceError::InvalidResponse {
                endpoint: "sensevoice-onnx".into(),
                message: error.to_string(),
                body_preview: String::new(),
            })?
            .map_err(|message| InferenceError::InvalidResponse {
                endpoint: "sensevoice-onnx".into(),
                message,
                body_preview: String::new(),
            })?;
        if text.trim().is_empty() {
            return Err(InferenceError::EmptyOutput {
                operation: "SenseVoice ASR",
            });
        }
        Ok(AsrTranscript { language, text })
    }
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
            Self::SenseVoice(adapter) => adapter.transcribe(pcm, options.language).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run with SENSEVOICE_TEST_DIR pointing at a verified model package plus
    /// the upstream test_wavs/en.wav fixture.
    #[tokio::test]
    #[ignore = "requires downloaded SenseVoiceSmall model and audio fixture"]
    async fn sensevoice_english_fixture_smoke() {
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
        let result = adapter.transcribe(&pcm, Some("en".into())).await.unwrap();
        assert!(!result.text.trim().is_empty());
        eprintln!("SenseVoice transcription: {}", result.text);
    }
}
