//! SenseVoice ONNX execution and its sherpa-onnx wire contract.

use crate::asr::types::parse_language;
use crate::{AsrTranscript, InferenceError};
use std::{collections::HashMap, path::PathBuf, sync::mpsc, thread};

#[derive(Clone, Debug)]
pub struct SenseVoiceAdapter {
    requests: mpsc::Sender<SenseVoiceRequest>,
}

#[derive(Debug)]
struct SenseVoiceRequest {
    samples: Vec<f32>,
    language: &'static str,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

// sherpa-onnx accepts only these base codes, even for Traditional Chinese.
fn sensevoice_language(language: Option<&str>) -> Result<&'static str, InferenceError> {
    let Some(language) = parse_language(language)? else {
        return Ok("auto");
    };
    match language.base_code() {
        code @ ("zh" | "en" | "ja" | "ko" | "yue") => Ok(code),
        _ => Err(InferenceError::InvalidConfiguration {
            field: "asr.language",
            message: format!(
                "SenseVoice does not support {}; supported codes: zh, en, ja, ko, yue",
                language.code()
            ),
        }),
    }
}

impl SenseVoiceAdapter {
    pub fn new(model: PathBuf, tokens: PathBuf) -> Self {
        let (requests, receiver) = mpsc::channel::<SenseVoiceRequest>();
        thread::Builder::new()
            .name("sensevoice-asr".into())
            .spawn(move || {
                use sherpa_onnx::{
                    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
                };
                let mut recognizers: HashMap<&'static str, OfflineRecognizer> = HashMap::new();
                for request in receiver {
                    let result = (|| {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            recognizers.entry(request.language)
                        {
                            let mut config = OfflineRecognizerConfig::default();
                            config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                                model: Some(model.to_string_lossy().into_owned()),
                                language: Some(request.language.to_owned()),
                                use_itn: true,
                            };
                            config.model_config.tokens =
                                Some(tokens.to_string_lossy().into_owned());
                            config.model_config.provider = Some("cpu".into());
                            config.model_config.num_threads = 2;
                            let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
                                format!(
                                    "cannot create SenseVoice recognizer (language={}, provider=cpu, \
                                     model={}, tokens={}); check the model files and the sherpa-onnx \
                                     diagnostic in runtime/logs/backend_startup.log",
                                    request.language,
                                    model.display(),
                                    tokens.display(),
                                )
                            })?;
                            entry.insert(recognizer);
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

    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        language: Option<&str>,
    ) -> Result<AsrTranscript, InferenceError> {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err(InferenceError::InvalidAudio {
                message: "SenseVoice requires nonempty 16-bit mono PCM".into(),
            });
        }
        let normalized_language = sensevoice_language(language)?;
        let samples = pcm
            .chunks_exact(2)
            .map(|bytes| f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0)
            .collect();
        let (reply, result) = tokio::sync::oneshot::channel();
        self.requests
            .send(SenseVoiceRequest {
                samples,
                language: normalized_language,
                reply,
            })
            .map_err(sensevoice_error)?;
        let text = result
            .await
            .map_err(sensevoice_error)?
            .map_err(sensevoice_error)?;
        if text.trim().is_empty() {
            return Err(InferenceError::EmptyOutput {
                operation: "SenseVoice ASR",
            });
        }
        Ok(AsrTranscript {
            language: (normalized_language != "auto").then(|| normalized_language.to_owned()),
            text,
        })
    }
}

fn sensevoice_error(error: impl std::fmt::Display) -> InferenceError {
    InferenceError::InvalidResponse {
        endpoint: "sensevoice-onnx".into(),
        message: error.to_string(),
        body_preview: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensevoice_accepts_pipeline_language_names_and_regional_codes() {
        use xrtranslate_engine::language::SupportedLanguage;

        // Accept canonical pipeline codes and legacy callers through the shared parser.
        for code in ["zh", "zh-TW", "en", "ja", "ko", "yue"] {
            let language = SupportedLanguage::from_code(code).unwrap();
            let expected = if code == "zh-TW" { "zh" } else { code };
            assert_eq!(sensevoice_language(Some(language.name())), Ok(expected));
            assert_eq!(sensevoice_language(Some(code)), Ok(expected));
        }
        assert_eq!(sensevoice_language(Some("zh_Hant_TW")), Ok("zh"));
        assert_eq!(sensevoice_language(None), Ok("auto"));
    }

    #[tokio::test]
    async fn sensevoice_rejects_unsupported_languages_before_native_initialization() {
        let (requests, receiver) = mpsc::channel();
        let adapter = SenseVoiceAdapter { requests };
        for language in ["Russian", "fr-FR", "unknown", "en,ja"] {
            let error = adapter
                .transcribe_pcm16(&[0, 0], Some(language))
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                InferenceError::InvalidConfiguration {
                    field: "asr.language",
                    ..
                }
            ));
            assert!(!error.to_string().is_empty());
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn sensevoice_initialization_error_includes_language_and_model_paths() {
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-missing-sensevoice-{}",
            std::process::id()
        ));
        assert!(!root.exists());
        let model = root.join("model.int8.onnx");
        let tokens = root.join("tokens.txt");
        let adapter = SenseVoiceAdapter::new(model.clone(), tokens.clone());
        let error = adapter
            .transcribe_pcm16(&[0, 0], Some("English"))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("language=en"), "{message}");
        assert!(message.contains(&model.display().to_string()), "{message}");
        assert!(message.contains(&tokens.display().to_string()), "{message}");
    }

    /// Set SENSEVOICE_TEST_DIR to the verified model and upstream test_wavs.
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
            let adapter =
                SenseVoiceAdapter::new(root.join("model.int8.onnx"), root.join("tokens.txt"));
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
