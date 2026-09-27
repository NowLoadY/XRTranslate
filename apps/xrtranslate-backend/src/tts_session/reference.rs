//! Reference transcription uses the configured ASR adapter and shared scheduling.
use axum::{
    Json,
    body::Bytes,
    extract::{Query, State},
    http::StatusCode,
};
use xrtranslate_assets::voices::{MAX_REFERENCE_SECONDS, SAMPLE_RATE};
use xrtranslate_engine::language::{LanguageSet, SupportedLanguage};
use xrtranslate_protocol::{
    InferenceWorkload,
    tts::{ReferenceLanguage, ReferenceTranscript},
};

use crate::{BackendState, model_runtime::NativeAsrOptions};

pub(crate) async fn transcribe_reference(
    State(state): State<BackendState>,
    Query(reference): Query<ReferenceLanguage>,
    pcm: Bytes,
) -> Result<Json<ReferenceTranscript>, (StatusCode, String)> {
    let invalid = |message: &str| (StatusCode::BAD_REQUEST, message.to_owned());
    if !pcm.len().is_multiple_of(2)
        || pcm.len() < SAMPLE_RATE as usize
        || pcm.len() > MAX_REFERENCE_SECONDS * SAMPLE_RATE as usize * 2
    {
        return Err(invalid(
            "Reference audio must be between 0.5 and 60 seconds.",
        ));
    }
    let available = state
        .model_plan
        .language_capabilities
        .recognition
        .unwrap_or(LanguageSet::ALL);
    let language = if reference.language == "auto" {
        None
    } else {
        Some(
            SupportedLanguage::parse(&reference.language)
                .filter(|language| available.contains(*language))
                .ok_or_else(|| invalid("The ASR model does not support this language."))?
                .code()
                .to_owned(),
        )
    };
    if available.is_empty() {
        return Err(invalid("No recognition language is available."));
    }
    let failed = |error: String| (StatusCode::BAD_GATEWAY, error);
    let adapter = state
        .model_plan
        .asr_adapter(state.model_plan.asr_http_client().map_err(failed)?)
        .map_err(|error| failed(error.to_string()))?;
    let _permit = state
        .inference_scheduler
        .acquire_asr(InferenceWorkload::Offline)
        .await;
    let transcript = adapter
        .transcribe_pcm16(
            &pcm,
            NativeAsrOptions {
                language,
                instruction_prompt: None,
                context_bias: None,
                vocabulary_bias: Vec::new(),
                max_tokens: state.model_plan.asr_runtime().max_tokens,
            },
        )
        .await
        .map_err(|error| failed(error.to_string()))?;
    let text = transcript.text.trim();
    if text.is_empty() {
        return Err(invalid(
            "No speech was recognized. Try another recording or specify its text.",
        ));
    }
    Ok(Json(ReferenceTranscript {
        text: text.to_owned(),
    }))
}
