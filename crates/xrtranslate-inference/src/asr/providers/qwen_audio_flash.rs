use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::qwen_audio::validated_vocabulary;
use crate::{
    AsrTranscript, AsrVocabularyBias, AsyncHttpClient, HttpRequest, InferenceError, error::preview,
    pcm16_mono_16khz_to_wav,
};

const MAX_CONTEXT_CHARS: usize = 400;

/// Lexical recognition hints supported by Qwen Audio's native HTTP API.
///
/// Context and weighted vocabulary are separate inputs. This API does not
/// accept an instruction prompt or chat-generation sampling parameters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QwenAudioFlashOptions {
    pub language: Option<String>,
    pub context_bias: Option<String>,
    pub vocabulary_bias: Vec<AsrVocabularyBias>,
}

/// Recognizes a complete utterance through DashScope's native multimodal API.
///
/// Both Qwen Audio 3.0 and 3.1 Flash accept this contract. The endpoint is the
/// complete `/api/v1/services/aigc/multimodal-generation/generation` URL, or a
/// gateway exposing the same request and response format.
#[derive(Clone)]
pub struct QwenAudioFlashAdapter<C> {
    http: C,
    endpoint: String,
    model: String,
    authorization: String,
}

impl<C> std::fmt::Debug for QwenAudioFlashAdapter<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QwenAudioFlashAdapter")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl<C> QwenAudioFlashAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let endpoint = required_value("endpoint", endpoint.into())?;
        let url = reqwest::Url::parse(&endpoint).map_err(|error| {
            InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: error.to_string(),
            }
        })?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(InferenceError::InvalidConfiguration {
                field: "endpoint",
                message: "must be a complete http:// or https:// URL".into(),
            });
        }
        let api_key = required_value("api_key", api_key.into())?;
        let authorization = format!("Bearer {api_key}");
        reqwest::header::HeaderValue::from_str(&authorization).map_err(|_| {
            InferenceError::InvalidConfiguration {
                field: "api_key",
                message: "must be a valid bearer token".into(),
            }
        })?;
        Ok(Self {
            http,
            endpoint,
            model: required_value("model", model.into())?,
            authorization,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl<C: AsyncHttpClient> QwenAudioFlashAdapter<C> {
    /// Converts one VAD-delimited PCM16, 16 kHz, mono utterance into a WAV
    /// Data URL and submits it in one request, without replaying the audio.
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: QwenAudioFlashOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        let (language_hint, language) = map_language(options.language.as_deref())?;
        let vocabulary = validated_vocabulary(&options.vocabulary_bias)?;
        let context = options
            .context_bias
            .as_deref()
            .map(str::trim)
            .filter(|context| !context.is_empty());
        if context.is_some_and(|context| context.chars().count() > MAX_CONTEXT_CHARS) {
            return Err(InferenceError::InvalidConfiguration {
                field: "context_bias",
                message: format!("must contain at most {MAX_CONTEXT_CHARS} characters"),
            });
        }

        let wav = pcm16_mono_16khz_to_wav(pcm)?;
        let data = format!("data:audio/wav;base64,{}", STANDARD.encode(wav));
        let mut messages = Vec::new();
        if let Some(context) = context {
            messages.push(json!({
                "role": "user",
                "content": [{"type": "input_text", "text": context}]
            }));
        }
        messages.push(json!({
            "role": "user",
            "content": [{"type": "input_audio", "input_audio": {"data": data}}]
        }));
        let mut parameters = json!({"format": "wav", "sample_rate": "16000"});
        if let Some(hint) = language_hint {
            parameters["language_hints"] = json!([hint]);
        }
        if !vocabulary.is_empty() {
            parameters["vocabulary"] = json!(vocabulary);
        }
        let mut request = HttpRequest::post_json(
            &self.endpoint,
            json!({
                "model": self.model,
                "input": {"messages": messages},
                "parameters": parameters,
            }),
        );
        request
            .headers
            .push(("authorization".into(), self.authorization.clone()));
        let response =
            self.http
                .execute(request)
                .await
                .map_err(|source| InferenceError::Transport {
                    endpoint: self.endpoint.clone(),
                    source,
                })?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: preview(&response.body),
            });
        }
        let value: Value = serde_json::from_str(&response.body)
            .map_err(|error| self.invalid_response(error.to_string(), &response.body))?;
        // `output.text` contains the full cumulative transcript. The separate
        // `sentence.text` field can contain only the last sentence.
        let text = value["output"]["text"]
            .as_str()
            .ok_or_else(|| {
                self.invalid_response("missing string output.text".into(), &response.body)
            })?
            .trim()
            .to_owned();
        // A successful empty transcript is a valid no-speech result. The
        // shared pipeline suppresses it without surfacing a provider failure.
        Ok(AsrTranscript { language, text })
    }

    fn invalid_response(&self, message: String, body: &str) -> InferenceError {
        InferenceError::InvalidResponse {
            endpoint: self.endpoint.clone(),
            message,
            body_preview: preview(body),
        }
    }
}

fn required_value(field: &'static str, value: String) -> Result<String, InferenceError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(InferenceError::InvalidConfiguration {
            field,
            message: "must not be empty".into(),
        });
    }
    Ok(value.to_owned())
}

fn map_language(
    language: Option<&str>,
) -> Result<(Option<&'static str>, Option<String>), InferenceError> {
    let Some(language) = crate::asr::types::parse_language(language)? else {
        return Ok((None, None));
    };
    let code = match language.base_code() {
        "tl" => "fil",
        code => code,
    };
    if !xrtranslate_assets::remote::QWEN_AUDIO_LANGUAGES.contains(&code) {
        return Err(InferenceError::InvalidConfiguration {
            field: "asr.language",
            message: format!(
                "{} is not supported by Qwen Audio Flash ASR",
                language.code()
            ),
        });
    }
    Ok((Some(code), Some(language.code().to_owned())))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{HttpResponse, TransportError};

    const ENDPOINT: &str =
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
    const MODEL: &str = "qwen-audio-3.0-asr-flash";

    #[derive(Default)]
    struct RecordingHttpClient {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<Vec<Result<HttpResponse, TransportError>>>,
    }

    impl RecordingHttpClient {
        fn response(status: u16, body: &str) -> Self {
            Self {
                responses: Mutex::new(vec![Ok(HttpResponse {
                    status,
                    body: body.into(),
                })]),
                ..Default::default()
            }
        }
    }

    impl AsyncHttpClient for RecordingHttpClient {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.responses.lock().unwrap().remove(0)
        }
    }

    #[tokio::test]
    async fn native_http_sends_wav_context_and_weighted_vocabulary() {
        let http = RecordingHttpClient::response(
            200,
            r#"{"output":{"text":" Go to spawn. Stay there. ","sentence":{"text":"Stay there."}}}"#,
        );
        let adapter = QwenAudioFlashAdapter::new(http, ENDPOINT, MODEL, "test-key").unwrap();
        let transcript = adapter
            .transcribe_pcm16(
                &[1, 0, 2, 0],
                QwenAudioFlashOptions {
                    language: Some("English".into()),
                    context_bias: Some(" VRChat, spawn ".into()),
                    vocabulary_bias: vec![
                        AsrVocabularyBias {
                            text: " VRChat ".into(),
                            weight: 4,
                        },
                        AsrVocabularyBias {
                            text: "Bhop".into(),
                            weight: 50,
                        },
                    ],
                },
            )
            .await
            .unwrap();
        assert_eq!(transcript.text, "Go to spawn. Stay there.");
        assert_eq!(transcript.language.as_deref(), Some("en"));
        assert!(!format!("{adapter:?}").contains("test-key"));
        let request = adapter.http.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.url, ENDPOINT);
        assert_eq!(request.method, "POST");
        assert!(request.multipart.is_none());
        assert!(
            request
                .headers
                .contains(&("authorization".into(), "Bearer test-key".into()))
        );
        assert!(
            request
                .headers
                .contains(&("content-type".into(), "application/json".into()))
        );
        let audio = &request.body["input"]["messages"][1]["content"][0]["input_audio"]["data"];
        let wav = STANDARD
            .decode(
                audio
                    .as_str()
                    .unwrap()
                    .strip_prefix("data:audio/wav;base64,")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
        // Exact equality guards against accidentally using Chat Completions,
        // dropping weights, or sending unsupported generation parameters.
        assert_eq!(
            request.body,
            json!({
                "model": MODEL,
                "input": {"messages": [
                    {"role": "user", "content": [{"type": "input_text", "text": "VRChat, spawn"}]},
                    {"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": audio}}]}
                ]},
                "parameters": {"format": "wav", "sample_rate": "16000", "language_hints": ["en"], "vocabulary": {"VRChat": 4, "Bhop": 50}}
            })
        );
    }

    #[tokio::test]
    async fn automatic_recognition_omits_empty_hints() {
        let http = RecordingHttpClient::response(200, r#"{"output":{"text":"Hello"}}"#);
        let adapter =
            QwenAudioFlashAdapter::new(http, ENDPOINT, "qwen-audio-3.1-asr-flash", "key").unwrap();
        let transcript = adapter
            .transcribe_pcm16(
                &[0, 0],
                QwenAudioFlashOptions {
                    language: Some("auto".into()),
                    context_bias: Some("   ".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(transcript.language, None);
        let request = adapter.http.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.body["model"], "qwen-audio-3.1-asr-flash");
        assert_eq!(
            request.body["input"]["messages"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            request.body["parameters"],
            json!({"format": "wav", "sample_rate": "16000"})
        );
    }

    #[test]
    fn language_hints_use_native_codes_and_reject_removed_languages() {
        for (input, expected) in [
            ("tl", "fil"),
            ("fil", "fil"),
            ("zh-Hant", "zh"),
            ("English", "en"),
        ] {
            assert_eq!(map_language(Some(input)).unwrap().0, Some(expected));
        }
        for input in ["tr", "uk", "is"] {
            assert!(matches!(
                map_language(Some(input)),
                Err(InferenceError::InvalidConfiguration {
                    field: "asr.language",
                    ..
                })
            ));
        }
    }

    #[tokio::test]
    async fn invalid_input_is_rejected_before_network_io() {
        let adapter =
            QwenAudioFlashAdapter::new(RecordingHttpClient::default(), ENDPOINT, MODEL, "key")
                .unwrap();
        assert!(matches!(
            adapter
                .transcribe_pcm16(&[0], QwenAudioFlashOptions::default())
                .await,
            Err(InferenceError::InvalidAudio { .. })
        ));
        assert!(matches!(
            adapter
                .transcribe_pcm16(
                    &[0, 0],
                    QwenAudioFlashOptions {
                        context_bias: Some("界".repeat(401)),
                        ..Default::default()
                    }
                )
                .await,
            Err(InferenceError::InvalidConfiguration {
                field: "context_bias",
                ..
            })
        ));
        assert!(matches!(
            adapter
                .transcribe_pcm16(
                    &[0, 0],
                    QwenAudioFlashOptions {
                        vocabulary_bias: vec![AsrVocabularyBias {
                            text: "spawn".into(),
                            weight: 6
                        }],
                        ..Default::default()
                    }
                )
                .await,
            Err(InferenceError::InvalidConfiguration {
                field: "vocabulary_bias.weight",
                ..
            })
        ));
        assert!(adapter.http.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn remote_failures_preserve_status_and_transport_category() {
        let adapter = QwenAudioFlashAdapter::new(
            RecordingHttpClient::response(
                401,
                r#"{"code":"InvalidApiKey","message":"Invalid API-key provided."}"#,
            ),
            ENDPOINT,
            MODEL,
            "key",
        )
        .unwrap();
        let error = adapter
            .transcribe_pcm16(&[0, 0], QwenAudioFlashOptions::default())
            .await
            .unwrap_err();
        assert!(error.requires_provider_configuration());
        assert!(
            matches!(error, InferenceError::HttpStatus { status: 401, ref body_preview, .. } if body_preview.contains("InvalidApiKey"))
        );
        let http = RecordingHttpClient {
            responses: Mutex::new(vec![Err(TransportError::new(
                "timeout",
                "request timed out",
            ))]),
            ..Default::default()
        };
        let adapter = QwenAudioFlashAdapter::new(http, ENDPOINT, MODEL, "key").unwrap();
        assert!(
            matches!(adapter.transcribe_pcm16(&[0, 0], QwenAudioFlashOptions::default()).await, Err(InferenceError::Transport { source: TransportError { ref kind, .. }, .. }) if kind == "timeout")
        );
    }

    #[tokio::test]
    async fn response_requires_full_transcript_and_preserves_no_speech_result() {
        for body in [
            "not JSON",
            r#"{"output":{"sentence":{"text":"Only last sentence"}}}"#,
            r#"{"choices":[{"message":{"content":"Legacy wire format"}}]}"#,
            r#"{"output":{"text":17}}"#,
        ] {
            let adapter = QwenAudioFlashAdapter::new(
                RecordingHttpClient::response(200, body),
                ENDPOINT,
                MODEL,
                "key",
            )
            .unwrap();
            assert!(matches!(
                adapter
                    .transcribe_pcm16(&[0, 0], QwenAudioFlashOptions::default())
                    .await,
                Err(InferenceError::InvalidResponse { .. })
            ));
        }
        let adapter = QwenAudioFlashAdapter::new(
            RecordingHttpClient::response(200, r#"{"output":{"text":"   "}}"#),
            ENDPOINT,
            MODEL,
            "key",
        )
        .unwrap();
        let transcript = adapter
            .transcribe_pcm16(&[0, 0], QwenAudioFlashOptions::default())
            .await
            .unwrap();
        assert!(transcript.text.is_empty());
        assert_eq!(transcript.language, None);
    }

    #[test]
    fn configuration_requires_http_endpoint_model_and_valid_bearer_token() {
        for (endpoint, model, key, field) in [
            (
                "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
                MODEL,
                "key",
                "endpoint",
            ),
            (ENDPOINT, " ", "key", "model"),
            (ENDPOINT, MODEL, " ", "api_key"),
            (ENDPOINT, MODEL, "secret\nheader: injected", "api_key"),
        ] {
            assert!(
                matches!(QwenAudioFlashAdapter::new(RecordingHttpClient::default(), endpoint, model, key), Err(InferenceError::InvalidConfiguration { field: actual, .. }) if actual == field)
            );
        }
    }
}
