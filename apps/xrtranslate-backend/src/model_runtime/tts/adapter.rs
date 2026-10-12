//! Provider-erased TTS capability consumed by session orchestration.

use std::sync::Arc;
use xrtranslate_engine::language::SupportedLanguage;

use xrtranslate_inference::{
    Audio8OnnxAdapter, InferenceError, OnnxExecutionDevice, OpenVoiceOnnxAdapter, RemoteTtsAdapter,
    SynthesizedPcm, TransportError,
};

#[derive(Clone)]
pub(crate) struct TtsAdapter {
    models: Arc<[TtsModel]>,
}

#[derive(Clone)]
enum TtsBackend {
    OpenVoice(OpenVoiceOnnxAdapter),
    Audio8(Audio8OnnxAdapter),
    Remote(RemoteTtsAdapter),
}

#[derive(Clone)]
struct TtsModel {
    backend: TtsBackend,
    supported_languages: Arc<[String]>,
}

impl TtsAdapter {
    pub(super) fn remote(adapter: RemoteTtsAdapter, supported_languages: Vec<String>) -> Self {
        Self {
            models: vec![TtsModel {
                backend: TtsBackend::Remote(adapter),
                supported_languages: supported_languages.into(),
            }]
            .into(),
        }
    }

    pub(crate) fn supports_cloning(&self) -> bool {
        self.models.iter().all(|model| match &model.backend {
            TtsBackend::Remote(adapter) => adapter.capabilities().voice_cloning,
            _ => true,
        })
    }

    pub(crate) fn is_local(&self) -> bool {
        self.models
            .iter()
            .all(|model| !matches!(model.backend, TtsBackend::Remote(_)))
    }

    pub(crate) fn preset_voices(&self) -> Vec<String> {
        self.models
            .iter()
            .flat_map(|model| match &model.backend {
                TtsBackend::Remote(adapter) => adapter.capabilities().voices.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    pub(super) fn openvoice(
        adapter: OpenVoiceOnnxAdapter,
        supported_languages: Vec<String>,
    ) -> Self {
        Self {
            models: vec![TtsModel {
                backend: TtsBackend::OpenVoice(adapter),
                supported_languages: supported_languages.into(),
            }]
            .into(),
        }
    }

    pub(super) fn audio8(adapter: Audio8OnnxAdapter, supported_languages: Vec<String>) -> Self {
        Self {
            models: vec![TtsModel {
                backend: TtsBackend::Audio8(adapter),
                supported_languages: supported_languages.into(),
            }]
            .into(),
        }
    }

    pub(super) fn combine(adapters: Vec<Self>) -> Result<Self, String> {
        let models = adapters
            .into_iter()
            .flat_map(|adapter| adapter.models.iter().cloned().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        if models.is_empty() {
            return Err(
                "a native TTS provider must activate at least one model package".to_owned(),
            );
        }
        Ok(Self {
            models: models.into(),
        })
    }

    /// An empty list means the provider accepts every language. Otherwise a
    /// locale such as `en-US` is matched against both its full tag and base.
    pub(crate) fn supports_language(&self, language: &str) -> bool {
        self.models
            .iter()
            .any(|model| language_is_supported(&model.supported_languages, language))
    }

    pub(crate) async fn prepare(&self) -> Result<Option<OnnxExecutionDevice>, InferenceError> {
        let mut prepared_device = None;
        for model in self.models.iter() {
            let device = match &model.backend {
                TtsBackend::OpenVoice(adapter) => adapter.prepare().await?,
                TtsBackend::Audio8(adapter) => adapter.prepare().await?,
                TtsBackend::Remote(_) => continue,
            };
            if prepared_device.is_some_and(|prepared| prepared != device) {
                return Err(InferenceError::InvalidConfiguration {
                    field: "tts.models",
                    message: "active TTS language packs resolved to different execution devices"
                        .to_owned(),
                });
            }
            prepared_device = Some(device);
        }
        Ok(prepared_device)
    }

    pub(crate) async fn has_voice(&self, name: &str) -> bool {
        for model in self.models.iter() {
            let found = match &model.backend {
                TtsBackend::OpenVoice(adapter) => adapter.has_voice(name).await,
                TtsBackend::Audio8(adapter) => adapter.has_voice(name).await,
                TtsBackend::Remote(adapter) => adapter
                    .capabilities()
                    .voices
                    .iter()
                    .any(|voice| voice == name),
            };
            if !found {
                return false;
            }
        }
        true
    }

    pub(crate) async fn register_voice(
        &self,
        name: &str,
        reference_wav: Vec<u8>,
        transcript: &str,
    ) -> Result<(), InferenceError> {
        for model in self.models.iter() {
            match &model.backend {
                TtsBackend::Remote(_) => {
                    return Err(InferenceError::InvalidConfiguration {
                        field: "tts.voice",
                        message: "this connector supports preset voices only".into(),
                    });
                }
                TtsBackend::OpenVoice(adapter) => {
                    adapter
                        .register_voice(name, reference_wav.clone(), transcript)
                        .await?;
                }
                TtsBackend::Audio8(adapter) => {
                    adapter
                        .register_voice(name, reference_wav.clone(), transcript)
                        .await?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        target_lang: &str,
    ) -> Result<SynthesizedPcm, InferenceError> {
        let mut output = SynthesizedPcm {
            bytes: Vec::new(),
            sample_rate: 0,
        };
        self.synthesize_stream(text, voice, target_lang, |audio| {
            output.sample_rate = audio.sample_rate;
            output.bytes.extend_from_slice(&audio.bytes);
            std::future::ready(Ok(()))
        })
        .await?;
        Ok(output)
    }

    pub(crate) async fn synthesize_stream<F, Fut>(
        &self,
        text: &str,
        voice: &str,
        target_lang: &str,
        mut on_audio: F,
    ) -> Result<(), InferenceError>
    where
        F: FnMut(SynthesizedPcm) -> Fut + Send,
        Fut: Future<Output = Result<(), TransportError>> + Send,
    {
        let model = self
            .models
            .iter()
            .find(|model| language_is_supported(&model.supported_languages, target_lang))
            .ok_or_else(|| InferenceError::InvalidConfiguration {
                field: "tts.target_language",
                message: format!("no active TTS model supports {target_lang:?}"),
            })?;
        let audio = match &model.backend {
            TtsBackend::Remote(adapter) => return adapter.synthesize(text, voice, on_audio).await,
            TtsBackend::OpenVoice(adapter) => adapter.synthesize(text, voice, target_lang).await?,
            TtsBackend::Audio8(adapter) => adapter.synthesize(text, voice).await?,
        };
        on_audio(audio)
            .await
            .map_err(|source| InferenceError::Transport {
                endpoint: "tts-output".into(),
                source,
            })
    }
}

fn language_is_supported(supported_languages: &[String], language: &str) -> bool {
    let parsed = SupportedLanguage::parse(language);
    supported_languages.is_empty()
        || supported_languages.iter().any(|supported| {
            match (SupportedLanguage::parse(supported), parsed) {
                (Some(supported), Some(language)) => supported.base_code() == language.base_code(),
                _ => supported.eq_ignore_ascii_case(language.trim()),
            }
        })
}
