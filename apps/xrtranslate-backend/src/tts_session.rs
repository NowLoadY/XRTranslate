//! Provider-neutral TTS session work.
//!
//! Provider selection belongs to `model_runtime::tts`; WebSocket ordering and
//! epochs remain in `main`. This module owns only clone capture and the bounded
//! synthesis worker shared by every native TTS provider.

use tokio::{sync::mpsc, time::Instant};
use tracing::{info, warn};
use xrtranslate_config::AppConfig;
use xrtranslate_engine::TtsEpoch;
use xrtranslate_inference::{InferenceError, SynthesizedPcm};
use xrtranslate_protocol::AudioSource;
use xrtranslate_vad::SAMPLE_RATE_HZ;

use crate::{PipelineGeneration, millis, model_runtime::NativeTtsAdapter};

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
    pub(crate) output: Result<Vec<SynthesizedPcm>, InferenceError>,
}

pub(crate) async fn run_tts_worker(
    adapter: NativeTtsAdapter,
    mut jobs: mpsc::Receiver<TtsSynthesisJob>,
    results: mpsc::Sender<TtsSynthesisResult>,
) {
    while let Some(job) = jobs.recv().await {
        let started_at = Instant::now();
        let chunk_count = job.text_chunks.len();
        let input_chars = job
            .text_chunks
            .iter()
            .map(|chunk| chunk.chars().count())
            .sum::<usize>();
        info!(
            generation = ?job.generation,
            voice = %job.voice_name,
            target_language = %job.target_language,
            chunk_count,
            input_chars,
            "TTS synthesis started"
        );
        let mut audio = Vec::with_capacity(job.text_chunks.len());
        let mut failure = None;
        for chunk in job.text_chunks {
            match adapter
                .synthesize(&chunk, &job.voice_name, &job.target_language)
                .await
            {
                Ok(chunk) => audio.push(chunk),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let output = failure.map_or_else(|| Ok(audio), Err);
        match &output {
            Ok(chunks) => {
                let output_bytes = chunks.iter().map(|chunk| chunk.bytes.len()).sum::<usize>();
                let sample_rate = chunks.first().map_or(0, |chunk| chunk.sample_rate);
                info!(
                    generation = ?job.generation,
                    voice = %job.voice_name,
                    chunk_count = chunks.len(),
                    output_bytes,
                    sample_rate,
                    elapsed_ms = millis(started_at.elapsed()),
                    "TTS synthesis completed"
                );
            }
            Err(error) => warn!(
                generation = ?job.generation,
                voice = %job.voice_name,
                elapsed_ms = millis(started_at.elapsed()),
                %error,
                "TTS synthesis failed"
            ),
        }
        if results
            .send(TtsSynthesisResult {
                generation: job.generation,
                tts_epoch: job.tts_epoch,
                output,
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

pub(crate) fn clone_voice_name(source: AudioSource) -> &'static str {
    match source {
        AudioSource::Microphone => "xrtranslate_microphone",
        AudioSource::SystemAudio => "xrtranslate_system_audio",
    }
}

pub(crate) fn max_input_chars(config: &AppConfig) -> usize {
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
    let mut current = String::new();
    for character in text.trim().chars() {
        current.push(character);
        let boundary = character.is_whitespace()
            || matches!(
                character,
                '.' | ',' | '!' | '?' | ';' | ':' | '。' | '，' | '！' | '？' | '；' | '：'
            );
        if current.chars().count() >= max_chars
            || (boundary && current.chars().count() >= max_chars / 2)
        {
            let chunk = current.trim();
            if !chunk.is_empty() {
                chunks.push(chunk.to_owned());
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_owned());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_chunks_preserve_content_and_provider_limit() {
        let chunks = split_text("你好，世界。这是一段语音。", 6);
        assert_eq!(chunks.concat(), "你好，世界。这是一段语音。");
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 6));
    }
}
