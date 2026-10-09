//! Native speaker embeddings and deterministic online speaker tracking.
//!
//! The ONNX contract follows 3D-Speaker's official ERes2NetV2 exporter:
//! `feature` is `[batch, frames, 80]` Kaldi-compatible log filterbanks and
//! `embedding` is `[batch, 192]`.  Audio preprocessing and clustering live in
//! this crate so the latency-sensitive backend does not need a Python sidecar.

#![forbid(unsafe_code)]

mod stability;

use std::{
    error::Error,
    f32::consts::PI,
    fmt,
    path::Path,
    time::{Duration, Instant},
};

use ndarray::Array3;
use ort::{session::Session, value::Value};
use stability::{StableIdentityDecision, StableIdentityTracker};

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const FEATURE_DIMENSIONS: usize = 80;
pub const EMBEDDING_DIMENSIONS: usize = 192;
const FRAME_SAMPLES: usize = 400;
const FRAME_SHIFT_SAMPLES: usize = 160;
const FFT_SIZE: usize = 512;
const FFT_BINS: usize = FFT_SIZE / 2;

#[derive(Debug)]
pub enum SpeakerError {
    Ort(ort::Error),
    EmptyEmbedding,
    NonFiniteEmbedding,
    InvalidModelOutput,
    InvalidTrackerConfig(&'static str),
}

impl fmt::Display for SpeakerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ort(error) => write!(formatter, "ONNX Runtime speaker inference failed: {error}"),
            Self::EmptyEmbedding => {
                formatter.write_str("speaker model returned an empty embedding")
            }
            Self::NonFiniteEmbedding => {
                formatter.write_str("speaker model returned a non-finite embedding")
            }
            Self::InvalidModelOutput => {
                formatter.write_str("speaker model output is not a valid float tensor")
            }
            Self::InvalidTrackerConfig(message) => write!(
                formatter,
                "invalid speaker tracker configuration: {message}"
            ),
        }
    }
}

impl Error for SpeakerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ort(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ort::Error> for SpeakerError {
    fn from(value: ort::Error) -> Self {
        Self::Ort(value)
    }
}

/// Stateless ERes2NetV2 ONNX inference. Keep one instance on one worker task.
#[derive(Debug)]
pub struct SpeakerEmbeddingModel {
    session: Session,
    fbank: FbankExtractor,
}

impl SpeakerEmbeddingModel {
    pub fn from_file(
        model_path: impl AsRef<Path>,
        intra_threads: usize,
    ) -> Result<Self, SpeakerError> {
        let session = Session::builder()?
            .with_intra_threads(intra_threads.max(1))
            .map_err(|error| SpeakerError::Ort(ort::Error::new(error.to_string())))?
            .with_inter_threads(1)
            .map_err(|error| SpeakerError::Ort(ort::Error::new(error.to_string())))?
            .commit_from_file(model_path)?;
        Ok(Self {
            session,
            fbank: FbankExtractor::new(),
        })
    }

    /// Extracts one L2-normalized embedding from mono PCM16 at 16 kHz.
    pub fn extract(&mut self, samples: &[i16]) -> Result<Vec<f32>, SpeakerError> {
        let features = self.fbank.compute(samples);
        let frames = features.len() / FEATURE_DIMENSIONS;
        let feature = Array3::from_shape_vec((1, frames, FEATURE_DIMENSIONS), features)
            .expect("fbank output always has a complete final dimension");
        let feature = Value::from_array(feature)?;
        let outputs = self.session.run(ort::inputs!["feature" => feature])?;
        let embedding = outputs
            .get("embedding")
            .ok_or(SpeakerError::InvalidModelOutput)?;
        let (_, values) = embedding
            .try_extract_tensor::<f32>()
            .map_err(|_| SpeakerError::InvalidModelOutput)?;
        if values.len() != EMBEDDING_DIMENSIONS {
            return Err(SpeakerError::InvalidModelOutput);
        }
        normalize_embedding(values.to_vec())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackerConfig {
    pub similarity_threshold: f32,
    pub same_speaker_hysteresis: f32,
    /// Minimum cosine advantage required before switching away from the
    /// immediately previous speaker while that speaker remains plausible.
    pub speaker_switch_margin: f32,
    pub max_speakers: usize,
}

impl TrackerConfig {
    pub fn validate(self) -> Result<Self, SpeakerError> {
        if !self.similarity_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.similarity_threshold)
        {
            return Err(SpeakerError::InvalidTrackerConfig(
                "similarity_threshold must be finite and within 0..=1",
            ));
        }
        if !self.same_speaker_hysteresis.is_finite()
            || !(0.0..=0.25).contains(&self.same_speaker_hysteresis)
        {
            return Err(SpeakerError::InvalidTrackerConfig(
                "same_speaker_hysteresis must be finite and within 0..=0.25",
            ));
        }
        if !self.speaker_switch_margin.is_finite()
            || !(0.0..=0.25).contains(&self.speaker_switch_margin)
        {
            return Err(SpeakerError::InvalidTrackerConfig(
                "speaker_switch_margin must be finite and within 0..=0.25",
            ));
        }
        if self.max_speakers == 0 || self.max_speakers > 64 {
            return Err(SpeakerError::InvalidTrackerConfig(
                "max_speakers must be within 1..=64",
            ));
        }
        Ok(self)
    }
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.56,
            same_speaker_hysteresis: 0.14,
            speaker_switch_margin: 0.04,
            max_speakers: 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerAssignment {
    pub speaker_id: String,
    pub similarity: f32,
    pub is_new: bool,
}

#[derive(Debug)]
struct SpeakerObservation {
    embedding: Vec<f32>,
    matched_index: Option<usize>,
    best_index: Option<usize>,
    similarity: f32,
}

fn speaker_id(index: usize) -> String {
    format!("speaker-{:02}", index + 1)
}

#[derive(Debug)]
struct SpeakerCentroid {
    embedding: Vec<f32>,
    observations: u32,
}

/// Arrival-ordered, memory-bounded online cosine clustering.
///
/// IDs never change inside an audio generation. A small hysteresis prevents
/// adjacent chunks from the same speaker from oscillating near the threshold.
#[derive(Debug)]
pub struct OnlineSpeakerTracker {
    config: TrackerConfig,
    centroids: Vec<SpeakerCentroid>,
    previous_speaker: Option<usize>,
}

impl OnlineSpeakerTracker {
    pub fn new(config: TrackerConfig) -> Result<Self, SpeakerError> {
        Ok(Self {
            config: config.validate()?,
            centroids: Vec::with_capacity(config.max_speakers),
            previous_speaker: None,
        })
    }

    pub fn reset(&mut self) {
        self.centroids.clear();
        self.previous_speaker = None;
    }

    pub fn speaker_count(&self) -> usize {
        self.centroids.len()
    }

    pub fn assign(&mut self, embedding: &[f32]) -> Result<SpeakerAssignment, SpeakerError> {
        let observation = self.observe(embedding, true)?;
        self.commit(observation)
    }

    fn observe(
        &self,
        embedding: &[f32],
        prefer_previous: bool,
    ) -> Result<SpeakerObservation, SpeakerError> {
        let embedding = normalize_embedding(embedding.to_vec())?;
        let mut best = None;
        let mut previous_similarity = None;
        for (index, centroid) in self.centroids.iter().enumerate() {
            let similarity = cosine(&embedding, &centroid.embedding);
            if self.previous_speaker == Some(index) {
                previous_similarity = Some(similarity);
            }
            if best.is_none_or(|(_, best_similarity)| similarity > best_similarity) {
                best = Some((index, similarity));
            }
        }

        // A nearest-centroid decision is unstable when two profiles score
        // almost equally. Preserve temporal continuity while the previous
        // speaker is still above its hysteresis threshold, and switch only
        // when another profile has a meaningful cosine advantage.
        let preferred = match (
            prefer_previous.then_some(self.previous_speaker).flatten(),
            best,
        ) {
            (Some(previous_index), Some((best_index, best_similarity)))
                if best_index != previous_index =>
            {
                let previous_similarity = previous_similarity.unwrap_or(f32::NEG_INFINITY);
                let continuation_threshold =
                    self.config.similarity_threshold - self.config.same_speaker_hysteresis;
                if previous_similarity >= continuation_threshold
                    && best_similarity - previous_similarity < self.config.speaker_switch_margin
                {
                    Some((previous_index, previous_similarity))
                } else {
                    Some((best_index, best_similarity))
                }
            }
            _ => best,
        };

        let accepted = preferred.filter(|(index, similarity)| {
            let threshold = if prefer_previous && self.previous_speaker == Some(*index) {
                self.config.similarity_threshold - self.config.same_speaker_hysteresis
            } else {
                self.config.similarity_threshold
            };
            *similarity >= threshold
        });

        Ok(SpeakerObservation {
            embedding,
            matched_index: accepted.map(|(index, _)| index),
            best_index: best.map(|(index, _)| index),
            similarity: accepted.or(best).map_or(0.0, |(_, similarity)| similarity),
        })
    }

    fn commit(
        &mut self,
        observation: SpeakerObservation,
    ) -> Result<SpeakerAssignment, SpeakerError> {
        let (index, similarity, is_new) = if let Some(index) = observation.matched_index {
            self.update_centroid(index, &observation.embedding)?;
            (index, observation.similarity, false)
        } else if self.centroids.len() < self.config.max_speakers {
            let index = self.centroids.len();
            self.centroids.push(SpeakerCentroid {
                embedding: observation.embedding,
                observations: 1,
            });
            (index, 1.0, true)
        } else if let Some(index) = observation.best_index {
            // The configured memory bound is strict. Once full, choose the
            // nearest known speaker but do not contaminate its centroid with a
            // below-threshold observation.
            (index, observation.similarity, false)
        } else {
            return Err(SpeakerError::EmptyEmbedding);
        };
        self.previous_speaker = Some(index);
        Ok(SpeakerAssignment {
            speaker_id: speaker_id(index),
            similarity,
            is_new,
        })
    }

    fn is_full(&self) -> bool {
        self.centroids.len() >= self.config.max_speakers
    }

    fn update_centroid(&mut self, index: usize, embedding: &[f32]) -> Result<(), SpeakerError> {
        let centroid = &mut self.centroids[index];
        // Cap historical weight so a speaker profile can slowly adapt to a
        // changed microphone or room without reacting to one noisy segment.
        let history = centroid.observations.min(19) as f32;
        let incoming_weight = 1.0 / (history + 1.0);
        for (mean, value) in centroid.embedding.iter_mut().zip(embedding) {
            *mean = *mean * (1.0 - incoming_weight) + *value * incoming_weight;
        }
        centroid.embedding = normalize_embedding(std::mem::take(&mut centroid.embedding))?;
        centroid.observations = centroid.observations.saturating_add(1);
        Ok(())
    }
}

/// Combines one model session with its generation-local online tracker.
#[derive(Debug)]
pub struct OnlineSpeakerDiarizer {
    model: SpeakerEmbeddingModel,
    tracker: OnlineSpeakerTracker,
}

impl OnlineSpeakerDiarizer {
    pub fn from_file(
        model_path: impl AsRef<Path>,
        intra_threads: usize,
        tracker: TrackerConfig,
    ) -> Result<Self, SpeakerError> {
        Ok(Self {
            model: SpeakerEmbeddingModel::from_file(model_path, intra_threads)?,
            tracker: OnlineSpeakerTracker::new(tracker)?,
        })
    }

    pub fn identify(&mut self, samples: &[i16]) -> Result<SpeakerAssignment, SpeakerError> {
        let embedding = self.model.extract(samples)?;
        self.tracker.assign(&embedding)
    }

    pub fn tracker(&self) -> &OnlineSpeakerTracker {
        &self.tracker
    }

    pub fn tracker_mut(&mut self) -> &mut OnlineSpeakerTracker {
        &mut self.tracker
    }

    pub fn model_mut(&mut self) -> &mut SpeakerEmbeddingModel {
        &mut self.model
    }

    pub fn reset(&mut self) {
        self.tracker.reset();
    }
}

/// Dynamic rate limiter that regulates speaker inference frequency to prevent latency accumulation.
#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveSpeakerThrottle {
    min_step_samples: usize,
    max_step_samples: usize,
    current_step_samples: usize,
    avg_inference_duration: Duration,
    target_duty_cycle: f32,
    healthy_cycles: usize,
    circuit_broken: bool,
}

impl AdaptiveSpeakerThrottle {
    pub const DEFAULT_MIN_STEP_MS: u32 = 300;
    pub const DEFAULT_MAX_STEP_MS: u32 = 1000;
    pub const DEFAULT_TARGET_DUTY_CYCLE: f32 = 0.30;
    pub const CIRCUIT_BREAKER_THRESHOLD_MS: u64 = 350;

    pub fn new(sample_rate_hz: u32) -> Self {
        let min_step =
            (sample_rate_hz as f64 * (Self::DEFAULT_MIN_STEP_MS as f64 / 1000.0)) as usize;
        let max_step =
            (sample_rate_hz as f64 * (Self::DEFAULT_MAX_STEP_MS as f64 / 1000.0)) as usize;
        Self {
            min_step_samples: min_step.max(1),
            max_step_samples: max_step.max(min_step),
            current_step_samples: min_step.max(1),
            avg_inference_duration: Duration::from_millis(10),
            target_duty_cycle: Self::DEFAULT_TARGET_DUTY_CYCLE,
            healthy_cycles: 0,
            circuit_broken: false,
        }
    }

    pub fn with_limits(
        min_step_samples: usize,
        max_step_samples: usize,
        target_duty_cycle: f32,
    ) -> Self {
        let min_step = min_step_samples.max(1);
        let max_step = max_step_samples.max(min_step);
        Self {
            min_step_samples: min_step,
            max_step_samples: max_step,
            current_step_samples: min_step,
            avg_inference_duration: Duration::from_millis(10),
            target_duty_cycle: target_duty_cycle.clamp(0.05, 0.90),
            healthy_cycles: 0,
            circuit_broken: false,
        }
    }

    pub fn current_step_samples(&self) -> usize {
        self.current_step_samples
    }

    pub fn avg_inference_duration(&self) -> Duration {
        self.avg_inference_duration
    }

    pub fn is_circuit_broken(&self) -> bool {
        self.circuit_broken
    }

    pub fn record_inference_duration(&mut self, elapsed: Duration) {
        let elapsed_secs = elapsed.as_secs_f32();
        let old_avg = self.avg_inference_duration.as_secs_f32();
        let new_avg = old_avg * 0.75 + elapsed_secs * 0.25;
        self.avg_inference_duration = Duration::from_secs_f32(new_avg);

        if elapsed.as_millis() > Self::CIRCUIT_BREAKER_THRESHOLD_MS as u128
            && self.avg_inference_duration.as_millis() > Self::CIRCUIT_BREAKER_THRESHOLD_MS as u128
        {
            self.circuit_broken = true;
            return;
        } else {
            self.circuit_broken = false;
        }

        // T_ideal = T_infer / target_duty_cycle
        let ideal_interval_secs = new_avg / self.target_duty_cycle;
        let ideal_step_samples = (ideal_interval_secs * (SAMPLE_RATE_HZ as f32)) as usize;

        if ideal_step_samples > self.current_step_samples {
            // High load: immediate backoff
            self.current_step_samples = ideal_step_samples
                .min(self.max_step_samples)
                .max(self.min_step_samples);
            self.healthy_cycles = 0;
        } else {
            // Healthy load: gradual recovery
            self.healthy_cycles = self.healthy_cycles.saturating_add(1);
            if self.healthy_cycles >= 4 && self.current_step_samples > self.min_step_samples {
                let step_reduction = (SAMPLE_RATE_HZ as f32 * 0.05) as usize; // 50ms reduction
                self.current_step_samples = self
                    .current_step_samples
                    .saturating_sub(step_reduction)
                    .max(self.min_step_samples);
                self.healthy_cycles = 0;
            }
        }
    }

    pub fn reset(&mut self) {
        self.current_step_samples = self.min_step_samples;
        self.healthy_cycles = 0;
        self.circuit_broken = false;
    }
}

/// Configuration for streaming speaker segmenter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamingDiarizerConfig {
    pub tracker: TrackerConfig,
    /// Analysis window duration in samples (e.g. 800ms = 12800 samples at 16kHz)
    pub window_samples: usize,
    /// Minimum initial speech samples before evaluating speaker identity (e.g. 500ms = 8000 samples)
    pub min_speech_samples: usize,
    /// Minimum coherent observations required to trigger a cut. Confirmation
    /// also requires evidence spanning most of an analysis window.
    pub consecutive_confirmations: usize,
    /// Target duty cycle for the adaptive throttle
    pub target_duty_cycle: f32,
}

impl Default for StreamingDiarizerConfig {
    fn default() -> Self {
        Self {
            tracker: TrackerConfig::default(),
            window_samples: (SAMPLE_RATE_HZ as f32 * 0.8) as usize, // 800ms
            min_speech_samples: (SAMPLE_RATE_HZ as f32 * 0.5) as usize, // 500ms
            consecutive_confirmations: 2,
            target_duty_cycle: 0.30,
        }
    }
}

/// Event emitted by [`StreamingSpeakerSegmenter`].
#[derive(Clone, Debug, PartialEq)]
pub enum StreamingSpeakerEvent {
    /// Speech continues with currently assigned speaker.
    Continues { speaker_id: String },
    /// A speaker change was confirmed from temporally separated observations.
    SpeakerCut {
        previous_speaker: String,
        new_speaker: String,
    },
}

/// Online streaming speaker segmenter that runs during active speech turns,
/// monitors speaker identity via sliding window, and triggers forced utterance
/// segmentation on detected speaker changes.
#[derive(Debug)]
pub struct StreamingSpeakerSegmenter {
    diarizer: OnlineSpeakerDiarizer,
    throttle: AdaptiveSpeakerThrottle,
    config: StreamingDiarizerConfig,
    active_samples: Vec<i16>,
    identity: StableIdentityTracker,
    samples_since_last_evaluation: usize,
    turn_speech_samples: usize,
    speech_clock: usize,
    silence_samples: usize,
}

impl StreamingSpeakerSegmenter {
    pub fn from_diarizer(diarizer: OnlineSpeakerDiarizer, config: StreamingDiarizerConfig) -> Self {
        let throttle = AdaptiveSpeakerThrottle::with_limits(
            (SAMPLE_RATE_HZ as f32 * 0.3) as usize,
            (SAMPLE_RATE_HZ as f32 * 1.0) as usize,
            config.target_duty_cycle,
        );
        Self {
            diarizer,
            throttle,
            config,
            active_samples: Vec::with_capacity(config.window_samples * 2),
            identity: StableIdentityTracker::default(),
            samples_since_last_evaluation: 0,
            turn_speech_samples: 0,
            speech_clock: 0,
            silence_samples: usize::MAX,
        }
    }

    pub fn from_file(
        model_path: impl AsRef<Path>,
        intra_threads: usize,
        config: StreamingDiarizerConfig,
    ) -> Result<Self, SpeakerError> {
        let diarizer = OnlineSpeakerDiarizer::from_file(model_path, intra_threads, config.tracker)?;
        Ok(Self::from_diarizer(diarizer, config))
    }

    pub fn throttle(&self) -> &AdaptiveSpeakerThrottle {
        &self.throttle
    }

    pub fn throttle_mut(&mut self) -> &mut AdaptiveSpeakerThrottle {
        &mut self.throttle
    }

    pub fn current_speaker_id(&self) -> Option<&str> {
        self.identity.current.as_deref()
    }

    pub fn active_sample_count(&self) -> usize {
        self.active_samples.len()
    }

    /// Feed active speech samples into the streaming segmenter.
    pub fn push_speech_samples(
        &mut self,
        samples: &[i16],
    ) -> Result<Option<StreamingSpeakerEvent>, SpeakerError> {
        // A zero/very short gap is produced by technical hard splits and can
        // safely inherit the active identity. A real VAD pause starts a fresh
        // identity decision, while coherent provisional evidence may survive
        // a normal conversational gap.
        const IDENTITY_CONTINUITY_GAP_SAMPLES: usize = SAMPLE_RATE_HZ as usize / 16;
        const CANDIDATE_CONTINUITY_GAP_SAMPLES: usize = SAMPLE_RATE_HZ as usize * 3 / 2;
        if self.turn_speech_samples == 0 {
            self.identity.begin_turn(
                self.silence_samples <= IDENTITY_CONTINUITY_GAP_SAMPLES,
                self.silence_samples <= CANDIDATE_CONTINUITY_GAP_SAMPLES,
            );
        }
        self.active_samples.extend_from_slice(samples);
        self.samples_since_last_evaluation = self
            .samples_since_last_evaluation
            .saturating_add(samples.len());
        self.turn_speech_samples = self.turn_speech_samples.saturating_add(samples.len());
        self.speech_clock = self.speech_clock.saturating_add(samples.len());
        self.silence_samples = 0;

        // Stop further model work under extreme load. A turn without enough
        // trusted evidence is reported as unknown instead of guessing an ID.
        if self.throttle.is_circuit_broken() {
            return Ok(self
                .identity
                .current
                .as_ref()
                .map(|id| StreamingSpeakerEvent::Continues {
                    speaker_id: id.clone(),
                }));
        }

        let step_threshold = self.throttle.current_step_samples();
        if self.active_samples.len() < self.config.min_speech_samples
            || self.samples_since_last_evaluation < step_threshold
        {
            return Ok(None);
        }

        self.samples_since_last_evaluation = 0;

        let window_len = self.config.window_samples.min(self.active_samples.len());
        let window_start = self.active_samples.len().saturating_sub(window_len);
        let window = &self.active_samples[window_start..];

        let start_time = Instant::now();
        let embedding = self.diarizer.model_mut().extract(window)?;
        let elapsed = start_time.elapsed();
        self.throttle.record_inference_duration(elapsed);
        let decision = self.identity.observe(
            self.diarizer.tracker_mut(),
            &embedding,
            self.speech_clock,
            self.config.window_samples,
            self.config.consecutive_confirmations,
        )?;
        Ok(match decision {
            StableIdentityDecision::Pending => None,
            StableIdentityDecision::Continues(speaker_id) => {
                Some(StreamingSpeakerEvent::Continues { speaker_id })
            }
            StableIdentityDecision::Switch { previous, new } => {
                if self.active_samples.len() > self.config.window_samples {
                    let retain_from = self.active_samples.len() - self.config.window_samples;
                    self.active_samples.drain(..retain_from);
                }
                self.samples_since_last_evaluation = 0;
                Some(StreamingSpeakerEvent::SpeakerCut {
                    previous_speaker: previous,
                    new_speaker: new,
                })
            }
        })
    }

    pub fn observe_silence(&mut self, samples: usize) {
        self.silence_samples = self.silence_samples.saturating_add(samples);
    }

    /// Completes a speech turn without inventing an identity for insufficient evidence.
    pub fn finalize_speech(&mut self) -> Option<String> {
        let speaker_id = self.identity.finish_turn();
        self.active_samples.clear();
        self.samples_since_last_evaluation = 0;
        self.turn_speech_samples = 0;
        speaker_id
    }

    pub fn reset(&mut self) {
        self.active_samples.clear();
        self.samples_since_last_evaluation = 0;
        self.turn_speech_samples = 0;
        self.speech_clock = 0;
        self.silence_samples = usize::MAX;
        self.identity.reset();
        self.diarizer.reset();
        self.throttle.reset();
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f32,
    im: f32,
}

#[derive(Debug)]
struct FbankExtractor {
    window: [f32; FRAME_SAMPLES],
    mel_filters: Vec<Vec<(usize, f32)>>,
}

impl FbankExtractor {
    fn new() -> Self {
        let window = std::array::from_fn(|index| {
            let phase = 2.0 * PI * index as f32 / (FRAME_SAMPLES - 1) as f32;
            (0.5 - 0.5 * phase.cos()).powf(0.85)
        });
        let mel_low = mel_scale(20.0);
        let mel_high = mel_scale(SAMPLE_RATE_HZ as f32 / 2.0);
        let delta = (mel_high - mel_low) / (FEATURE_DIMENSIONS + 1) as f32;
        let bin_width = SAMPLE_RATE_HZ as f32 / FFT_SIZE as f32;
        let mel_filters = (0..FEATURE_DIMENSIONS)
            .map(|band| {
                let left = mel_low + band as f32 * delta;
                let middle = left + delta;
                let right = middle + delta;
                (0..FFT_BINS)
                    .filter_map(|bin| {
                        let mel = mel_scale(bin as f32 * bin_width);
                        let weight = if mel > left && mel <= middle {
                            (mel - left) / (middle - left)
                        } else if mel > middle && mel < right {
                            (right - mel) / (right - middle)
                        } else {
                            0.0
                        };
                        (weight > 0.0).then_some((bin, weight))
                    })
                    .collect()
            })
            .collect();
        Self {
            window,
            mel_filters,
        }
    }

    fn compute(&self, samples: &[i16]) -> Vec<f32> {
        let mut normalized = samples
            .iter()
            .map(|sample| f32::from(*sample) / 32_768.0)
            .collect::<Vec<_>>();
        if normalized.len() < FRAME_SAMPLES {
            normalized.resize(FRAME_SAMPLES, 0.0);
        }
        let frames = 1 + (normalized.len() - FRAME_SAMPLES) / FRAME_SHIFT_SAMPLES;
        let mut features = Vec::with_capacity(frames * FEATURE_DIMENSIONS);
        let mut fft = [Complex::default(); FFT_SIZE];
        for frame_index in 0..frames {
            let start = frame_index * FRAME_SHIFT_SAMPLES;
            let frame = &normalized[start..start + FRAME_SAMPLES];
            let mean = frame.iter().sum::<f32>() / FRAME_SAMPLES as f32;
            let mut previous = frame[0] - mean;
            for index in 0..FRAME_SAMPLES {
                let centered = frame[index] - mean;
                let emphasized = if index == 0 {
                    centered - 0.97 * centered
                } else {
                    centered - 0.97 * previous
                };
                previous = centered;
                fft[index] = Complex {
                    re: emphasized * self.window[index],
                    im: 0.0,
                };
            }
            fft[FRAME_SAMPLES..].fill(Complex::default());
            fft_in_place(&mut fft);
            let power = std::array::from_fn::<_, FFT_BINS, _>(|bin| {
                fft[bin].re.mul_add(fft[bin].re, fft[bin].im * fft[bin].im)
            });
            for filter in &self.mel_filters {
                let energy = filter
                    .iter()
                    .map(|(bin, weight)| power[*bin] * weight)
                    .sum::<f32>()
                    .max(f32::EPSILON);
                features.push(energy.ln());
            }
        }

        // 3D-Speaker inference applies utterance-level cepstral mean
        // normalization after Kaldi fbank extraction.
        for dimension in 0..FEATURE_DIMENSIONS {
            let mean = (0..frames)
                .map(|frame| features[frame * FEATURE_DIMENSIONS + dimension])
                .sum::<f32>()
                / frames as f32;
            for frame in 0..frames {
                features[frame * FEATURE_DIMENSIONS + dimension] -= mean;
            }
        }
        features
    }
}

fn mel_scale(frequency: f32) -> f32 {
    1127.0 * (1.0 + frequency / 700.0).ln()
}

fn fft_in_place(values: &mut [Complex; FFT_SIZE]) {
    for index in 1..FFT_SIZE {
        let reversed = index.reverse_bits() >> (usize::BITS - FFT_SIZE.trailing_zeros());
        if index < reversed {
            values.swap(index, reversed);
        }
    }
    let mut length = 2;
    while length <= FFT_SIZE {
        let angle = -2.0 * PI / length as f32;
        let root = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for start in (0..FFT_SIZE).step_by(length) {
            let mut twiddle = Complex { re: 1.0, im: 0.0 };
            for offset in 0..length / 2 {
                let even = values[start + offset];
                let odd = multiply(values[start + offset + length / 2], twiddle);
                values[start + offset] = Complex {
                    re: even.re + odd.re,
                    im: even.im + odd.im,
                };
                values[start + offset + length / 2] = Complex {
                    re: even.re - odd.re,
                    im: even.im - odd.im,
                };
                twiddle = multiply(twiddle, root);
            }
        }
        length *= 2;
    }
}

fn multiply(left: Complex, right: Complex) -> Complex {
    Complex {
        re: left.re.mul_add(right.re, -(left.im * right.im)),
        im: left.re.mul_add(right.im, left.im * right.re),
    }
}

fn normalize_embedding(mut embedding: Vec<f32>) -> Result<Vec<f32>, SpeakerError> {
    if embedding.is_empty() {
        return Err(SpeakerError::EmptyEmbedding);
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(SpeakerError::NonFiniteEmbedding);
    }
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return Err(SpeakerError::EmptyEmbedding);
    }
    for value in &mut embedding {
        *value /= norm;
    }
    Ok(embedding)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return -1.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}
