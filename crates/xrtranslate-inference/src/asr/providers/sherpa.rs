//! Shared sherpa-onnx offline execution, configured by the model catalog.

use crate::asr::types::parse_language;
use crate::{AsrTranscript, InferenceError};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineTransducerModelConfig,
};
use std::{collections::HashMap, sync::mpsc, thread};
use xrtranslate_assets::{
    ModelAssetManifest, ModelFileRole, ModelRuntime, ResolvedModelAsset, SherpaOfflineModel,
};
use xrtranslate_engine::language::LanguageSet;

#[derive(Clone, Debug)]
pub struct SherpaOfflineAsrAdapter {
    manifest: &'static ModelAssetManifest,
    model: SherpaOfflineModel,
    leading_silence_ms: u32,
    requests: mpsc::Sender<RecognitionRequest>,
}

#[derive(Debug)]
struct RecognitionRequest {
    samples: Vec<f32>,
    language: &'static str,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

impl SherpaOfflineAsrAdapter {
    pub fn new(asset: &ResolvedModelAsset) -> Result<Self, InferenceError> {
        let manifest = asset.manifest();
        let Some(ModelRuntime::SherpaOfflineAsr {
            model,
            leading_silence_ms,
            ..
        }) = manifest.runtime
        else {
            return Err(InferenceError::InvalidConfiguration {
                field: "asr.model_asset.runtime",
                message: format!("{} is not a Sherpa offline model", manifest.label),
            });
        };
        let path = |role| {
            asset
                .file_path(role)
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| InferenceError::InvalidConfiguration {
                    field: "asr.model_asset",
                    message: format!("{} is missing {role:?}", manifest.label),
                })
        };
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.tokens = Some(path(ModelFileRole::Tokenizer)?);
        config.model_config.provider = Some("cpu".into());
        match model {
            SherpaOfflineModel::SenseVoice => {
                config.model_config.num_threads = 2;
                config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                    model: Some(path(ModelFileRole::Weights)?),
                    language: Some("auto".into()),
                    use_itn: true,
                };
            }
            SherpaOfflineModel::NemoTransducer => {
                config.model_config.num_threads =
                    thread::available_parallelism().map_or(2, |count| count.get().min(8)) as i32;
                config.model_config.model_type = Some("nemo_transducer".into());
                config.model_config.transducer = OfflineTransducerModelConfig {
                    encoder: Some(path(ModelFileRole::EncoderGraph)?),
                    decoder: Some(path(ModelFileRole::DecoderGraph)?),
                    joiner: Some(path(ModelFileRole::JoinerGraph)?),
                };
            }
        }
        let (requests, receiver) = mpsc::channel::<RecognitionRequest>();
        thread::Builder::new()
            .name(format!("asr-{}", manifest.id.as_str()))
            .spawn(move || {
                let mut recognizers: HashMap<&'static str, OfflineRecognizer> = HashMap::new();
                for request in receiver {
                    let result = (|| {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            recognizers.entry(request.language)
                        {
                            if model == SherpaOfflineModel::SenseVoice {
                                config.model_config.sense_voice.language =
                                    Some(request.language.into());
                            }
                            let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
                                format!(
                                    "cannot create {} recognizer (language={}, config={config:?}); \
                                     check the model files and sherpa-onnx diagnostics in \
                                     runtime/logs/backend_startup.log",
                                    manifest.label, request.language,
                                )
                            })?;
                            entry.insert(recognizer);
                        }
                        let recognizer = &recognizers[&request.language];
                        let stream = recognizer.create_stream();
                        stream.accept_waveform(16_000, &request.samples);
                        recognizer.decode(&stream);
                        let result = stream
                            .get_result()
                            .ok_or_else(|| format!("{} returned no result", manifest.label))?;
                        Ok(match model {
                            // NeMo's tokenizer omits this special token; sherpa exposes it.
                            SherpaOfflineModel::NemoTransducer => {
                                result.text.replace("<unk>", "").trim().to_owned()
                            }
                            SherpaOfflineModel::SenseVoice => result.text,
                        })
                    })();
                    let _ = request.reply.send(result);
                }
            })
            .map_err(|error| InferenceError::InvalidConfiguration {
                field: "asr.worker",
                message: error.to_string(),
            })?;
        Ok(Self {
            manifest,
            model,
            leading_silence_ms,
            requests,
        })
    }

    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        language: Option<&str>,
    ) -> Result<AsrTranscript, InferenceError> {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err(InferenceError::InvalidAudio {
                message: "Sherpa offline ASR requires nonempty 16-bit mono PCM".into(),
            });
        }
        let language = parse_language(language)?;
        if let Some(language) = language
            && !LanguageSet::from_codes(self.manifest.languages, true).contains(language)
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "asr.language",
                message: format!(
                    "{} does not support {}; supported codes: {}",
                    self.manifest.label,
                    language.code(),
                    self.manifest.languages.join(", ")
                ),
            });
        }
        let language = language.map(|language| language.base_code()).or_else(|| {
            match self.manifest.languages {
                [language] => Some(*language),
                _ => None,
            }
        });
        // Only SenseVoice consumes a language hint. NeMo shares one recognizer
        // for automatic and explicit routes through the same model.
        let recognizer_language = match self.model {
            SherpaOfflineModel::SenseVoice => language.unwrap_or("auto"),
            SherpaOfflineModel::NemoTransducer => "auto",
        };
        let mut samples = vec![0.0; self.leading_silence_ms as usize * 16];
        samples.extend(
            pcm.chunks_exact(2)
                .map(|bytes| f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0),
        );
        let (reply, result) = tokio::sync::oneshot::channel();
        self.requests
            .send(RecognitionRequest {
                samples,
                language: recognizer_language,
                reply,
            })
            .map_err(|error| self.error(error))?;
        let text = result
            .await
            .map_err(|error| self.error(error))?
            .map_err(|error| self.error(error))?;
        if text.trim().is_empty() {
            return Err(InferenceError::EmptyOutput {
                operation: "Sherpa offline ASR",
            });
        }
        Ok(AsrTranscript {
            language: language.map(str::to_owned),
            text,
        })
    }

    fn error(&self, error: impl std::fmt::Display) -> InferenceError {
        InferenceError::InvalidResponse {
            endpoint: self.manifest.id.as_str().into(),
            message: error.to_string(),
            body_preview: String::new(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xrtranslate_assets::{ModelAssetId, ModelAssetsConfig};

    /// Set SENSEVOICE_TEST_DIR to a models root containing SenseVoiceSmall-INT8-ONNX and test WAVs.
    #[tokio::test]
    #[ignore = "requires downloaded SenseVoiceSmall model and en/zh/ja audio fixtures"]
    async fn sensevoice_multilingual_fixture_smoke() {
        use xrtranslate_engine::language::SupportedLanguage;

        let root =
            PathBuf::from(std::env::var_os("SENSEVOICE_TEST_DIR").expect("SENSEVOICE_TEST_DIR"));
        for code in ["en", "zh", "ja"] {
            let wave = sherpa_onnx::Wave::read(root.join(format!("{code}.wav")).to_str().unwrap())
                .unwrap();
            assert_eq!(wave.sample_rate(), 16_000);
            let pcm = wave
                .samples()
                .iter()
                .flat_map(|sample| ((*sample * 32767.0) as i16).to_le_bytes())
                .collect::<Vec<_>>();
            let assets =
                ModelAssetsConfig::with_directory_overrides(Some(root.clone()), None, None)
                    .resolve_selected(".");
            let adapter =
                SherpaOfflineAsrAdapter::new(assets.asset(ModelAssetId::SenseVoiceSmallInt8Onnx))
                    .unwrap();
            let language = SupportedLanguage::from_code(code).unwrap();
            let result = adapter
                .transcribe_pcm16(&pcm, Some(language.name()))
                .await
                .unwrap();
            assert!(!result.text.trim().is_empty());
            assert_eq!(result.language.as_deref(), Some(code));
            eprintln!("SenseVoice {code}: {}", result.text);

            let by_code = adapter.transcribe_pcm16(&pcm, Some(code)).await.unwrap();
            assert_eq!(by_code.text, result.text);
        }
    }
}
