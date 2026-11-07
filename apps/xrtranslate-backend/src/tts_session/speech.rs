//! Stateless synthesis for consumers that store audio instead of playing it live.
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{io::Cursor, time::Duration};
use xrtranslate_inference::InferenceError;
use xrtranslate_protocol::tts::SpeechRequest;

#[derive(Debug)]
pub(crate) struct SpeechError {
    status: StatusCode,
    message: String,
    retry_after: Option<Duration>,
}

impl From<String> for SpeechError {
    fn from(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            retry_after: None,
        }
    }
}

impl From<&str> for SpeechError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<InferenceError> for SpeechError {
    fn from(error: InferenceError) -> Self {
        let (status, retry_after) = match &error {
            InferenceError::HttpStatus {
                status,
                retry_after,
                ..
            } => (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
                *retry_after,
            ),
            _ => (StatusCode::BAD_GATEWAY, None),
        };
        Self {
            status,
            message: error.to_string(),
            retry_after,
        }
    }
}

impl IntoResponse for SpeechError {
    fn into_response(self) -> axum::response::Response {
        let mut headers = HeaderMap::new();
        if let Some(delay) = self.retry_after {
            let seconds = delay
                .as_secs()
                .saturating_add(u64::from(delay.subsec_nanos() > 0));
            headers.insert(header::RETRY_AFTER, seconds.to_string().parse().unwrap());
        }
        (self.status, headers, self.message).into_response()
    }
}

pub(crate) async fn synthesize(
    State(state): State<crate::BackendState>,
    Json(request): Json<SpeechRequest>,
) -> Result<impl IntoResponse, SpeechError> {
    let started = std::time::Instant::now();
    // Finish cleanup of a temporary cloud voice even if the caller disconnects.
    let output = if request.reference_audio.is_some() || request.reference_context.is_some() {
        let (state, request) = (state.clone(), request.clone());
        tokio::spawn(async move { synthesize_wav(&state, &request).await })
            .await
            .map_err(|e| SpeechError::from(e.to_string()))?
    } else {
        synthesize_wav(&state, &request).await
    };
    match &output {
        Ok(wav) => tracing::info!(language = %request.language, audio_bytes = wav.len(),
            elapsed_ms = crate::millis(started.elapsed()), "Speech file synthesized"),
        Err(error) => tracing::warn!(language = %request.language, error = %error.message,
            elapsed_ms = crate::millis(started.elapsed()), "Speech file synthesis failed"),
    }
    output.map(|wav| ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav))
}

async fn synthesize_wav(
    state: &crate::BackendState,
    request: &SpeechRequest,
) -> Result<Vec<u8>, SpeechError> {
    if request.text.trim().is_empty() || request.language.trim().is_empty() {
        return Err("Speech text and target language are required".into());
    }
    let speech = state.prepare_speech().await?;
    let adapter = speech.tts.as_ref().ok_or("Configure a TTS model first.")?;
    if !adapter.supports_language(&request.language) {
        return Err(format!("The TTS model does not support {}", request.language).into());
    }
    // Keep job cleanup behind any in-flight enrollment or synthesis.
    let mut references = if request.reference_context.is_some() {
        Some(speech.reference_voices.lock().await)
    } else {
        None
    };
    let name = if let Some(context) = &request.reference_context {
        references
            .as_mut()
            .unwrap()
            .resolve(
                context,
                request.reference_audio.as_deref(),
                &request.language,
                adapter,
            )
            .await?
    } else if let Some(reference) = &request.reference_audio {
        let wav = decode_reference(reference)?;
        adapter
            .create_reference_voice(&wav, &request.language)
            .await?
    } else {
        let voice = request
            .voice
            .clone()
            .unwrap_or_else(|| speech.voices.status().selection);
        speech.voices.register_selection(&voice).await?
    };
    let output = synthesize_voice_wav(
        adapter,
        &name,
        request,
        super::max_input_chars(&state.config),
    )
    .await;
    if request.reference_audio.is_some() && request.reference_context.is_none() {
        adapter
            .delete_reference_voice(&name, &request.language)
            .await;
    }
    output
}

pub(super) fn decode_reference(reference: &str) -> Result<Vec<u8>, SpeechError> {
    if reference.len() > 14 * 1024 * 1024 {
        return Err("Voice reference is too large".into());
    }
    STANDARD
        .decode(reference)
        .map_err(|_| "Invalid voice reference".into())
}

async fn synthesize_voice_wav(
    adapter: &crate::model_runtime::TtsAdapter,
    name: &str,
    request: &SpeechRequest,
    max_chars: usize,
) -> Result<Vec<u8>, SpeechError> {
    let mut pcm = Vec::new();
    let mut sample_rate = None;
    for text in super::split_text(&request.text, max_chars) {
        let audio = adapter.synthesize(&text, name, &request.language).await?;
        if audio.sample_rate == 0 || audio.bytes.is_empty() || !audio.bytes.len().is_multiple_of(2)
        {
            return Err("The TTS model returned invalid or empty audio".into());
        }
        if sample_rate
            .replace(audio.sample_rate)
            .is_some_and(|rate| rate != audio.sample_rate)
        {
            return Err("The TTS model changed sample rate during synthesis".into());
        }
        pcm.extend(audio.bytes);
    }
    let mut buffer = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(
        &mut buffer,
        hound::WavSpec {
            channels: 1,
            sample_rate: sample_rate.ok_or("The TTS model returned no audio")?,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .map_err(|e| e.to_string())?;
    for sample in pcm.chunks_exact(2) {
        writer
            .write_sample(i16::from_le_bytes([sample[0], sample[1]]))
            .map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_provider_status_and_cooldown() {
        for status in [401, 429, 503] {
            let response = SpeechError::from(InferenceError::HttpStatus {
                endpoint: "http://fixture".into(),
                status,
                body_preview: "provider error".into(),
                retry_after: (status == 429).then_some(Duration::from_millis(2500)),
            })
            .into_response();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .map(|v| v.to_str().unwrap()),
                (status == 429).then_some("3")
            );
        }
        assert_eq!(
            SpeechError::from("invalid input").into_response().status(),
            StatusCode::BAD_REQUEST
        );
    }
}
