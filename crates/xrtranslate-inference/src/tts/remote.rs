//! Online synthesis wire formats; callers receive only mono PCM16 chunks.
mod reference;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::ops::ControlFlow;
use xrtranslate_assets::remote::{RemoteConnector, RemoteModelManifest, TtsCapabilities};
use xrtranslate_engine::language::SupportedLanguage;

use super::{SynthesizedPcm, decode_pcm16_wav};
use crate::{AsyncHttpClient, HttpRequest, InferenceError, ReqwestClient, TransportError};

#[derive(Clone)]
pub struct RemoteTtsAdapter<C = ReqwestClient> {
    http: C,
    endpoint: String,
    api_key: String,
    model: &'static RemoteModelManifest,
}

impl<C: AsyncHttpClient> RemoteTtsAdapter<C> {
    pub fn new(
        http: C,
        endpoint: String,
        api_key: String,
        model: &'static RemoteModelManifest,
    ) -> Self {
        Self {
            http,
            endpoint,
            api_key,
            model,
        }
    }

    pub fn capabilities(&self) -> &TtsCapabilities {
        self.model.tts.as_ref().expect("validated TTS model")
    }

    pub async fn synthesize<F, Fut>(
        &self,
        text: &str,
        voice: &str,
        target_language: &str,
        mut on_audio: F,
    ) -> Result<(), InferenceError>
    where
        F: FnMut(SynthesizedPcm) -> Fut + Send,
        Fut: Future<Output = Result<(), TransportError>> + Send,
    {
        if text.trim().is_empty() || text.chars().count() > self.capabilities().max_input_chars {
            return Err(InferenceError::InvalidConfiguration {
                field: "tts.input",
                message: "speech text is empty or exceeds the model limit".into(),
            });
        }
        if !self
            .capabilities()
            .voices
            .iter()
            .any(|preset| preset == voice)
            && !(self.capabilities().reference_audio
                && voice.starts_with(&format!("{}-", self.model.model)))
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "tts.voice",
                message: format!("unknown preset voice {voice:?}"),
            });
        }
        let language = SupportedLanguage::parse(target_language)
            .map(|language| language.base_code())
            .unwrap_or(target_language);
        if self
            .capabilities()
            .voice_languages
            .get(voice)
            .is_some_and(|languages| !languages.iter().any(|supported| supported == language))
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "tts.voice",
                message: format!("The selected voice does not support {language}."),
            });
        }
        let language_hint = self
            .model
            .language_aliases
            .iter()
            .find(|(code, _)| code == language)
            .map(|(_, name)| name.as_str());
        let mut request = match self.model.connector {
            RemoteConnector::OpenaiSpeech => HttpRequest::post_json(
                &self.endpoint,
                json!({
                    "model": self.model.model, "input": text, "voice": voice, "response_format": "wav"
                }),
            ),
            RemoteConnector::GeminiSpeech => HttpRequest::post_json(
                &self.endpoint,
                json!({
                    "model": self.model.model,
                    "input": [{"type": "user_input", "content": [{"type": "text", "text": text}]}],
                    "response_format": {"type": "audio", "mime_type": "audio/l16", "sample_rate": self.capabilities().sample_rate},
                    "generation_config": {"speech_config": [{"voice": voice}]},
                    "stream": true, "store": false
                }),
            ),
            RemoteConnector::MinimaxSpeech => HttpRequest::post_json(
                &self.endpoint,
                json!({
                    "model": self.model.model, "text": text, "stream": false,
                    "voice_setting": {"voice_id": voice},
                    "audio_setting": {"format": "wav", "sample_rate": self.capabilities().sample_rate, "channel": 1},
                    "output_format": "hex", "language_boost": language_hint.unwrap_or("auto"),
                    "text_normalization_mode": "quality"
                }),
            ),
            RemoteConnector::DashscopeSpeech => {
                // English instructions can pull a cloned English voice away from Chinese output.
                let instruction = if language == "zh" {
                    "请用中文普通话逐字朗读给定文本，不要翻译、改写或添加内容。".to_owned()
                } else {
                    let language_name = SupportedLanguage::parse(language)
                        .map(SupportedLanguage::name)
                        .unwrap_or(language);
                    format!(
                        "Speak in {language_name}. Read only the provided text, without translating it or adding content."
                    )
                };
                HttpRequest::post_json(
                    &self.endpoint,
                    json!({
                        "model": self.model.model,
                        "input": {"text": text, "voice": voice, "format": "pcm",
                            "sample_rate": self.capabilities().sample_rate, "language_hints": [language],
                            "instruction": instruction}
                    }),
                )
            }
            _ => {
                return Err(InferenceError::InvalidConfiguration {
                    field: "tts.connector",
                    message: "unsupported speech connector".into(),
                });
            }
        };
        let gemini = self.model.connector == RemoteConnector::GeminiSpeech;
        request.headers.push(if gemini {
            ("x-goog-api-key".into(), self.api_key.clone())
        } else {
            ("authorization".into(), format!("Bearer {}", self.api_key))
        });
        let mut wav = Vec::new();
        let mut stream = GeminiAudioStream::default();
        let response = self
            .http
            .execute_streaming(request, |chunk| {
                let decoded = if gemini {
                    stream.push(&chunk)
                } else if wav.len() + chunk.len() <= 32 * 1024 * 1024 {
                    wav.extend_from_slice(&chunk);
                    Ok(Vec::new())
                } else {
                    Err(TransportError::new("tts", "speech response exceeds 32 MiB"))
                };
                let pending = decoded.map(|chunks| {
                    chunks
                        .into_iter()
                        .map(|bytes| {
                            on_audio(SynthesizedPcm {
                                bytes,
                                sample_rate: self.capabilities().sample_rate,
                            })
                        })
                        .collect::<Vec<_>>()
                });
                async move {
                    for delivery in pending? {
                        delivery.await?;
                    }
                    Ok(ControlFlow::Continue(()))
                }
            })
            .await
            .map_err(|source| self.transport_error(source))?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: response.body,
                retry_after: response.retry_after,
            });
        }
        if gemini {
            // An HTTP EOF is not a successful synthesis completion.
            if !stream.completed || !stream.buffer.is_empty() || stream.odd_byte.is_some() {
                return Err(InferenceError::IncompleteCompletion {
                    endpoint: self.endpoint.clone(),
                    finish_reason: "audio stream ended before completion".into(),
                });
            }
            if !stream.has_audio {
                return Err(InferenceError::InvalidAudio {
                    message: "TTS returned no audio".into(),
                });
            }
        } else {
            let mut audio = match self.model.connector {
                RemoteConnector::MinimaxSpeech => {
                    decode_pcm16_wav(&self.minimax_audio(&wav, response.retry_after)?)?
                }
                // Request raw PCM: this service's WAV headers can contain placeholder lengths.
                RemoteConnector::DashscopeSpeech => SynthesizedPcm {
                    bytes: self.dashscope_audio(&wav).await?,
                    sample_rate: self.capabilities().sample_rate,
                },
                _ => decode_pcm16_wav(&wav)?,
            };
            if audio.bytes.is_empty() || audio.bytes.len() % 2 != 0 || audio.sample_rate == 0 {
                return Err(InferenceError::InvalidAudio {
                    message: "TTS returned empty or incomplete PCM16 audio".into(),
                });
            }
            if audio.sample_rate != self.capabilities().sample_rate {
                audio.bytes = audio
                    .resample_pcm16(self.capabilities().sample_rate)?
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
                    .collect();
                audio.sample_rate = self.capabilities().sample_rate;
            }
            on_audio(audio)
                .await
                .map_err(|source| self.transport_error(source))?;
        }
        Ok(())
    }

    fn response_json(&self, bytes: &[u8]) -> Result<Value, InferenceError> {
        serde_json::from_slice(bytes)
            .map_err(|_| self.invalid_response("expected a speech response object"))
    }

    fn invalid_response(&self, message: &str) -> InferenceError {
        InferenceError::InvalidResponse {
            endpoint: self.endpoint.clone(),
            message: message.into(),
            body_preview: String::new(),
        }
    }

    fn minimax_audio(
        &self,
        bytes: &[u8],
        retry_after: Option<std::time::Duration>,
    ) -> Result<Vec<u8>, InferenceError> {
        let body = self.response_json(bytes)?;
        let code = body["base_resp"]["status_code"]
            .as_u64()
            .ok_or_else(|| self.invalid_response("missing speech response status"))?;
        if code != 0 {
            // This API reports service errors inside a successful HTTP response.
            let status = match code {
                1002 | 1045 | 2045 | 2046 | 2047 | 2056 | 2062 => 429,
                1004 | 2049 => 401,
                1008 | 2053 | 2067 => 402,
                1000 | 1024 | 1033 | 2064 => 503,
                1001 | 2066 => 504,
                _ => 400,
            };
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status,
                body_preview: crate::error::preview(&body["base_resp"].to_string()),
                retry_after,
            });
        }
        if body["data"]["status"].as_u64() != Some(2) {
            return Err(self.invalid_response("speech generation did not finish"));
        }
        let hex = body["data"]["audio"]
            .as_str()
            .filter(|hex| !hex.is_empty() && hex.len() % 2 == 0)
            .ok_or_else(|| self.invalid_response("missing or incomplete speech audio"))?;
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = (pair[0] as char).to_digit(16);
                let low = (pair[1] as char).to_digit(16);
                high.zip(low)
                    .map(|(high, low)| (high * 16 + low) as u8)
                    .ok_or_else(|| self.invalid_response("invalid hexadecimal speech audio"))
            })
            .collect()
    }

    async fn dashscope_audio(&self, bytes: &[u8]) -> Result<Vec<u8>, InferenceError> {
        let body = self.response_json(bytes)?;
        if body["output"]["finish_reason"] != "stop" {
            return Err(self.invalid_response("speech generation did not finish"));
        }
        let url = body["output"]["audio"]["url"]
            .as_str()
            .and_then(|url| reqwest::Url::parse(url).ok())
            .filter(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .ok_or_else(|| self.invalid_response("missing or invalid speech audio URL"))?;
        // The signed audio URL does not receive the synthesis service's credentials.
        let request = HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Value::Null,
            multipart: None,
        };
        let mut audio = Vec::new();
        let response = self
            .http
            .execute_streaming(request, |chunk| {
                let result = if audio.len() + chunk.len() <= 32 * 1024 * 1024 {
                    audio.extend_from_slice(&chunk);
                    Ok(ControlFlow::Continue(()))
                } else {
                    Err(TransportError::new("tts", "speech response exceeds 32 MiB"))
                };
                std::future::ready(result)
            })
            .await
            .map_err(|source| self.transport_error(source))?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: crate::error::preview(&response.body),
                retry_after: response.retry_after,
            });
        }
        Ok(audio)
    }

    fn transport_error(&self, source: TransportError) -> InferenceError {
        InferenceError::Transport {
            endpoint: self.endpoint.clone(),
            source,
        }
    }
}

#[cfg(test)]
mod tests;

#[derive(Default)]
struct GeminiAudioStream {
    buffer: Vec<u8>,
    data: String,
    completed: bool,
    has_audio: bool,
    odd_byte: Option<u8>,
}

impl GeminiAudioStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, TransportError> {
        self.buffer.extend_from_slice(bytes);
        let mut audio = Vec::new();
        let mut consumed = 0;
        while let Some(end) = self.buffer[consumed..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let line = std::str::from_utf8(&self.buffer[consumed..consumed + end])
                .map_err(|error| TransportError::new("tts", error.to_string()))?
                .trim_end_matches('\r')
                .to_owned();
            consumed += end + 1;
            if line.is_empty() {
                if let Some(chunk) = self.event()? {
                    audio.push(chunk);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.trim_start());
            }
        }
        self.buffer.drain(..consumed);
        if self.buffer.len() + self.data.len() > 8 * 1024 * 1024 {
            return Err(TransportError::new(
                "tts",
                "speech stream event exceeds 8 MiB",
            ));
        }
        Ok(audio)
    }

    fn event(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        let data = std::mem::take(&mut self.data);
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }
        let event: Value = serde_json::from_str(&data)
            .map_err(|error| TransportError::new("tts", error.to_string()))?;
        if event.get("error").is_some()
            || matches!(
                event["event_type"].as_str(),
                Some("error" | "interaction.failed")
            )
        {
            return Err(TransportError::new("tts", data));
        }
        if event["event_type"] == "interaction.completed" {
            if event["interaction"]["status"]
                .as_str()
                .is_some_and(|status| status != "completed")
            {
                return Err(TransportError::new("tts", data));
            }
            self.completed = true;
        }
        if event["event_type"] != "step.delta" || event["delta"]["type"] != "audio" {
            return Ok(None);
        }
        let delta = &event["delta"];
        if delta["mime_type"]
            .as_str()
            .is_some_and(|mime| !mime.starts_with("audio/l16"))
        {
            return Err(TransportError::new("tts", "expected raw PCM16 audio"));
        }
        let encoded = delta["data"]
            .as_str()
            .ok_or_else(|| TransportError::new("tts", "missing audio data"))?;
        let mut pcm = Vec::new();
        if let Some(byte) = self.odd_byte.take() {
            pcm.push(byte);
        }
        STANDARD
            .decode_vec(encoded, &mut pcm)
            .map_err(|error| TransportError::new("tts", error.to_string()))?;
        if pcm.len() % 2 != 0 {
            self.odd_byte = pcm.pop();
        }
        self.has_audio |= !pcm.is_empty();
        Ok((!pcm.is_empty()).then_some(pcm))
    }
}
