//! Provider-neutral TTS session work.
//!
//! Provider selection belongs to `model_runtime::tts`; WebSocket ordering and
//! epochs remain in `main`. This module owns only clone capture and the bounded
//! synthesis worker shared by every native TTS provider.

mod reference;
mod voices;
pub(crate) use reference::transcribe_reference;

#[cfg(test)]
use std::fs;
pub(crate) use voices::{
    MICROPHONE_VOICE_NAME, VoiceLibrary, restore_persisted_voice_clones, select_voice, voice_status,
};
#[cfg(test)]
use voices::{load_persisted_voice_clones, save_persisted_voice_clone};

use tokio::{sync::mpsc, time::Instant};
use tracing::{info, warn};
use xrtranslate_config::AppConfig;
use xrtranslate_engine::TtsEpoch;
use xrtranslate_inference::{InferenceError, SynthesizedPcm};
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
    pub(crate) output: Result<SynthesizedPcm, InferenceError>,
    pub(crate) finished: bool,
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
        let mut output_bytes = 0;
        for (index, chunk) in job.text_chunks.iter().enumerate() {
            let output = adapter
                .synthesize(chunk, &job.voice_name, &job.target_language)
                .await
                .map(|audio| {
                    output_bytes += audio.bytes.len();
                    audio
                });
            let finished = output.is_err() || index + 1 == chunk_count;
            if finished {
                match &output {
                    Ok(audio) => info!(
                        generation = ?job.generation,
                        voice = %job.voice_name,
                        chunk_count,
                        output_bytes,
                        sample_rate = audio.sample_rate,
                        elapsed_ms = millis(started_at.elapsed()),
                        "TTS synthesis completed"
                    ),
                    Err(error) => warn!(
                        generation = ?job.generation,
                        voice = %job.voice_name,
                        completed_chunks = index,
                        elapsed_ms = millis(started_at.elapsed()),
                        %error,
                        "TTS synthesis failed"
                    ),
                }
            }
            if results
                .send(TtsSynthesisResult {
                    generation: job.generation,
                    tts_epoch: job.tts_epoch,
                    output,
                    finished,
                })
                .await
                .is_err()
            {
                return;
            }
            if finished {
                break;
            }
        }
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

    #[test]
    fn persisted_voice_clones_save_and_load_round_trip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "xrtranslate-test-voice-clones-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&temp_dir);

        let wav_data = b"RIFFfake_wav_data_for_testing";
        let transcript = "testing voice clone transcript";
        save_persisted_voice_clone(&temp_dir, "xrtranslate_microphone", wav_data, transcript)
            .expect("save should succeed");

        let loaded = load_persisted_voice_clones(&temp_dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].voice_name, "xrtranslate_microphone");
        assert_eq!(loaded[0].transcript, transcript);
        assert_eq!(loaded[0].wav_bytes, wav_data);

        // Overwrite with new data
        let new_wav = b"RIFFnew_wav_data";
        let new_transcript = "updated transcript";
        save_persisted_voice_clone(&temp_dir, "xrtranslate_microphone", new_wav, new_transcript)
            .expect("overwrite should succeed");

        let reloaded = load_persisted_voice_clones(&temp_dir);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].transcript, new_transcript);
        assert_eq!(reloaded[0].wav_bytes, new_wav);

        // User cards live in independent directories, even when display names match.
        let catalog = xrtranslate_assets::voices::VoiceCatalog::new(&temp_dir);
        let reference = xrtranslate_assets::voices::builtin("cute").unwrap();
        let first = catalog
            .add(
                "Same name",
                "First",
                reference.transcript,
                reference.pcm16(),
            )
            .unwrap();
        let second = catalog
            .add(
                "Same name",
                "Second",
                reference.transcript,
                reference.pcm16(),
            )
            .unwrap();
        assert_ne!(first.id, second.id);
        let cards = catalog.list().unwrap();
        assert_eq!(cards.iter().filter(|card| !card.is_builtin()).count(), 2);
        let (wav, text) = catalog.reference(&first.id).unwrap();
        assert_eq!(wav.as_ref(), reference.wav);
        assert_eq!(text.as_ref(), reference.transcript.trim());
        assert!(catalog.reference("../xrtranslate_microphone").is_err());
        assert!(catalog.add("", "", "words", reference.pcm16()).is_err());
        assert_eq!(catalog.list().unwrap().len(), cards.len());
        assert_eq!(load_persisted_voice_clones(&temp_dir).len(), 1);
        assert_eq!(
            fs::read(temp_dir.join("xrtranslate_microphone.wav")).unwrap(),
            new_wav
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
