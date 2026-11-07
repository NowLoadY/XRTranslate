//! Short-lived provider voices for a single reference-based synthesis.
use super::*;
use std::time::Duration;

impl<C: AsyncHttpClient> RemoteTtsAdapter<C> {
    pub async fn create_reference_voice(&self, wav: &[u8]) -> Result<String, InferenceError> {
        if !self.capabilities().reference_audio
            || self.model.connector != RemoteConnector::DashscopeSpeech
        {
            return Err(InferenceError::InvalidConfiguration {
                field: "tts.reference",
                message: "The selected TTS model does not support reference audio.".into(),
            });
        }
        if wav.len() > 10 * 1024 * 1024 {
            return Err(self.invalid_response("Voice reference exceeds 10 MiB"));
        }
        let audio = decode_pcm16_wav(wav)?;
        let seconds = audio.bytes.len() as f64 / (2.0 * audio.sample_rate as f64);
        if audio.sample_rate < 16_000 || !(5.0..=60.0).contains(&seconds) {
            return Err(InferenceError::InvalidAudio {
                message: "Use 5–60 seconds of clear original speech for voice cloning.".into(),
            });
        }
        let created = self.voice_request(json!({
            "action": "create_voice", "target_model": self.model.model,
            "prefix": "xrtmedia", "audio": format!("data:audio/wav;base64,{}", STANDARD.encode(wav))
        })).await?;
        let voice = created["output"]["voice_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| self.invalid_response("Missing cloned voice ID"))?
            .to_owned();
        let ready = async {
            for attempt in 0..60 {
                let status = self.voice_request(json!({"action": "query_voice", "voice_id": voice})).await?;
                match status["output"]["status"].as_str() {
                    Some("OK") => return Ok(()),
                    Some("DEPLOYING") if attempt < 59 => tokio::time::sleep(Duration::from_secs(1)).await,
                    _ => return Err(self.invalid_response("The voice reference could not be prepared. Use clear speech from one speaker.")),
                }
            }
            unreachable!()
        }.await;
        if let Err(error) = ready {
            self.delete_reference_voice(&voice).await;
            return Err(error);
        }
        Ok(voice)
    }

    pub async fn delete_reference_voice(&self, voice: &str) {
        if let Err(error) = self
            .voice_request(json!({"action": "delete_voice", "voice_id": voice}))
            .await
        {
            tracing::warn!(%error, "Could not remove temporary TTS voice");
        }
    }

    async fn voice_request(&self, input: Value) -> Result<Value, InferenceError> {
        let mut endpoint = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| self.invalid_response("Invalid speech endpoint"))?;
        endpoint.set_path("/api/v1/services/audio/tts/customization");
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        let mut request = HttpRequest::post_json(
            endpoint.as_str(),
            json!({"model": "voice-enrollment", "input": input}),
        );
        request
            .headers
            .push(("authorization".into(), format!("Bearer {}", self.api_key)));
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|source| self.transport_error(source))?;
        if !(200..300).contains(&response.status) {
            return Err(InferenceError::HttpStatus {
                endpoint: endpoint.into(),
                status: response.status,
                body_preview: response.body,
                retry_after: response.retry_after,
            });
        }
        self.response_json(response.body.as_bytes())
    }
}
