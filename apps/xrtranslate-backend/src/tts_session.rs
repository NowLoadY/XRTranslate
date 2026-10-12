//! Provider-neutral TTS session work.
//!
//! Provider selection belongs to `model_runtime::tts`; WebSocket ordering and
//! epochs remain in `main`. This module owns only clone capture and the bounded
//! synthesis worker shared by local and online TTS connectors.

mod reference;
mod voices;
pub(crate) use reference::transcribe_reference;

pub(crate) use voices::{
    MICROPHONE_VOICE_NAME, VoiceLibrary, restore_persisted_voice_clones, select_voice, voice_status,
};

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tracing::warn;
use xrtranslate_config::AppConfig;
use xrtranslate_engine::TtsEpoch;
use xrtranslate_inference::{InferenceError, SynthesizedPcm};
use xrtranslate_vad::SAMPLE_RATE_HZ;

use crate::{PipelineGeneration, millis, model_runtime::TtsAdapter};

pub(crate) struct VoiceCloneCapture {
    pub(crate) armed: bool,
    pub(crate) ready: bool,
    pub(crate) samples: Vec<i16>,
    pub(crate) transcript: Vec<String>,
    pub(crate) minimum_samples: usize,
    pub(crate) maximum_samples: usize,
}

impl VoiceCloneCapture {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        let provider = config
            .tts
            .provider_config(&config.tts.provider)
            .and_then(serde_json::Value::as_object);
        let seconds = |key: &str, fallback: f64| {
            provider
                .and_then(|value| value.get(key))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(fallback)
        };
        Self {
            armed: false,
            ready: false,
            samples: Vec::new(),
            transcript: Vec::new(),
            minimum_samples: (seconds("clone_min_seconds", 0.5) * f64::from(SAMPLE_RATE_HZ))
                as usize,
            maximum_samples: (seconds("clone_max_seconds", 30.0) * f64::from(SAMPLE_RATE_HZ))
                as usize,
        }
    }

    pub(crate) fn arm(&mut self) {
        self.armed = true;
        self.samples.clear();
        self.transcript.clear();
    }

    pub(crate) fn clear_capture(&mut self) {
        self.armed = false;
        self.samples.clear();
        self.transcript.clear();
    }

    pub(crate) fn collected_seconds(&self) -> f32 {
        self.samples.len() as f32 / SAMPLE_RATE_HZ as f32
    }
}

pub(crate) struct TtsSynthesisJob {
    pub(crate) generation: PipelineGeneration,
    pub(crate) tts_epoch: TtsEpoch,
    pub(crate) text_chunks: Vec<String>,
    pub(crate) voice_name: String,
    pub(crate) target_language: String,
}

pub(crate) struct TtsSynthesisResult {
    pub(crate) generation: PipelineGeneration,
    pub(crate) tts_epoch: TtsEpoch,
    pub(crate) output: Result<Option<SynthesizedPcm>, InferenceError>,
    pub(crate) finished: bool,
}

pub(crate) async fn run_tts_worker(
    adapter: TtsAdapter,
    mut jobs: mpsc::Receiver<TtsSynthesisJob>,
    results: mpsc::Sender<TtsSynthesisResult>,
    mut generation: watch::Receiver<PipelineGeneration>,
    mut epoch: watch::Receiver<TtsEpoch>,
) {
    while let Some(job) = jobs.recv().await {
        let started = Instant::now();
        let output = if *generation.borrow_and_update() != job.generation
            || *epoch.borrow_and_update() != job.tts_epoch
        {
            Ok(())
        } else {
            tokio::select! {
                biased;
                _ = generation.changed() => Ok(()),
                _ = epoch.changed() => Ok(()),
                output = async {
                    for chunk in &job.text_chunks {
                        adapter.synthesize_stream(chunk, &job.voice_name, &job.target_language, |audio| {
                            let results = results.clone();
                            async move {
                                results.send(TtsSynthesisResult {
                                    generation: job.generation, tts_epoch: job.tts_epoch,
                                    output: Ok(Some(audio)), finished: false,
                                }).await.map_err(|_| xrtranslate_inference::TransportError::new("cancelled", "TTS output closed"))
                            }
                        }).await?;
                    }
                    Ok(())
                } => output,
            }
        };
        if let Err(error) = &output {
            warn!(%error, elapsed_ms = millis(started.elapsed()), "TTS synthesis failed");
        }
        if results
            .send(TtsSynthesisResult {
                generation: job.generation,
                tts_epoch: job.tts_epoch,
                output: output.map(|()| None),
                finished: true,
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

pub(crate) fn max_input_chars(config: &AppConfig) -> usize {
    if let Ok(Some(remote)) = config.remote_tts() {
        return remote
            .model
            .tts
            .as_ref()
            .expect("validated TTS model")
            .max_input_chars;
    }
    config
        .tts
        .provider_config(&config.tts.provider)
        .and_then(|provider| provider.get("max_input_chars"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(150)
        .max(1)
}

pub(crate) fn split_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = text.trim();
    let max_chars = max_chars.max(1);
    while !remaining.is_empty() {
        let Some((limit, _)) = remaining.char_indices().nth(max_chars) else {
            chunks.push(remaining.to_owned());
            break;
        };
        let end = remaining[..limit]
            .char_indices()
            .rev()
            .find_map(|(index, ch)| {
                (ch.is_whitespace()
                    || matches!(
                        ch,
                        '.' | ',' | '!' | '?' | ';' | ':' | '。' | '，' | '！' | '？' | '；' | '：'
                    ))
                .then_some(index + ch.len_utf8())
            })
            .unwrap_or(limit);
        let chunk = remaining[..end].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_owned());
        }
        remaining = remaining[end..].trim_start();
    }
    chunks
}
