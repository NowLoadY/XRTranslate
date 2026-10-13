//! ASR provider profiles and provider-erased adapter dispatch.

use xrtranslate_assets::remote::{RemoteCapabilities, RemoteConnector};
use xrtranslate_assets::{
    AsrDelivery, AsrPromptStyle, ModelCapability, ModelFileRole, ModelRuntime, ResolvedModelAsset,
    manifests_for_capability,
};
use xrtranslate_config::NativeProviderConfig;
use xrtranslate_inference::{
    AsrOptions, AsrTranscript, InferenceError, OpenAiAsrAdapter, Qwen3AsrAdapter,
    QwenAudioFlashAdapter, QwenAudioStreamingAdapter, ReqwestClient, SenseVoiceAdapter,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AsrProfile {
    LocalCatalog,
    Remote(RemoteConnector),
}

/// Provider-erased ASR adapter consumed by the generic pipeline. New native
/// ASR families add one dispatch variant here without leaking their options
/// into session processing.
#[derive(Clone, Debug)]
pub(crate) enum NativeAsrAdapter {
    AudioChat(Qwen3AsrAdapter<ReqwestClient>),
    OpenAi(OpenAiAsrAdapter<ReqwestClient>),
    QwenAudioFlash(QwenAudioFlashAdapter<ReqwestClient>),
    QwenAudioStreaming(QwenAudioStreamingAdapter),
    SenseVoice(SenseVoiceAdapter),
}

impl AsrProfile {
    pub(super) fn registered(config: &NativeProviderConfig) -> Option<Self> {
        if config.uses_local_runtime() {
            return manifests_for_capability(ModelCapability::Asr)
                .any(|manifest| manifest.provider == config.provider)
                .then_some(Self::LocalCatalog);
        }
        config
            .connector
            .filter(|connector| connector.capability() == ModelCapability::Asr)
            .map(Self::Remote)
    }

    pub(super) fn adapter(
        self,
        http: ReqwestClient,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
        capabilities: &RemoteCapabilities,
    ) -> Result<NativeAsrAdapter, InferenceError> {
        match self {
            Self::Remote(
                connector @ (RemoteConnector::OpenaiTranscription
                | RemoteConnector::OpenaiAudioChat),
            ) => OpenAiAsrAdapter::new(
                http,
                endpoint,
                model,
                api_key.unwrap_or_default(),
                connector,
                capabilities,
            )
            .map(NativeAsrAdapter::OpenAi),
            Self::Remote(RemoteConnector::DashscopeAsr) => {
                QwenAudioFlashAdapter::new(http, endpoint, model, api_key.unwrap_or_default())
                    .map(NativeAsrAdapter::QwenAudioFlash)
            }
            Self::Remote(RemoteConnector::DashscopeAsrWebsocket) => {
                QwenAudioStreamingAdapter::new(endpoint, model, api_key.unwrap_or_default())
                    .map(NativeAsrAdapter::QwenAudioStreaming)
            }
            _ => Err(InferenceError::InvalidConfiguration {
                field: "asr.connector",
                message: "expected a remote ASR connector".into(),
            }),
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
    /// Connectors normalize their wire events to cumulative preview snapshots.
    /// Each callback replaces the preview; an empty snapshot retracts it. These
    /// updates never commit history or translation, regardless of punctuation.
    /// Only a successful return supplies the completed transcript. EOF must
    /// drain input and await provider completion; cancellation is not completion.
    pub(crate) async fn transcribe_stream<F, Fut>(
        &self,
        audio: tokio::sync::mpsc::Receiver<Vec<u8>>,
        options: AsrOptions,
        on_update: F,
    ) -> Result<AsrTranscript, InferenceError>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let audio = futures_util::stream::unfold(audio, |mut audio| async move {
            audio.recv().await.map(|frame| (frame, audio))
        });
        match self {
            Self::QwenAudioStreaming(adapter) => {
                adapter.transcribe_stream(audio, options, on_update).await
            }
            _ => Err(InferenceError::InvalidConfiguration {
                field: "asr.connector",
                message: "this connector does not accept live audio".into(),
            }),
        }
    }

    pub(crate) async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        match self {
            Self::AudioChat(adapter) => adapter.transcribe_pcm16(pcm, options).await,
            Self::OpenAi(adapter) => adapter.transcribe_pcm16(pcm, options).await,
            Self::QwenAudioFlash(adapter) => adapter.transcribe_pcm16(pcm, options).await,
            Self::QwenAudioStreaming(adapter) => adapter.transcribe_pcm16(pcm, options).await,
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

    /// Exercises the same automatic and explicit language paths as the pipeline.
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
        let AutoDecision::Accept(route) =
            route.classify(automatic.language.as_deref(), &automatic.text)
        else {
            panic!("expected direct routing for a clear English transcript");
        };
        assert_eq!(route.source.code(), "en");
        let explicit = adapter.transcribe_pcm16(&pcm, Some("en")).await.unwrap();
        assert_eq!(explicit.text, automatic.text);
        assert_eq!(explicit.language.as_deref(), Some("en"));
    }
}
