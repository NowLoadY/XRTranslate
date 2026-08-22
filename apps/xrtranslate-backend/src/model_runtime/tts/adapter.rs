//! Provider-erased TTS capability consumed by session orchestration.

use std::sync::Arc;

use xrtranslate_inference::{
    Audio8OnnxAdapter, InferenceError, OnnxExecutionDevice, OpenVoiceOnnxAdapter, SynthesizedPcm,
};

#[derive(Clone)]
pub(crate) struct NativeTtsAdapter {
    models: Arc<[NativeTtsModel]>,
}

#[derive(Clone)]
enum NativeTtsBackend {
    OpenVoice(OpenVoiceOnnxAdapter),
    Audio8(Audio8OnnxAdapter),
}

#[derive(Clone)]
struct NativeTtsModel {
    backend: NativeTtsBackend,
    supported_languages: Arc<[String]>,
}

impl NativeTtsAdapter {
    pub(super) fn openvoice(
        adapter: OpenVoiceOnnxAdapter,
        supported_languages: Vec<String>,
    ) -> Self {
        Self {
            models: vec![NativeTtsModel {
                backend: NativeTtsBackend::OpenVoice(adapter),
                supported_languages: supported_languages.into(),
            }]
            .into(),
        }
    }

    pub(super) fn audio8(adapter: Audio8OnnxAdapter, supported_languages: Vec<String>) -> Self {
        Self {
            models: vec![NativeTtsModel {
                backend: NativeTtsBackend::Audio8(adapter),
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

    pub(crate) async fn prepare(&self) -> Result<OnnxExecutionDevice, InferenceError> {
        let mut prepared_device = None;
        for model in self.models.iter() {
            let device = match &model.backend {
                NativeTtsBackend::OpenVoice(adapter) => adapter.prepare().await?,
                NativeTtsBackend::Audio8(adapter) => adapter.prepare().await?,
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
        Ok(prepared_device.expect("validated TTS model group"))
    }

    pub(crate) async fn has_voice(&self, name: &str) -> bool {
        for model in self.models.iter() {
            let found = match &model.backend {
                NativeTtsBackend::OpenVoice(adapter) => adapter.has_voice(name).await,
                NativeTtsBackend::Audio8(adapter) => adapter.has_voice(name).await,
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
                NativeTtsBackend::OpenVoice(adapter) => {
                    adapter
                        .register_voice(name, reference_wav.clone(), transcript)
                        .await?;
                }
                NativeTtsBackend::Audio8(adapter) => {
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
        let model = self
            .models
            .iter()
            .find(|model| language_is_supported(&model.supported_languages, target_lang))
            .ok_or_else(|| InferenceError::InvalidConfiguration {
                field: "tts.target_language",
                message: format!("no active TTS model supports {target_lang:?}"),
            })?;
        match &model.backend {
            NativeTtsBackend::OpenVoice(adapter) => {
                adapter.synthesize(text, voice, target_lang).await
            }
            NativeTtsBackend::Audio8(adapter) => adapter.synthesize(text, voice).await,
        }
    }
}

fn normalized_language(language: &str) -> String {
    language.trim().replace('_', "-").to_ascii_lowercase()
}

fn language_is_supported(supported_languages: &[String], language: &str) -> bool {
    let language = normalized_language(language);
    supported_languages.is_empty()
        || supported_languages.iter().any(|supported| {
            supported == &language
                || language
                    .split_once('-')
                    .is_some_and(|(base, _)| supported == base)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_capability_matches_locales_without_provider_branches() {
        let english = vec!["en".to_owned()];
        assert!(language_is_supported(&english, "EN_us"));
        assert!(!language_is_supported(&english, "zh-CN"));
        assert!(language_is_supported(&[], "zh-CN"));
    }
}
