//! Online synthesis wire formats; callers receive only mono PCM16 chunks.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use xrtranslate_assets::remote::{RemoteConnector, RemoteModelManifest, TtsCapabilities};

use super::{SynthesizedPcm, decode_pcm16_wav};
use crate::{AsyncHttpClient, HttpRequest, InferenceError, ReqwestClient, TransportError};

#[derive(Clone)]
pub struct RemoteTtsAdapter {
    http: ReqwestClient,
    endpoint: String,
    api_key: String,
    model: &'static RemoteModelManifest,
}

impl RemoteTtsAdapter {
    pub fn new(
        http: ReqwestClient,
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
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "tts.voice",
                message: format!("unknown preset voice {voice:?}"),
            });
        }
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
                    Ok(())
                }
            })
            .await
            .map_err(|source| self.transport_error(source))?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: response.status,
                body_preview: response.body,
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
            let mut audio = decode_pcm16_wav(&wav)?;
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

    fn transport_error(&self, source: TransportError) -> InferenceError {
        InferenceError::Transport {
            endpoint: self.endpoint.clone(),
            source,
        }
    }
}

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
