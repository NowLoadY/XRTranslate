//! Provider-neutral TTS session work.
//!
//! Provider selection belongs to `model_runtime::tts`; WebSocket ordering and
//! epochs remain in `main`. This module owns only clone capture and the bounded
//! synthesis worker shared by every native TTS provider.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PersistedVoiceMetadata {
    pub(crate) voice_name: String,
    pub(crate) transcript: String,
    #[serde(default)]
    pub(crate) created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedVoiceClone {
    pub(crate) voice_name: String,
    pub(crate) transcript: String,
    pub(crate) wav_bytes: Vec<u8>,
}

pub(crate) fn sanitize_voice_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if sanitized.trim().is_empty() {
        "voice".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn save_persisted_voice_clone(
    voice_clones_dir: &Path,
    voice_name: &str,
    wav: &[u8],
    transcript: &str,
) -> Result<(), std::io::Error> {
    fs::create_dir_all(voice_clones_dir)?;
    let file_stem = sanitize_voice_file_name(voice_name);
    let wav_path = voice_clones_dir.join(format!("{file_stem}.wav"));
    let meta_path = voice_clones_dir.join(format!("{file_stem}.json"));

    fs::write(&wav_path, wav)?;

    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let metadata = PersistedVoiceMetadata {
        voice_name: voice_name.to_owned(),
        transcript: transcript.to_owned(),
        created_at_ms,
    };
    let json_bytes = serde_json::to_vec_pretty(&metadata)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&meta_path, json_bytes)?;
    Ok(())
}

pub(crate) fn load_persisted_voice_clones(voice_clones_dir: &Path) -> Vec<PersistedVoiceClone> {
    if !voice_clones_dir.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(voice_clones_dir) else {
        return Vec::new();
    };

    let mut clones = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(metadata) = serde_json::from_str::<PersistedVoiceMetadata>(&content) else {
                continue;
            };
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let wav_path = voice_clones_dir.join(format!("{file_stem}.wav"));
            if !wav_path.is_file() {
                continue;
            }
            let Ok(wav_bytes) = fs::read(&wav_path) else {
                continue;
            };
            clones.push(PersistedVoiceClone {
                voice_name: metadata.voice_name,
                transcript: metadata.transcript,
                wav_bytes,
            });
        }
    }
    clones.sort_by(|a, b| a.voice_name.cmp(&b.voice_name));
    clones
}

pub(crate) async fn restore_persisted_voice_clones(
    voice_clones_dir: &Path,
    adapter: &NativeTtsAdapter,
) -> usize {
    let clones = load_persisted_voice_clones(voice_clones_dir);
    let mut restored = 0;
    for clone in clones {
        match adapter
            .register_voice(&clone.voice_name, clone.wav_bytes, &clone.transcript)
            .await
        {
            Ok(()) => {
                info!(voice = %clone.voice_name, "restored persisted voice clone");
                restored += 1;
            }
            Err(error) => {
                warn!(
                    voice = %clone.voice_name,
                    %error,
                    "failed to restore persisted voice clone into active TTS provider"
                );
            }
        }
    }
    restored
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

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
