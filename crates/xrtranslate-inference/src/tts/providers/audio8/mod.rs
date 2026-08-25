//! Native Audio8 ONNX inference. This is a Rust port of the provider's
//! reference runtime contract; no Python process or local HTTP service is
//! involved.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use half::f16;
use ndarray::{Array1, Array2, Array3};
use ort::{
    memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType},
    session::Session,
    value::{
        DynValue, PrimitiveTensorElementType, Tensor, TensorElementType, TensorRef, Value,
        ValueType,
    },
};
use serde::Deserialize;
use tokenizers::Tokenizer;
mod sampling;
mod text;

use sampling::{NumpyPcg64, sample, sample_semantic};
use text::{clean_text, format_reference_text, normalize_reference_transcript};

use crate::{
    InferenceError, SynthesizedPcm,
    tts::audio::resample_pcm16,
    tts::onnx_runtime::{
        ActiveOnnxDevice, OnnxExecutionDevice, build_session as build_onnx_session,
        build_session_group,
    },
};

/// Compatibility name retained for existing callers. Device selection is
/// shared by every native ONNX TTS provider.
pub type Audio8ExecutionDevice = OnnxExecutionDevice;
type ActiveAudio8Device = ActiveOnnxDevice;

/// Sampling controls used by Audio8's interactive ONNX service. Keeping these
/// explicit prevents the UI route from silently drifting from the provider
/// runtime when quality-sensitive defaults change.
#[derive(Clone, Copy, Debug)]
pub struct Audio8SynthesisOptions {
    pub max_new_tokens: usize,
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: usize,
}

impl Default for Audio8SynthesisOptions {
    fn default() -> Self {
        Self {
            max_new_tokens: 1024,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 50,
        }
    }
}

impl Audio8SynthesisOptions {
    fn validate(self) -> Result<Self, InferenceError> {
        if self.max_new_tokens == 0 {
            return Err(native_error("max_new_tokens must be greater than zero"));
        }
        if !self.temperature.is_finite() || self.temperature <= 0.0 {
            return Err(native_error(
                "temperature must be finite and greater than zero",
            ));
        }
        if !self.top_p.is_finite() || !(0.0..=1.0).contains(&self.top_p) || self.top_p == 0.0 {
            return Err(native_error("top_p must be finite and in (0, 1]"));
        }
        if self.top_k == 0 {
            return Err(native_error("top_k must be greater than zero"));
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct Audio8OnnxAdapter {
    state: Arc<Mutex<Audio8State>>,
}

struct Audio8State {
    model_dir: PathBuf,
    device: Audio8ExecutionDevice,
    threads: usize,
    synthesis: Audio8SynthesisOptions,
    voices: HashMap<String, VoiceProfile>,
    online: Option<OnlineRuntime>,
}

#[derive(Clone)]
struct VoiceProfile {
    reference_text: String,
    codes: Vec<u16>,
    frames: usize,
}

impl Audio8OnnxAdapter {
    pub fn new(
        model_dir: impl Into<PathBuf>,
        device: Audio8ExecutionDevice,
        threads: usize,
    ) -> Result<Self, InferenceError> {
        Self::with_synthesis_options(
            model_dir,
            device,
            threads,
            Audio8SynthesisOptions::default(),
        )
    }

    pub fn with_synthesis_options(
        model_dir: impl Into<PathBuf>,
        device: Audio8ExecutionDevice,
        threads: usize,
        synthesis: Audio8SynthesisOptions,
    ) -> Result<Self, InferenceError> {
        let model_dir = model_dir.into();
        for relative in [
            "runtime_manifest.json",
            "slow_ar_fp16.onnx",
            "fast_ar_fp16.onnx",
            "codec_decoder_fp16.onnx",
            "registration/codec_encoder_fp16.onnx",
            "tokenizer/tokenizer.json",
        ] {
            if !model_dir.join(relative).is_file() {
                return Err(native_error(format!(
                    "model file is missing: {}",
                    model_dir.join(relative).display()
                )));
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(Audio8State {
                model_dir,
                device,
                threads: threads.max(1),
                synthesis: synthesis.validate()?,
                voices: HashMap::new(),
                online: None,
            })),
        })
    }

    /// Replaces the named profile atomically. Only encoded codec tokens and
    /// the exact transcript survive; reference PCM is released after this
    /// call, so repeated cloning cannot grow the capture store.
    pub async fn register_voice(
        &self,
        name: &str,
        reference_wav: Vec<u8>,
        transcript: &str,
    ) -> Result<(), InferenceError> {
        let state = Arc::clone(&self.state);
        let name = name.to_owned();
        let transcript = normalize_reference_transcript(transcript);
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| native_error("runtime lock poisoned"))?;
            state.register_voice(&name, &reference_wav, &transcript)
        })
        .await
        .map_err(|error| native_error(format!("voice registration worker failed: {error}")))?
    }

    pub async fn synthesize(
        &self,
        text: &str,
        voice: &str,
    ) -> Result<SynthesizedPcm, InferenceError> {
        let state = Arc::clone(&self.state);
        let text = text.to_owned();
        let voice = voice.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| native_error("runtime lock poisoned"))?;
            state.synthesize(&text, &voice)
        })
        .await
        .map_err(|error| native_error(format!("TTS worker failed: {error}")))?
    }

    /// Loads the online Slow/Fast/decoder sessions without generating audio
    /// and reports the execution provider that was actually selected. Calls
    /// after the first successful preparation reuse the same runtime.
    pub async fn prepare(&self) -> Result<Audio8ExecutionDevice, InferenceError> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| native_error("runtime lock poisoned"))?;
            state.prepare()
        })
        .await
        .map_err(|error| native_error(format!("TTS preparation worker failed: {error}")))?
    }

    /// Reports whether the named encoded voice is available in this shared
    /// runtime. Profiles survive WebSocket session replacement, while their
    /// source PCM is released immediately after registration.
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

    /// Returns the provider selected for the loaded Slow/Fast pair. `None`
    /// means synthesis has not initialized the online runtime yet.
    pub async fn active_device(&self) -> Option<Audio8ExecutionDevice> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || {
            state.lock().ok().and_then(|state| {
                state
                    .online
                    .as_ref()
                    .map(|runtime| runtime.active_device.execution_device())
            })
        })
        .await
        .unwrap_or(None)
    }
}

impl Audio8State {
    fn register_voice(
        &mut self,
        name: &str,
        reference_wav: &[u8],
        transcript: &str,
    ) -> Result<(), InferenceError> {
        if transcript.is_empty() {
            return Err(native_error("reference transcript is empty"));
        }
        // Free the three online sessions before loading the registration
        // encoder, matching the provider's bounded-memory lifecycle.
        self.online = None;
        let runtime_manifest =
            read_manifest::<RuntimeManifest>(&self.model_dir.join("runtime_manifest.json"))?;
        let registration_manifest = read_manifest::<RegistrationManifest>(
            &self
                .model_dir
                .join("registration/registration_manifest.json"),
        )?;
        if registration_manifest.model_fingerprint != runtime_manifest.model_fingerprint {
            return Err(native_error(
                "registration model fingerprint does not match the online model",
            ));
        }
        if registration_manifest.num_codebooks != runtime_manifest.num_codebooks
            || registration_manifest.sample_rate != runtime_manifest.codec_sample_rate
            || registration_manifest.frame_length == 0
        {
            return Err(native_error(
                "registration manifest does not match the Audio8 runtime contract",
            ));
        }
        let decoded = crate::tts::decode_pcm16_wav(reference_wav)?;
        let input = resample_pcm16_to_f16(
            &decoded.bytes,
            decoded.sample_rate,
            registration_manifest.sample_rate,
        )?;
        let frames = input.len().div_ceil(registration_manifest.frame_length);
        let mut padded = input;
        padded.resize(frames * registration_manifest.frame_length, f16::ZERO);
        let mut encoder = build_session(
            &self.model_dir.join("registration/codec_encoder_fp16.onnx"),
            // Audio8's registration export has no GPU provider contract. A
            // numerically different reference code is carried into every
            // generated frame, so keep this one-shot stage on the provider's
            // reference CPU path. The autoregressive models still use the
            // configured accelerator.
            Audio8ExecutionDevice::Cpu,
            self.threads,
        )?;
        let input = Array3::from_shape_vec((1, 1, padded.len()), padded)
            .map_err(|error| native_error(error.to_string()))?;
        let outputs = encoder
            .run(ort::inputs!["audio" => Value::from_array(input).map_err(ort_error)?])
            .map_err(ort_error)?;
        let (shape, values) = outputs["codes"]
            .try_extract_tensor::<i64>()
            .map_err(ort_error)?;
        let expected_codebooks = registration_manifest.num_codebooks;
        if shape.len() != 3
            || shape[0] != 1
            || shape[1] != expected_codebooks as i64
            || shape[2] <= 0
            || values.len() != expected_codebooks * shape[2] as usize
        {
            return Err(native_error(
                "voice encoder returned an invalid code tensor",
            ));
        }
        let frames = shape[2] as usize;
        let codes = values
            .iter()
            .map(|value| {
                u16::try_from(*value)
                    .map_err(|_| native_error("voice encoder returned an out-of-range code"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        drop(outputs);
        drop(encoder);

        // Loading the online runtime while the one-shot registration encoder
        // is still alive would temporarily retain both model sets. Prepare it
        // only after releasing the encoder, then commit the profile and
        // runtime together so a failed preparation cannot replace a working
        // clone.
        let online = OnlineRuntime::load(&self.model_dir, self.device, self.threads)?;
        self.voices.insert(
            name.to_owned(),
            VoiceProfile {
                reference_text: transcript.to_owned(),
                codes,
                frames,
            },
        );
        self.online = Some(online);
        Ok(())
    }

    fn prepare(&mut self) -> Result<Audio8ExecutionDevice, InferenceError> {
        if self.online.is_none() {
            self.online = Some(OnlineRuntime::load(
                &self.model_dir,
                self.device,
                self.threads,
            )?);
        }
        Ok(self
            .online
            .as_ref()
            .expect("initialized above")
            .active_device
            .execution_device())
    }

    fn synthesize(&mut self, text: &str, voice: &str) -> Result<SynthesizedPcm, InferenceError> {
        let profile = self
            .voices
            .get(voice)
            .cloned()
            .ok_or_else(|| native_error(format!("voice profile is not ready: {voice}")))?;
        self.prepare()?;
        self.online
            .as_mut()
            .expect("initialized above")
            .synthesize(text, &profile, self.synthesis)
    }
}

#[derive(Deserialize)]
struct RuntimeManifest {
    max_seq_len: usize,
    num_layers: usize,
    num_fast_layers: usize,
    num_codebooks: usize,
    n_local_heads: usize,
    fast_n_local_heads: usize,
    head_dim: usize,
    fast_head_dim: usize,
    fast_dim: usize,
    codebook_size: usize,
    semantic_begin_id: i64,
    semantic_end_id: i64,
    im_end_id: i64,
    codec_sample_rate: u32,
    #[serde(default)]
    decoder_provider: Option<String>,
    model_fingerprint: String,
}

#[derive(Deserialize)]
struct RegistrationManifest {
    sample_rate: u32,
    num_codebooks: usize,
    frame_length: usize,
    model_fingerprint: String,
}

struct OnlineRuntime {
    manifest: RuntimeManifest,
    tokenizer: Tokenizer,
    slow: Session,
    fast: Session,
    decoder: Session,
    active_device: ActiveAudio8Device,
    slow_cache: Option<CacheSet>,
    fast_cache: Option<CacheSet>,
}

impl OnlineRuntime {
    fn load(
        model_dir: &Path,
        device: Audio8ExecutionDevice,
        threads: usize,
    ) -> Result<Self, InferenceError> {
        let manifest = serde_json::from_slice::<RuntimeManifest>(
            &std::fs::read(model_dir.join("runtime_manifest.json"))
                .map_err(|error| native_error(error.to_string()))?,
        )
        .map_err(|error| native_error(error.to_string()))?;
        let tokenizer = Tokenizer::from_file(model_dir.join("tokenizer/tokenizer.json"))
            .map_err(|error| native_error(error.to_string()))?;
        let slow_path = model_dir.join("slow_ar_fp16.onnx");
        #[cfg(test)]
        let slow_path = std::env::var_os("XRTRANSLATE_AUDIO8_SLOW_MODEL")
            .map(PathBuf::from)
            .unwrap_or(slow_path);
        let fast_path = model_dir.join("fast_ar_fp16.onnx");
        #[cfg(test)]
        let fast_path = std::env::var_os("XRTRANSLATE_AUDIO8_FAST_MODEL")
            .map(PathBuf::from)
            .unwrap_or(fast_path);
        let (slow, fast, active_device) =
            build_generator_sessions(&slow_path, &fast_path, device, threads)?;
        let decoder_device = codec_execution_device(
            manifest.decoder_provider.as_deref(),
            active_device.execution_device(),
        );
        Ok(Self {
            manifest,
            tokenizer,
            slow,
            fast,
            decoder: build_session(
                &model_dir.join("codec_decoder_fp16.onnx"),
                decoder_device,
                threads,
            )?,
            active_device,
            slow_cache: None,
            fast_cache: None,
        })
    }

    fn synthesize(
        &mut self,
        text: &str,
        profile: &VoiceProfile,
        options: Audio8SynthesisOptions,
    ) -> Result<SynthesizedPcm, InferenceError> {
        let prompt = self.build_prompt(text, profile)?;
        let prompt_len = prompt.len() / (self.manifest.num_codebooks + 1);
        #[cfg(test)]
        if std::env::var_os("XRTRANSLATE_AUDIO8_TRACE_CODES").is_some() {
            let hash = prompt
                .iter()
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, token| {
                    (hash ^ *token as u64).wrapping_mul(0x100_0000_01b3)
                });
            eprintln!("prompt_len={prompt_len} prompt_hash={hash:016x}");
        }
        if prompt_len >= self.manifest.max_seq_len {
            return Err(native_error(format!(
                "TTS prompt length {prompt_len} exceeds {}",
                self.manifest.max_seq_len
            )));
        }
        let mut slow_caches = self.slow_cache.take().unwrap_or_else(|| {
            CacheSet::new_for_session(
                &self.slow,
                self.active_device,
                self.manifest.num_layers * 2,
                self.manifest.n_local_heads,
                self.manifest.max_seq_len,
                self.manifest.head_dim,
            )
        });
        slow_caches.clear();
        let mut fast_caches = self.fast_cache.take().unwrap_or_else(|| {
            CacheSet::new_for_session(
                &self.fast,
                self.active_device,
                self.manifest.num_fast_layers * 2,
                self.manifest.fast_n_local_heads,
                self.manifest.num_codebooks,
                self.manifest.fast_head_dim,
            )
        });
        fast_caches.clear();
        let positions = (0..prompt_len)
            .map(|value| value as i64)
            .collect::<Vec<_>>();
        let (mut logits, mut hidden) =
            self.slow_step(&prompt, prompt_len, &positions, &mut slow_caches)?;
        let mut rng = NumpyPcg64::seed_42();
        let mut previous = Vec::new();
        let mut frames = Vec::<i64>::new();
        let max_tokens = options
            .max_new_tokens
            .min(self.manifest.max_seq_len - prompt_len);
        for step in 0..max_tokens {
            let semantic = sample_semantic(&logits, &previous, &self.manifest, options, &mut rng)?;
            if semantic == self.manifest.im_end_id {
                break;
            }
            previous.push(semantic);
            if previous.len() > 10 {
                previous.remove(0);
            }
            // Reuse the allocation for every semantic frame. Fast AR starts a
            // fresh codebook sequence each time, so only zeroing is required.
            fast_caches.clear();
            self.fast_step(&hidden, 0, true, 0, &mut fast_caches)?;
            let mut token = (semantic - self.manifest.semantic_begin_id)
                .clamp(0, self.manifest.codebook_size as i64 - 1);
            frames.push(token);
            for position in 1..self.manifest.num_codebooks {
                let fast_logits =
                    self.fast_step(&hidden, token, false, position, &mut fast_caches)?;
                token = sample(
                    &fast_logits,
                    options.temperature,
                    options.top_p,
                    options.top_k,
                    &mut rng,
                ) as i64;
                frames.push(token);
            }
            #[cfg(test)]
            if step < 12 && std::env::var_os("XRTRANSLATE_AUDIO8_TRACE_CODES").is_some() {
                eprintln!(
                    "step={step} semantic={semantic} codes={:?}",
                    &frames[frames.len() - self.manifest.num_codebooks..]
                );
            }
            let mut column = Vec::with_capacity(self.manifest.num_codebooks + 1);
            column.push(semantic);
            column.extend_from_slice(
                &frames[frames.len() - self.manifest.num_codebooks..frames.len()],
            );
            (logits, hidden) = self.slow_step(
                &column,
                1,
                &[i64::try_from(prompt_len + step)
                    .map_err(|_| native_error("TTS position overflow"))?],
                &mut slow_caches,
            )?;
        }
        if frames.is_empty() {
            return Err(native_error("Audio8 produced no codec frames"));
        }
        let frame_count = frames.len() / self.manifest.num_codebooks;
        tracing::info!(
            frame_count,
            output_seconds =
                frame_count as f64 * 2048.0 / f64::from(self.manifest.codec_sample_rate),
            "Audio8 generated codec frames"
        );
        // Runtime access is serialized by Audio8State, so one allocation per
        // graph is enough and avoids page-lock/allocation churn between short
        // real-time utterances.
        self.slow_cache = Some(slow_caches);
        self.fast_cache = Some(fast_caches);
        // Frames are generated frame-major; the decoder expects
        // [codebook, frame].
        let mut decoder_codes = vec![0_i64; frames.len()];
        for frame in 0..frame_count {
            for codebook in 0..self.manifest.num_codebooks {
                decoder_codes[codebook * frame_count + frame] =
                    frames[frame * self.manifest.num_codebooks + codebook];
            }
        }
        self.decode_codebook_major(decoder_codes, frame_count)
    }

    fn decode_codebook_major(
        &mut self,
        decoder_codes: Vec<i64>,
        frame_count: usize,
    ) -> Result<SynthesizedPcm, InferenceError> {
        let codes =
            Array3::from_shape_vec((1, self.manifest.num_codebooks, frame_count), decoder_codes)
                .map_err(|error| native_error(error.to_string()))?;
        let outputs = self
            .decoder
            .run(ort::inputs!["codes" => Value::from_array(codes).map_err(ort_error)?])
            .map_err(ort_error)?;
        let (_, audio) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if audio.iter().any(|sample| !sample.is_finite()) {
            return Err(native_error("Audio8 decoder returned non-finite audio"));
        }
        let mut bytes = Vec::with_capacity(audio.len() * 2);
        for sample in audio {
            // NumPy's float-to-int cast truncates toward zero.
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Ok(SynthesizedPcm {
            bytes,
            sample_rate: self.manifest.codec_sample_rate,
        })
    }

    fn build_prompt(&self, text: &str, profile: &VoiceProfile) -> Result<Vec<i64>, InferenceError> {
        let reference = format_reference_text(&profile.reference_text);
        // Keep the same tokenizer boundaries as the provider runtime. Joining
        // these fragments before tokenization changes BPE merges around the
        // special prompt sections and can destabilize semantic generation.
        let prefix = self.encode_parts(&[
            "<|im_start|>system\n",
            "convert the provided text to speech reference to the following:\n\nText:\n",
            &reference,
            "\n\nSpeech:\n",
        ])?;
        let target = clean_text(text);
        let suffix = self.encode_parts(&[
            "<|im_end|>\n",
            "<|im_start|>user\n",
            &target,
            "<|im_end|>\n",
            "<|im_start|>assistant\n<|voice|>",
        ])?;
        let sequence = prefix.len() + profile.frames + suffix.len();
        let rows = self.manifest.num_codebooks + 1;
        let mut values = vec![0_i64; rows * sequence];
        values[..prefix.len()].copy_from_slice(&prefix);
        for frame in 0..profile.frames {
            values[prefix.len() + frame] =
                i64::from(profile.codes[frame]) + self.manifest.semantic_begin_id;
        }
        values[prefix.len() + profile.frames..sequence].copy_from_slice(&suffix);
        for codebook in 0..self.manifest.num_codebooks {
            let source = &profile.codes[codebook * profile.frames..(codebook + 1) * profile.frames];
            let begin = (codebook + 1) * sequence + prefix.len();
            for (offset, code) in source.iter().enumerate() {
                values[begin + offset] = i64::from(*code);
            }
        }
        Ok(values)
    }

    fn encode(&self, text: &str) -> Result<Vec<i64>, InferenceError> {
        self.tokenizer
            .encode(text, false)
            .map(|encoding| encoding.get_ids().iter().map(|id| i64::from(*id)).collect())
            .map_err(|error| native_error(error.to_string()))
    }

    fn encode_parts(&self, parts: &[&str]) -> Result<Vec<i64>, InferenceError> {
        let mut tokens = Vec::new();
        for part in parts {
            tokens.extend(self.encode(part)?);
        }
        Ok(tokens)
    }

    fn slow_step(
        &mut self,
        codes: &[i64],
        sequence: usize,
        positions: &[i64],
        caches: &mut CacheSet,
    ) -> Result<(Vec<f32>, SlowHidden), InferenceError> {
        let rows = self.manifest.num_codebooks + 1;
        let codes = Array3::from_shape_vec((1, rows, sequence), codes.to_vec())
            .map_err(|error| native_error(error.to_string()))?;
        let positions_value = Array1::from_vec(positions.to_vec());
        let mut inputs = vec![
            (
                "codes".to_owned(),
                ort::session::SessionInputValue::from(
                    TensorRef::from_array_view(&codes).map_err(ort_error)?,
                ),
            ),
            (
                "input_pos".to_owned(),
                ort::session::SessionInputValue::from(
                    TensorRef::from_array_view(&positions_value).map_err(ort_error)?,
                ),
            ),
        ];
        caches.push_inputs(&mut inputs);
        let keep_hidden_on_cuda = self.active_device == ActiveAudio8Device::Cuda && sequence == 1;
        let (logits, hidden, deltas) = {
            let mut binding = keep_hidden_on_cuda
                .then(|| self.slow.create_binding())
                .transpose()
                .map_err(ort_error)?;
            let mut outputs = if let Some(binding) = binding.as_mut() {
                for (name, input) in &inputs {
                    binding.bind_input(name, input).map_err(ort_error)?;
                }
                let cpu_output = MemoryInfo::new(
                    AllocationDevice::CPU,
                    0,
                    AllocatorType::Device,
                    MemoryType::CPUOutput,
                )
                .map_err(ort_error)?;
                let cuda_output = MemoryInfo::new(
                    AllocationDevice::CUDA,
                    0,
                    AllocatorType::Device,
                    MemoryType::Default,
                )
                .map_err(ort_error)?;
                for output in self.slow.outputs() {
                    let memory = if output.name() == "slow_hidden" {
                        &cuda_output
                    } else {
                        &cpu_output
                    };
                    binding
                        .bind_output_to_device(output.name(), memory)
                        .map_err(ort_error)?;
                }
                self.slow.run_binding(binding).map_err(ort_error)?
            } else {
                self.slow.run(inputs).map_err(ort_error)?
            };
            let (logits_shape, logits) = match outputs["logits"].try_extract_tensor::<f32>() {
                Ok((shape, values)) => (shape.as_ref().to_vec(), values.to_vec()),
                Err(_) => {
                    let (shape, values) = outputs["logits"]
                        .try_extract_tensor::<f16>()
                        .map_err(ort_error)?;
                    (
                        shape.as_ref().to_vec(),
                        values.iter().map(|value| value.to_f32()).collect(),
                    )
                }
            };
            let (hidden_shape, hidden) = if keep_hidden_on_cuda {
                match outputs["slow_hidden"].dtype() {
                    ValueType::Tensor { ty, shape, .. } if *ty == TensorElementType::Float16 => {
                        (shape.as_ref().to_vec(), None)
                    }
                    dtype => {
                        return Err(native_error(format!(
                            "unexpected Audio8 CUDA slow hidden type: {dtype}"
                        )));
                    }
                }
            } else {
                let (shape, values) = outputs["slow_hidden"]
                    .try_extract_tensor::<f16>()
                    .map_err(ort_error)?;
                (shape.as_ref().to_vec(), Some(values.to_vec()))
            };
            let deltas = extract_cache_deltas(
                &outputs,
                caches.values.len(),
                caches.heads,
                positions.len(),
                caches.dimension,
            )?;
            let valid_time = |time: i64| time == 1 || time == sequence as i64;
            if logits_shape.len() != 3
                || logits_shape[0] != 1
                || !valid_time(logits_shape[1])
                || logits_shape[2] != 4097
                || hidden_shape.len() != 3
                || hidden_shape[0] != 1
                || !valid_time(hidden_shape[1])
                || hidden_shape[2] != self.manifest.fast_dim as i64
            {
                return Err(native_error(format!(
                    "unexpected Audio8 slow AR output shape: logits={logits_shape:?}, hidden={hidden_shape:?}, input_sequence={sequence}"
                )));
            }
            let logits = logits[logits.len() - 4097..].to_vec();
            let hidden = if let Some(hidden) = hidden {
                SlowHidden::Host(hidden[hidden.len() - self.manifest.fast_dim..].to_vec())
            } else {
                SlowHidden::Cuda(
                    outputs
                        .remove("slow_hidden")
                        .expect("bound slow hidden output exists"),
                )
            };
            (logits, hidden, deltas)
        };
        ensure_finite_logits("slow AR", &logits)?;
        caches.update(positions, &deltas)?;
        Ok((logits, hidden))
    }

    fn fast_step(
        &mut self,
        hidden: &SlowHidden,
        token: i64,
        use_hidden: bool,
        position: usize,
        caches: &mut CacheSet,
    ) -> Result<Vec<f32>, InferenceError> {
        let host_hidden = match hidden {
            SlowHidden::Host(values) => Some(
                Array3::from_shape_vec((1, 1, self.manifest.fast_dim), values.clone())
                    .map_err(|error| native_error(error.to_string()))?,
            ),
            SlowHidden::Cuda(_) => None,
        };
        let token = Array2::from_shape_vec((1, 1), vec![token])
            .map_err(|error| native_error(error.to_string()))?;
        let use_hidden = Array1::from_vec(vec![use_hidden]);
        let input_pos = Array1::from_vec(vec![position as i64]);
        let hidden_input = match (hidden, host_hidden.as_ref()) {
            (SlowHidden::Host(_), Some(hidden)) => named_tensor("slow_hidden", hidden)?.1,
            (SlowHidden::Cuda(hidden), None) => hidden.into(),
            _ => unreachable!("host hidden allocation follows the hidden storage"),
        };
        let mut inputs = vec![
            ("slow_hidden".to_owned(), hidden_input),
            named_tensor("token_id", &token)?,
            named_tensor("use_slow_hidden", &use_hidden)?,
            named_tensor("input_pos", &input_pos)?,
        ];
        caches.push_inputs(&mut inputs);
        let (logits, deltas) = {
            let outputs = self.fast.run(inputs).map_err(ort_error)?;
            let (logits_shape, logits) = match outputs["logits"].try_extract_tensor::<f32>() {
                Ok((shape, values)) => (shape.as_ref().to_vec(), values.to_vec()),
                Err(_) => {
                    let (shape, values) = outputs["logits"]
                        .try_extract_tensor::<f16>()
                        .map_err(ort_error)?;
                    (
                        shape.as_ref().to_vec(),
                        values.iter().map(|value| value.to_f32()).collect(),
                    )
                }
            };
            if logits_shape.as_slice() != [1, 1, self.manifest.codebook_size as i64] {
                return Err(native_error("unexpected Audio8 fast AR output shape"));
            }
            let deltas = extract_cache_deltas(
                &outputs,
                caches.values.len(),
                caches.heads,
                1,
                caches.dimension,
            )?;
            (logits, deltas)
        };
        ensure_finite_logits("fast AR", &logits)?;
        caches.update(&[position as i64], &deltas)?;
        Ok(logits)
    }
}

enum SlowHidden {
    Host(Vec<f16>),
    /// A single-token hidden state produced by Slow AR and consumed directly
    /// by Fast AR on the same CUDA device through ORT device tensors.
    Cuda(DynValue),
}

fn named_tensor<'a, T, D>(
    name: &str,
    array: &'a ndarray::Array<T, D>,
) -> Result<(String, ort::session::SessionInputValue<'a>), InferenceError>
where
    T: PrimitiveTensorElementType + Clone + std::fmt::Debug + 'static,
    D: ndarray::Dimension + 'static,
{
    Ok((
        name.to_owned(),
        TensorRef::from_array_view(array).map_err(ort_error)?.into(),
    ))
}

struct CacheSet {
    values: Vec<CacheStorage>,
    heads: usize,
    positions: usize,
    dimension: usize,
}

enum CacheStorage {
    Pageable(Vec<f16>),
    /// Page-locked host memory keeps the current, quality-proven CPU scatter
    /// update while avoiding the expensive pageable staging copy before every
    /// CUDA token. A future fused cache-update graph can replace this storage
    /// without changing prompt/sampling semantics.
    CudaPinned(Tensor<f16>),
}

impl CacheStorage {
    fn as_mut_slice(&mut self) -> &mut [f16] {
        match self {
            Self::Pageable(values) => values,
            Self::CudaPinned(tensor) => tensor.extract_tensor_mut().1,
        }
    }
}

impl CacheSet {
    fn new(count: usize, heads: usize, positions: usize, dimension: usize) -> Self {
        Self {
            values: (0..count)
                .map(|_| CacheStorage::Pageable(vec![f16::ZERO; heads * positions * dimension]))
                .collect(),
            heads,
            positions,
            dimension,
        }
    }

    fn new_for_session(
        session: &Session,
        device: ActiveAudio8Device,
        count: usize,
        heads: usize,
        positions: usize,
        dimension: usize,
    ) -> Self {
        if device != ActiveAudio8Device::Cuda {
            return Self::new(count, heads, positions, dimension);
        }
        let memory_info = MemoryInfo::new(
            AllocationDevice::CUDA_PINNED,
            0,
            AllocatorType::Device,
            MemoryType::CPUInput,
        );
        let pinned = memory_info
            .and_then(|info| Allocator::new(session, info))
            .and_then(|allocator| {
                (0..count)
                    .map(|_| {
                        let mut tensor = Tensor::<f16>::new(
                            &allocator,
                            [1, heads as i64, positions as i64, dimension as i64],
                        )?;
                        tensor.extract_tensor_mut().1.fill(f16::ZERO);
                        Ok(CacheStorage::CudaPinned(tensor))
                    })
                    .collect::<Result<Vec<_>, ort::Error>>()
            });
        match pinned {
            Ok(values) => Self {
                values,
                heads,
                positions,
                dimension,
            },
            Err(error) => {
                tracing::warn!(
                    %error,
                    "CUDA pinned cache allocation unavailable; using pageable host cache"
                );
                Self::new(count, heads, positions, dimension)
            }
        }
    }

    fn clear(&mut self) {
        for cache in &mut self.values {
            cache.as_mut_slice().fill(f16::ZERO);
        }
    }

    fn push_inputs<'a>(&'a self, inputs: &mut Vec<(String, ort::session::SessionInputValue<'a>)>) {
        for (index, cache) in self.values.iter().enumerate() {
            let layer = index / 2;
            let kind = if index % 2 == 0 { "key" } else { "value" };
            let input = match cache {
                CacheStorage::Pageable(values) => TensorRef::from_array_view((
                    [1, self.heads, self.positions, self.dimension],
                    values.as_slice(),
                ))
                .expect("fixed cache shape matches its allocation")
                .into(),
                CacheStorage::CudaPinned(tensor) => tensor.into(),
            };
            inputs.push((format!("cache_{kind}_{layer}"), input));
        }
    }

    fn update(&mut self, positions: &[i64], deltas: &[Vec<f16>]) -> Result<(), InferenceError> {
        if deltas.len() != self.values.len() {
            return Err(native_error("ONNX cache delta count mismatch"));
        }
        for (cache, delta) in self.values.iter_mut().zip(deltas) {
            let cache = cache.as_mut_slice();
            let expected = self.heads * positions.len() * self.dimension;
            if delta.len() != expected {
                return Err(native_error("ONNX cache delta shape mismatch"));
            }
            for head in 0..self.heads {
                for (source_position, target_position) in positions.iter().enumerate() {
                    let target_position = usize::try_from(*target_position)
                        .map_err(|_| native_error("negative cache position"))?;
                    if target_position >= self.positions {
                        return Err(native_error("cache position exceeds model context"));
                    }
                    let source = (head * positions.len() + source_position) * self.dimension;
                    let target = (head * self.positions + target_position) * self.dimension;
                    cache[target..target + self.dimension]
                        .copy_from_slice(&delta[source..source + self.dimension]);
                }
            }
        }
        Ok(())
    }
}

fn extract_cache_deltas(
    outputs: &ort::session::SessionOutputs<'_>,
    count: usize,
    heads: usize,
    positions: usize,
    dimension: usize,
) -> Result<Vec<Vec<f16>>, InferenceError> {
    (0..count)
        .map(|index| {
            let layer = index / 2;
            let kind = if index % 2 == 0 { "key" } else { "value" };
            let name = format!("{kind}_delta_{layer}");
            let (shape, values) = outputs[name.as_str()]
                .try_extract_tensor::<f16>()
                .map_err(ort_error)?;
            if shape.as_ref() != [1, heads as i64, positions as i64, dimension as i64] {
                return Err(native_error(format!(
                    "unexpected Audio8 cache delta shape for {name}: {shape:?}"
                )));
            }
            Ok(values.to_vec())
        })
        .collect()
}

fn build_generator_sessions(
    slow_path: &Path,
    fast_path: &Path,
    requested: Audio8ExecutionDevice,
    threads: usize,
) -> Result<(Session, Session, ActiveAudio8Device), InferenceError> {
    let (mut sessions, active) = build_session_group(
        &[slow_path, fast_path],
        requested,
        threads,
        "audio8-generator",
    )?;
    let fast = sessions.pop().expect("two Audio8 sessions were requested");
    let slow = sessions.pop().expect("two Audio8 sessions were requested");
    Ok((slow, fast, active))
}

fn build_session(
    path: &Path,
    requested: Audio8ExecutionDevice,
    threads: usize,
) -> Result<Session, InferenceError> {
    build_onnx_session(path, requested, threads, "audio8").map(|(session, _active)| session)
}

fn ensure_finite_logits(stage: &str, logits: &[f32]) -> Result<(), InferenceError> {
    if logits.is_empty() || logits.iter().any(|value| !value.is_finite()) {
        return Err(native_error(format!(
            "Audio8 {stage} returned non-finite logits; update ONNX Runtime or use a compatible execution provider"
        )));
    }
    Ok(())
}

fn codec_execution_device(
    manifest_provider: Option<&str>,
    requested: Audio8ExecutionDevice,
) -> Audio8ExecutionDevice {
    match manifest_provider.map(str::trim) {
        Some(provider) if provider.eq_ignore_ascii_case("cpu") => Audio8ExecutionDevice::Cpu,
        _ => requested,
    }
}

fn read_manifest<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, InferenceError> {
    serde_json::from_slice(
        &std::fs::read(path)
            .map_err(|error| native_error(format!("cannot read {}: {error}", path.display())))?,
    )
    .map_err(|error| native_error(format!("invalid {}: {error}", path.display())))
}

fn resample_pcm16_to_f16(
    bytes: &[u8],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f16>, InferenceError> {
    Ok(resample_pcm16(bytes, source_rate, target_rate)?
        .into_iter()
        .map(f16::from_f32)
        .collect())
}

fn ort_error(error: ort::Error) -> InferenceError {
    native_error(error.to_string())
}

fn native_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.providers.audio8.onnx",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{initialize_onnx_runtime, preload_onnx_cuda_libraries, tts::audio::resample};

    #[test]
    fn resampling_is_bounded_and_produces_the_expected_duration() {
        let bytes = [0_i16, 100, -100, 200]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let output = resample_pcm16_to_f16(&bytes, 16_000, 44_100).unwrap();
        assert_eq!(output.len(), 12);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn profile_replacement_keeps_only_the_latest_clone() {
        let mut voices = HashMap::new();
        voices.insert(
            "microphone".to_owned(),
            VoiceProfile {
                reference_text: "old".into(),
                codes: vec![1; 20],
                frames: 2,
            },
        );
        voices.insert(
            "microphone".to_owned(),
            VoiceProfile {
                reference_text: "new".into(),
                codes: vec![2; 30],
                frames: 3,
            },
        );
        assert_eq!(voices.len(), 1);
        assert_eq!(voices["microphone"].reference_text, "new");
    }

    #[test]
    fn codec_provider_contract_overrides_the_accelerated_generator() {
        assert_eq!(
            codec_execution_device(Some("cpu"), Audio8ExecutionDevice::Cuda),
            Audio8ExecutionDevice::Cpu
        );
        assert_eq!(
            codec_execution_device(None, Audio8ExecutionDevice::Cuda),
            Audio8ExecutionDevice::Cuda
        );
    }

    #[test]
    fn sampling_rng_matches_numpy_default_rng_42() {
        let mut rng = NumpyPcg64::seed_42();
        for expected in [
            0.773_956_048_555_963_3,
            0.438_878_439_752_052_3,
            0.858_597_919_911_382_5,
        ] {
            assert_eq!(rng.next_f64(), expected);
        }
    }

    #[test]
    fn non_finite_model_output_is_rejected_before_sampling() {
        assert!(ensure_finite_logits("slow AR", &[1.0, -2.0]).is_ok());
        assert!(ensure_finite_logits("slow AR", &[f32::NAN]).is_err());
        assert!(ensure_finite_logits("fast AR", &[f32::INFINITY]).is_err());
    }

    #[tokio::test]
    async fn cloned_profile_is_visible_to_replacement_sessions() {
        let mut voices = HashMap::new();
        voices.insert(
            "microphone".to_owned(),
            VoiceProfile {
                reference_text: "shared".into(),
                codes: vec![1; 20],
                frames: 2,
            },
        );
        let adapter = Audio8OnnxAdapter {
            state: Arc::new(Mutex::new(Audio8State {
                model_dir: PathBuf::new(),
                device: Audio8ExecutionDevice::Cpu,
                threads: 1,
                synthesis: Audio8SynthesisOptions::default(),
                voices,
                online: None,
            })),
        };

        let replacement_session = adapter.clone();
        assert!(replacement_session.has_voice("microphone").await);
        assert!(!replacement_session.has_voice("system_audio").await);
    }

    #[tokio::test]
    #[ignore = "requires the optional Audio8 FP16 model package"]
    async fn installed_runtime_prepares_both_ar_sessions_on_cuda() {
        let core = std::env::var_os("XRTRANSLATE_ORT_DYLIB_PATH")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_ORT_DYLIB_PATH");
        let libraries = std::env::var_os("XRTRANSLATE_AUDIO8_CUDA_PRELOAD")
            .expect("set XRTRANSLATE_AUDIO8_CUDA_PRELOAD");
        preload_onnx_cuda_libraries(&std::env::split_paths(&libraries).collect::<Vec<_>>())
            .unwrap();
        initialize_onnx_runtime(&core).unwrap();
        let model_dir = std::env::var_os("XRTRANSLATE_AUDIO8_MODEL_DIR")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_AUDIO8_MODEL_DIR");
        let (_, _, active) = build_generator_sessions(
            &model_dir.join("slow_ar_fp16.onnx"),
            &model_dir.join("fast_ar_fp16.onnx"),
            Audio8ExecutionDevice::Cuda,
            1,
        )
        .unwrap();
        assert_eq!(active, ActiveAudio8Device::Cuda);
    }

    #[test]
    #[ignore = "requires a packaged ONNX Runtime core and smoke-test model"]
    fn managed_runtime_core_loads_a_cpu_session_without_cuda_dependencies() {
        let core = std::env::var_os("XRTRANSLATE_ORT_DYLIB_PATH")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_ORT_DYLIB_PATH");
        initialize_onnx_runtime(&core).unwrap();
        let model = std::env::var_os("XRTRANSLATE_ORT_CPU_SMOKE_MODEL")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_ORT_CPU_SMOKE_MODEL");
        build_onnx_session(&model, Audio8ExecutionDevice::Cpu, 1, "cpu-smoke").unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the optional Audio8 FP16 model package"]
    async fn installed_model_registers_and_synthesizes_without_python() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(
                if std::env::var_os("XRTRANSLATE_AUDIO8_REQUIRE_CUDA").is_some() {
                    tracing::Level::TRACE
                } else {
                    tracing::Level::ERROR
                },
            )
            .with_test_writer()
            .try_init();
        let model_dir = std::env::var_os("XRTRANSLATE_AUDIO8_MODEL_DIR")
            .map(PathBuf::from)
            .expect("set XRTRANSLATE_AUDIO8_MODEL_DIR");
        if let Some(libraries) = std::env::var_os("XRTRANSLATE_AUDIO8_CUDA_PRELOAD") {
            preload_onnx_cuda_libraries(&std::env::split_paths(&libraries).collect::<Vec<_>>())
                .unwrap();
        }
        if let Some(core) = std::env::var_os("XRTRANSLATE_ORT_DYLIB_PATH") {
            initialize_onnx_runtime(Path::new(&core)).unwrap();
        }
        let wav = std::env::var_os("XRTRANSLATE_AUDIO8_REFERENCE_WAV")
            .map(std::fs::read)
            .transpose()
            .unwrap()
            .unwrap_or_else(|| {
                let samples = (0..16_000)
                    .map(|index| {
                        let phase = index as f32 * 220.0 * std::f32::consts::TAU / 16_000.0;
                        (phase.sin() * 4_000.0) as i16
                    })
                    .flat_map(i16::to_le_bytes)
                    .collect::<Vec<_>>();
                crate::pcm16_mono_16khz_to_wav(&samples).unwrap()
            });
        let transcript = std::env::var("XRTRANSLATE_AUDIO8_REFERENCE_TEXT")
            .unwrap_or_else(|_| "This is a native runtime test.".into());
        let target = std::env::var("XRTRANSLATE_AUDIO8_TARGET_TEXT")
            .unwrap_or_else(|_| "Native ONNX speech works.".into());
        let mut synthesis = Audio8SynthesisOptions::default();
        if let Ok(value) = std::env::var("XRTRANSLATE_AUDIO8_TEMPERATURE") {
            synthesis.temperature = value.parse().unwrap();
        }
        if let Ok(value) = std::env::var("XRTRANSLATE_AUDIO8_TOP_P") {
            synthesis.top_p = value.parse().unwrap();
        }
        if let Ok(value) = std::env::var("XRTRANSLATE_AUDIO8_MAX_NEW_TOKENS") {
            synthesis.max_new_tokens = value.parse().unwrap();
        }
        if let Ok(value) = std::env::var("XRTRANSLATE_AUDIO8_TOP_K") {
            synthesis.top_k = value.parse().unwrap();
        }
        let device = match std::env::var("XRTRANSLATE_AUDIO8_DEVICE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "cuda" => Audio8ExecutionDevice::Cuda,
            "cpu" => Audio8ExecutionDevice::Cpu,
            _ => Audio8ExecutionDevice::Auto,
        };
        let adapter =
            Audio8OnnxAdapter::with_synthesis_options(model_dir, device, 5, synthesis).unwrap();
        if let Some(path) = std::env::var_os("XRTRANSLATE_AUDIO8_PROFILE_JSON") {
            let profile: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let rows = profile["codes"].as_array().unwrap();
            let frames = rows[0].as_array().unwrap().len();
            let codes = rows
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .map(|code| code.as_u64().unwrap() as u16)
                .collect::<Vec<_>>();
            adapter.state.lock().unwrap().voices.insert(
                "smoke".into(),
                VoiceProfile {
                    reference_text: profile["reference_text"].as_str().unwrap().into(),
                    codes,
                    frames,
                },
            );
            adapter.prepare().await.unwrap();
        } else {
            adapter
                .register_voice("smoke", wav, &transcript)
                .await
                .unwrap();
        }
        let prepared = adapter
            .active_device()
            .await
            .expect("registration or explicit preparation loads the runtime");
        eprintln!("active_tts_device={prepared:?}");
        if std::env::var_os("XRTRANSLATE_AUDIO8_REQUIRE_CUDA").is_some() {
            assert_eq!(prepared, Audio8ExecutionDevice::Cuda);
        }
        assert!(matches!(
            prepared,
            Audio8ExecutionDevice::Cuda | Audio8ExecutionDevice::Cpu
        ));
        assert_eq!(adapter.prepare().await.unwrap(), prepared);
        if let Some(path) = std::env::var_os("XRTRANSLATE_AUDIO8_ROUNDTRIP_WAV") {
            let roundtrip = {
                let state = adapter.state.lock().unwrap();
                let profile = state.voices["smoke"].clone();
                let mut runtime = OnlineRuntime::load(
                    &state.model_dir,
                    Audio8ExecutionDevice::Cpu,
                    state.threads,
                )
                .unwrap();
                runtime
                    .decode_codebook_major(
                        profile.codes.into_iter().map(i64::from).collect(),
                        profile.frames,
                    )
                    .unwrap()
            };
            write_pcm16_wav(Path::new(&path), &roundtrip.bytes, roundtrip.sample_rate).unwrap();
        }
        let audio = adapter.synthesize(&target, "smoke").await.unwrap();
        eprintln!(
            "generated_frames={} output_seconds={:.3}",
            audio.bytes.len() / 2 / 2048,
            audio.bytes.len() as f64 / 2.0 / f64::from(audio.sample_rate)
        );
        assert_eq!(audio.sample_rate, 44_100);
        assert!(audio.bytes.len() > 44_100);
        assert!(
            audio
                .bytes
                .chunks_exact(2)
                .any(|sample| { i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() > 128 })
        );
        if let Some(path) = std::env::var_os("XRTRANSLATE_AUDIO8_OUTPUT_WAV") {
            write_pcm16_wav(Path::new(&path), &audio.bytes, audio.sample_rate).unwrap();
        }
        if let Ok(endpoint) = std::env::var("XRTRANSLATE_AUDIO8_ASR_ENDPOINT") {
            let source = audio
                .bytes
                .chunks_exact(2)
                .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0)
                .collect::<Vec<_>>();
            let pcm = resample(&source, audio.sample_rate, 16_000)
                .unwrap()
                .into_iter()
                .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                .collect::<Vec<_>>();
            let asr = crate::Qwen3AsrAdapter::new(
                crate::ReqwestClient::with_default_direct_timeout().unwrap(),
                endpoint,
                "qwen3-asr",
            )
            .unwrap();
            let transcript = asr
                .transcribe_pcm16(
                    &pcm,
                    crate::Qwen3AsrOptions {
                        language: Some("Chinese".into()),
                        context_bias: None,
                        vocabulary_bias: Vec::new(),
                        instruction_prompt: None,
                        max_tokens: 128,
                    },
                )
                .await
                .unwrap();
            eprintln!("generated_asr={}", transcript.text);
        }
    }

    fn write_pcm16_wav(path: &Path, pcm: &[u8], sample_rate: u32) -> std::io::Result<()> {
        let mut wav = Vec::with_capacity(44 + pcm.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36_u32 + pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(pcm);
        std::fs::write(path, wav)
    }
}
