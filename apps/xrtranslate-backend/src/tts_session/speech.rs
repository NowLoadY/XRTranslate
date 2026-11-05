//! Stateless synthesis for consumers that store audio instead of playing it live.
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use std::io::Cursor;
use xrtranslate_protocol::tts::SpeechRequest;

pub(crate) async fn synthesize(
    State(state): State<crate::BackendState>,
    Json(request): Json<SpeechRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let started = std::time::Instant::now();
    let output = synthesize_wav(&state, &request).await;
    match &output {
        Ok(wav) => tracing::info!(language = %request.language, audio_bytes = wav.len(),
            elapsed_ms = crate::millis(started.elapsed()), "Speech file synthesized"),
        Err(error) => tracing::warn!(language = %request.language, %error,
            elapsed_ms = crate::millis(started.elapsed()), "Speech file synthesis failed"),
    }
    output
        .map(|wav| ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav))
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn synthesize_wav(
    state: &crate::BackendState,
    request: &SpeechRequest,
) -> Result<Vec<u8>, String> {
    if request.text.trim().is_empty() || request.language.trim().is_empty() {
        return Err("Speech text and target language are required".into());
    }
    let speech = state.prepare_speech().await?;
    let adapter = speech.tts.as_ref().ok_or("Configure a TTS model first.")?;
    if !adapter.supports_language(&request.language) {
        return Err(format!(
            "The TTS model does not support {}",
            request.language
        ));
    }
    let voice = request
        .voice
        .clone()
        .unwrap_or_else(|| speech.voices.status().selection);
    speech.voices.register_selection(&voice).await?;
    let name = super::voices::voice_name(&voice);
    let mut pcm = Vec::new();
    let mut sample_rate = None;
    for text in super::split_text(&request.text, super::max_input_chars(&state.config)) {
        let audio = adapter
            .synthesize(&text, &name, &request.language)
            .await
            .map_err(|e| e.to_string())?;
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
