//! Pure-Rust OpenVoice inference using ONNX Runtime.
//!
//! The provider uses NVIDIA's OpenVoice v3 package: MeloTTS English v3 as the
//! base voice and the OpenVoice V2 tone-color converter. No Python process or
//! local model service participates at runtime.

mod frontend;
mod model;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use half::f16;

use crate::{
    InferenceError, SynthesizedPcm,
    tts::{decode_pcm16_wav, onnx_runtime::OnnxExecutionDevice},
};

use model::OpenVoiceRuntime;

#[derive(Clone, Copy, Debug)]
pub struct OpenVoiceSynthesisOptions {
    pub speed: f32,
}

impl Default for OpenVoiceSynthesisOptions {
    fn default() -> Self {
        Self { speed: 1.0 }
    }
}

impl OpenVoiceSynthesisOptions {
    fn validate(self) -> Result<Self, InferenceError> {
        if !self.speed.is_finite() || !(0.5..=2.0).contains(&self.speed) {
            return Err(openvoice_error("speed must be finite and in 0.5..=2.0"));
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct OpenVoiceOnnxAdapter {
    state: Arc<Mutex<OpenVoiceState>>,
}

struct OpenVoiceState {
    model_dir: PathBuf,
    device: OnnxExecutionDevice,
    threads: usize,
    synthesis: OpenVoiceSynthesisOptions,
    voices: HashMap<String, Vec<f16>>,
    runtime: Option<OpenVoiceRuntime>,
}

impl OpenVoiceOnnxAdapter {
    pub fn new(
        model_dir: impl Into<PathBuf>,
        device: OnnxExecutionDevice,
        threads: usize,
    ) -> Result<Self, InferenceError> {
        Self::with_synthesis_options(
            model_dir,
            device,
            threads,
            OpenVoiceSynthesisOptions::default(),
        )
    }

    pub fn with_synthesis_options(
        model_dir: impl Into<PathBuf>,
        device: OnnxExecutionDevice,
        threads: usize,
        synthesis: OpenVoiceSynthesisOptions,
    ) -> Result<Self, InferenceError> {
        let model_dir = model_dir.into();
        for relative in [
            "model_config.json",
            "models/bert.onnx",
            "models/melo.onnx",
            "models/converter.onnx",
            "models/reference_encoder.onnx",
            "frontend/cmudict.json",
            "frontend/bert_vocab.txt",
            "voices/en_newest.bin",
        ] {
            if !model_dir.join(relative).is_file() {
                return Err(openvoice_error(format!(
                    "model file is missing: {}",
                    model_dir.join(relative).display()
                )));
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(OpenVoiceState {
                model_dir,
                device,
                threads: threads.max(1),
                synthesis: synthesis.validate()?,
                voices: HashMap::new(),
                runtime: None,
            })),
        })
    }

    pub async fn prepare(&self) -> Result<OnnxExecutionDevice, InferenceError> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| openvoice_error("runtime lock poisoned"))?;
            state.prepare()
        })
        .await
        .map_err(|error| openvoice_error(format!("TTS preparation worker failed: {error}")))?
    }

    pub async fn register_voice(
        &self,
        name: &str,
        reference_wav: Vec<u8>,
        _transcript: &str,
    ) -> Result<(), InferenceError> {
        let state = Arc::clone(&self.state);
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| openvoice_error("runtime lock poisoned"))?;
            state.register_voice(&name, &reference_wav)
        })
        .await
        .map_err(|error| openvoice_error(format!("voice registration worker failed: {error}")))?
    }

    pub async fn has_voice(&self, name: &str) -> bool {
        let state = Arc::clone(&self.state);
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            state
                .lock()
                .is_ok_and(|state| state.voices.contains_key(&name))
        })
        .await
        .unwrap_or(false)
    }

    pub async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        target_language: &str,
    ) -> Result<SynthesizedPcm, InferenceError> {
        if !is_english(target_language) {
            return Err(openvoice_error(format!(
                "NVIDIA OpenVoice v3 supports English synthesis, not {target_language:?}"
            )));
        }
        let state = Arc::clone(&self.state);
        let text = text.to_owned();
        let voice = voice.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| openvoice_error("runtime lock poisoned"))?;
            state.synthesize(&text, &voice)
        })
        .await
        .map_err(|error| openvoice_error(format!("TTS worker failed: {error}")))?
    }
}

impl OpenVoiceState {
    fn prepare(&mut self) -> Result<OnnxExecutionDevice, InferenceError> {
        if self.runtime.is_none() {
            self.runtime = Some(OpenVoiceRuntime::load(
                &self.model_dir,
                self.device,
                self.threads,
            )?);
        }
        Ok(self
            .runtime
            .as_ref()
            .expect("initialized above")
            .active_device())
    }

    fn register_voice(&mut self, name: &str, wav: &[u8]) -> Result<(), InferenceError> {
        self.prepare()?;
        let decoded = decode_pcm16_wav(wav)?;
        let embedding = self
            .runtime
            .as_mut()
            .expect("initialized above")
            .encode_reference(&decoded.bytes, decoded.sample_rate)?;
        self.voices.insert(name.to_owned(), embedding);
        Ok(())
    }

    fn synthesize(&mut self, text: &str, voice: &str) -> Result<SynthesizedPcm, InferenceError> {
        let embedding = self
            .voices
            .get(voice)
            .cloned()
            .ok_or_else(|| openvoice_error(format!("voice profile is not ready: {voice}")))?;
        self.prepare()?;
        self.runtime
            .as_mut()
            .expect("initialized above")
            .synthesize(text, &embedding, self.synthesis.speed)
    }
}

fn is_english(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "en" | "en-us" | "en-gb" | "english"
    )
}

fn openvoice_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.providers.openvoice",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_gate_is_explicit_about_the_ngc_package_scope() {
        assert!(is_english("en"));
        assert!(is_english("EN-US"));
        assert!(!is_english("zh"));
    }

    #[tokio::test]
    #[ignore = "requires the optional OpenVoice v3 model package"]
    async fn installed_model_clones_and_synthesizes_english_without_python() {
        if let Some(libraries) = std::env::var_os("XRTRANSLATE_OPENVOICE_CUDA_PRELOAD") {
            crate::preload_onnx_cuda_libraries(
                &std::env::split_paths(&libraries).collect::<Vec<_>>(),
            )
            .unwrap();
        }
        if let Some(core) = std::env::var_os("XRTRANSLATE_ORT_DYLIB_PATH") {
            crate::initialize_onnx_runtime(std::path::Path::new(&core)).unwrap();
        }
        let model_dir = std::env::var_os("XRTRANSLATE_OPENVOICE_MODEL_DIR")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_OPENVOICE_MODEL_DIR");
        let device = OnnxExecutionDevice::from_config(
            &std::env::var("XRTRANSLATE_OPENVOICE_DEVICE").unwrap_or_else(|_| "auto".into()),
        );
        let adapter = OpenVoiceOnnxAdapter::new(model_dir, device, 5).unwrap();
        let prepared = adapter.prepare().await.unwrap();
        if std::env::var_os("XRTRANSLATE_OPENVOICE_REQUIRE_CUDA").is_some() {
            assert_eq!(prepared, OnnxExecutionDevice::Cuda);
        }
        let samples = (0..32_000)
            .map(|index| {
                let phase = index as f32 * 220.0 * std::f32::consts::TAU / 16_000.0;
                (phase.sin() * 4_000.0) as i16
            })
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let wav = crate::pcm16_mono_16khz_to_wav(&samples).unwrap();
        adapter.register_voice("smoke", wav, "").await.unwrap();
        let audio = adapter
            .synthesize("Native OpenVoice speech works.", "smoke", "en")
            .await
            .unwrap();
        eprintln!(
            "openvoice_device={prepared:?} samples={} seconds={:.3}",
            audio.bytes.len() / 2,
            audio.bytes.len() as f64 / 2.0 / f64::from(audio.sample_rate)
        );
        assert_eq!(audio.sample_rate, model::OUTPUT_SAMPLE_RATE);
        assert!(audio.bytes.len() > audio.sample_rate as usize);
        assert!(
            audio
                .bytes
                .chunks_exact(2)
                .any(|sample| { i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() > 128 })
        );
    }
}
