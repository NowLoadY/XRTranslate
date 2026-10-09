//! Stateful 16 kHz Silero VAD inference and deterministic utterance endpointing.
//!
//! [`SileroVad`] is deliberately narrow: the bundled Silero ONNX model expects
//! mono PCM16 audio at 16 kHz in windows of exactly [`FRAME_SAMPLES`] samples.
//! It owns the recurrent model state, so callers must keep one instance per
//! input stream and call [`SileroVad::reset`] when a stream is replaced.
//!
//! [`EndpointDetector`] has no dependency on an ONNX model.  It accepts VAD
//! probabilities produced by `SileroVad` (or by a test double), retains a
//! bounded pre-roll, and returns completed utterances after configured silence.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::mem;
use std::path::Path;

use ndarray::{Array1, Array2, ArrayD};
use ort::session::Session;
use ort::value::{DynValue, Value};

/// The only sample rate supported by the streaming Silero interface here.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// Number of 16-bit mono samples in each 32 ms model window at 16 kHz.
pub const FRAME_SAMPLES: usize = 512;

/// Number of bytes in one little-endian PCM16 model window.
pub const FRAME_BYTES: usize = FRAME_SAMPLES * std::mem::size_of::<i16>();

const CONTEXT_SAMPLES: usize = 64;
const STATE_SHAPE: [usize; 3] = [2, 1, 128];
const MIN_DYNAMIC_SILENCE_FRAMES: usize = 2; // 64 ms at 16 kHz / 512 samples.
const DYNAMIC_SILENCE_CALIBRATION_PERCENT: usize = 85;

/// Errors from PCM validation, endpoint configuration, and ONNX inference.
#[derive(Debug)]
pub enum VadError {
    /// The caller supplied a frame whose sample count differs from 512.
    InvalidFrameSamples {
        /// Required number of samples.
        expected: usize,
        /// Actual number of samples.
        actual: usize,
    },
    /// A PCM16LE frame does not contain exactly 1,024 bytes.
    InvalidPcmBytes {
        /// Required byte count.
        expected: usize,
        /// Actual byte count.
        actual: usize,
    },
    /// A VAD probability must be finite and in the inclusive range 0..=1.
    InvalidSpeechProbability {
        /// Invalid supplied value.
        value: f32,
    },
    /// The speech threshold must be finite and in the inclusive range 0..=1.
    InvalidSpeechThreshold {
        /// Invalid supplied value.
        value: f32,
    },
    /// At least one silence frame is required to automatically finish speech.
    InvalidSilenceFrames,
    /// At least one speech frame is required to start speech.
    InvalidMinSpeechFrames,
    /// Opening window must be at least min_speech_frames_to_start.
    InvalidOpeningWindowFrames,
    /// At least one active-speech frame is required to bound utterance memory.
    InvalidMaxActiveFrames,
    /// Adaptive endpointing must begin within the hard active-speech limit.
    InvalidAdaptiveAfterFrames,
    /// Adaptive endpointing requires a non-zero silence window no longer than
    /// the ordinary sentence-ending silence window.
    InvalidAdaptiveSilenceFrames,
    /// Hard-split overlap must be shorter than the hard active-speech limit.
    InvalidMaxActiveOverlapFrames,
    /// The loaded model did not return a named output expected from Silero.
    MissingModelOutput {
        /// Expected ONNX output name.
        name: &'static str,
    },
    /// The loaded model returned an invalid speech probability.
    InvalidModelOutput,
    /// An ONNX Runtime operation failed.
    Ort(ort::Error),
}

impl fmt::Display for VadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameSamples { expected, actual } => write!(
                formatter,
                "Silero VAD requires exactly {expected} samples at {SAMPLE_RATE_HZ} Hz; received {actual}"
            ),
            Self::InvalidPcmBytes { expected, actual } => write!(
                formatter,
                "Silero VAD requires exactly {expected} PCM16LE bytes; received {actual}"
            ),
            Self::InvalidSpeechProbability { value } => write!(
                formatter,
                "speech probability must be finite and within 0..=1; received {value}"
            ),
            Self::InvalidSpeechThreshold { value } => write!(
                formatter,
                "speech threshold must be finite and within 0..=1; received {value}"
            ),
            Self::InvalidSilenceFrames => {
                formatter.write_str("silence_frames_to_finalize must be at least one")
            }
            Self::InvalidMinSpeechFrames => {
                formatter.write_str("min_speech_frames_to_start must be at least one")
            }
            Self::InvalidOpeningWindowFrames => formatter
                .write_str("opening_window_frames must be at least min_speech_frames_to_start"),
            Self::InvalidMaxActiveFrames => {
                formatter.write_str("max_active_frames must be at least one")
            }
            Self::InvalidAdaptiveAfterFrames => formatter
                .write_str("adaptive_silence_after_frames must be within 1..=max_active_frames"),
            Self::InvalidAdaptiveSilenceFrames => formatter.write_str(
                "adaptive_silence_frames_to_finalize must be within 1..=silence_frames_to_finalize",
            ),
            Self::InvalidMaxActiveOverlapFrames => {
                formatter.write_str("max_active_overlap_frames must be less than max_active_frames")
            }
            Self::MissingModelOutput { name } => {
                write!(
                    formatter,
                    "Silero ONNX model did not return required output {name:?}"
                )
            }
            Self::InvalidModelOutput => {
                formatter.write_str("Silero ONNX model returned an invalid speech probability")
            }
            Self::Ort(error) => error.fmt(formatter),
        }
    }
}

impl Error for VadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ort(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ort::Error> for VadError {
    fn from(error: ort::Error) -> Self {
        Self::Ort(error)
    }
}

/// Verifies that a model frame has the required sample count.
pub fn validate_frame(samples: &[i16]) -> Result<(), VadError> {
    if samples.len() != FRAME_SAMPLES {
        return Err(VadError::InvalidFrameSamples {
            expected: FRAME_SAMPLES,
            actual: samples.len(),
        });
    }
    Ok(())
}

/// Decodes one exact 16 kHz mono PCM16LE model frame.
///
/// This explicit conversion is preferred at transport boundaries so byte order
/// and frame length are checked before stateful inference or endpointing.
pub fn decode_pcm16le_frame(bytes: &[u8]) -> Result<[i16; FRAME_SAMPLES], VadError> {
    if bytes.len() != FRAME_BYTES {
        return Err(VadError::InvalidPcmBytes {
            expected: FRAME_BYTES,
            actual: bytes.len(),
        });
    }

    let mut samples = [0_i16; FRAME_SAMPLES];
    for (sample, encoded) in samples.iter_mut().zip(bytes.chunks_exact(2)) {
        *sample = i16::from_le_bytes([encoded[0], encoded[1]]);
    }
    Ok(samples)
}

/// Stateful inference wrapper for Silero's 16 kHz ONNX VAD model.
///
/// The instance is intentionally not `Clone`: copying it would fork the ONNX
/// recurrent state and make stream ownership ambiguous.  Use a separate
/// instance for every microphone or remote-speaker stream.
#[derive(Debug)]
pub struct SileroVad {
    session: Session,
    sample_rate: DynValue,
    state: ArrayD<f32>,
    context: Array1<f32>,
}

impl SileroVad {
    /// Loads a standard Silero 16 kHz ONNX model and initializes its recurrent state.
    pub fn from_file(model_path: impl AsRef<Path>) -> Result<Self, VadError> {
        let session = Session::builder()?.commit_from_file(model_path)?;
        Ok(Self {
            session,
            sample_rate: Value::from_array(Array1::from_vec(vec![i64::from(SAMPLE_RATE_HZ)]))?
                .into_dyn(),
            state: initial_state(),
            context: Array1::zeros(CONTEXT_SAMPLES),
        })
    }

    /// Clears the recurrent state and the preceding-sample context.
    pub fn reset(&mut self) {
        self.state = initial_state();
        self.context = Array1::zeros(CONTEXT_SAMPLES);
    }

    /// Runs one frame of PCM16 mono audio and returns its speech probability.
    ///
    /// The frame must contain exactly 512 samples.  Any failed model request
    /// resets internal recurrence so a later frame cannot use partial state.
    pub fn infer(&mut self, samples: &[i16]) -> Result<f32, VadError> {
        validate_frame(samples)?;

        let normalized = samples
            .iter()
            .map(|sample| f32::from(*sample) / f32::from(i16::MAX))
            .collect::<Vec<_>>();

        let mut input_with_context = Vec::with_capacity(CONTEXT_SAMPLES + FRAME_SAMPLES);
        input_with_context.extend(self.context.iter().copied());
        input_with_context.extend(normalized.iter().copied());
        let frame = Array2::from_shape_vec((1, input_with_context.len()), input_with_context)
            .expect("context and validated frame lengths always form a valid 2D array");
        let frame_value = Value::from_array(frame)?;
        // `Value` takes ownership of the tensor.  Replace first so a failed
        // invocation cannot leave an invalid/partly-consumed state behind.
        let state_value = Value::from_array(mem::replace(&mut self.state, initial_state()))?;
        // Keep `SessionOutputs` and its borrow of `self.session` inside this
        // scope.  The result is owned, allowing us to reset or update the
        // recurrent fields only after ORT has released that borrow.
        let result = (|| -> Result<(ArrayD<f32>, f32), VadError> {
            let outputs = self.session.run([
                (&frame_value).into(),
                (&state_value).into(),
                (&self.sample_rate).into(),
            ])?;
            let state_output = outputs
                .get("stateN")
                .ok_or(VadError::MissingModelOutput { name: "stateN" })?;
            let (shape, data) = state_output.try_extract_tensor::<f32>()?;
            let state_shape = shape
                .iter()
                .map(|dimension| {
                    usize::try_from(*dimension).map_err(|_| VadError::InvalidModelOutput)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let next_state = ArrayD::from_shape_vec(state_shape, data.to_vec())
                .map_err(|_| VadError::InvalidModelOutput)?;

            let probability_output = outputs
                .get("output")
                .ok_or(VadError::MissingModelOutput { name: "output" })?;
            let (_, data) = probability_output.try_extract_tensor::<f32>()?;
            let probability = *data.first().ok_or(VadError::InvalidModelOutput)?;
            validate_probability(probability).map_err(|_| VadError::InvalidModelOutput)?;
            Ok((next_state, probability))
        })();

        match result {
            Ok((next_state, probability)) => {
                self.state = next_state;
                self.context =
                    Array1::from_vec(normalized[FRAME_SAMPLES - CONTEXT_SAMPLES..].to_vec());
                Ok(probability)
            }
            Err(error) => {
                self.reset();
                Err(error)
            }
        }
    }

    /// Decodes and evaluates exactly one PCM16LE transport frame.
    pub fn infer_pcm16le(&mut self, bytes: &[u8]) -> Result<f32, VadError> {
        let samples = decode_pcm16le_frame(bytes)?;
        self.infer(&samples)
    }
}

fn initial_state() -> ArrayD<f32> {
    ArrayD::zeros(STATE_SHAPE.as_slice())
}

fn validate_probability(value: f32) -> Result<(), VadError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(VadError::InvalidSpeechProbability { value });
    }
    Ok(())
}

/// Tuning values for [`EndpointDetector`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndpointConfig {
    /// Probability at or above which a frame is considered speech.
    pub speech_threshold: f32,
    /// Consecutive non-speech frames that finish an active utterance.
    pub silence_frames_to_finalize: usize,
    /// Calibration duration for dynamic endpointing. By roughly this point,
    /// long utterances accept a shorter natural pause.
    pub adaptive_silence_after_frames: usize,
    /// Preferred long-utterance pause near
    /// [`Self::adaptive_silence_after_frames`]. Longer speech keeps shrinking
    /// the required pause down to the fixed 64 ms lower bound.
    pub adaptive_silence_frames_to_finalize: usize,
    /// Number of previous non-speech frames copied before a speech start.
    pub pre_roll_frames: usize,
    /// Maximum number of 32 ms frames accepted after speech starts.
    ///
    /// Pre-roll is governed separately by [`Self::pre_roll_frames`], so the
    /// maximum number of frames retained for one active utterance is bounded by
    /// `pre_roll_frames + max_active_frames`.  Once this limit is reached the
    /// detector returns a finalized utterance whose reason is
    /// [`UtteranceEndReason::MaxActiveFrames`].
    pub max_active_frames: usize,
    /// Tail frames copied into the next utterance after a hard split. This
    /// protects phonemes spanning the forced boundary; downstream ASR text
    /// must remove the matching prefix.
    pub max_active_overlap_frames: usize,
    /// Number of speech frames within [`Self::opening_window_frames`] required to transition to speech start.
    /// Defaults to 3 (roughly 96 ms) within a 4-frame (roughly 128 ms) window.
    pub min_speech_frames_to_start: usize,
    /// Window of recent frames observed to detect speech onset. Defaults to 4 (roughly 128 ms).
    pub opening_window_frames: usize,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            speech_threshold: 0.5,
            // 512 samples at 16 kHz is 32 ms; 16 frames is roughly 512 ms.
            silence_frames_to_finalize: 16,
            // After ~4 seconds, accept a ~128 ms micro-pause.
            adaptive_silence_after_frames: 125,
            adaptive_silence_frames_to_finalize: 4,
            // Preserve the ~320 ms pre-roll used by the prior Qwen3 path.
            pre_roll_frames: 10,
            // Bound uninterrupted speech at ~8 seconds with ~256 ms overlap.
            max_active_frames: 250,
            max_active_overlap_frames: 8,
            min_speech_frames_to_start: 3,
            opening_window_frames: 4,
        }
    }
}

impl EndpointConfig {
    /// Checks configuration before any audio is accepted.
    pub fn validate(self) -> Result<Self, VadError> {
        if !self.speech_threshold.is_finite() || !(0.0..=1.0).contains(&self.speech_threshold) {
            return Err(VadError::InvalidSpeechThreshold {
                value: self.speech_threshold,
            });
        }
        if self.silence_frames_to_finalize == 0 {
            return Err(VadError::InvalidSilenceFrames);
        }
        if self.min_speech_frames_to_start == 0 {
            return Err(VadError::InvalidMinSpeechFrames);
        }
        if self.opening_window_frames < self.min_speech_frames_to_start {
            return Err(VadError::InvalidOpeningWindowFrames);
        }
        if self.max_active_frames == 0 {
            return Err(VadError::InvalidMaxActiveFrames);
        }
        if self.adaptive_silence_after_frames == 0
            || self.adaptive_silence_after_frames > self.max_active_frames
        {
            return Err(VadError::InvalidAdaptiveAfterFrames);
        }
        if self.adaptive_silence_frames_to_finalize == 0
            || self.adaptive_silence_frames_to_finalize > self.silence_frames_to_finalize
        {
            return Err(VadError::InvalidAdaptiveSilenceFrames);
        }
        if self.max_active_overlap_frames >= self.max_active_frames {
            return Err(VadError::InvalidMaxActiveOverlapFrames);
        }
        Ok(self)
    }

    /// Duration, in milliseconds, of the active-speech memory cap.
    ///
    /// This excludes the independently bounded pre-roll duration.
    pub const fn max_active_duration_ms(self) -> u64 {
        self.max_active_frames as u64 * FRAME_SAMPLES as u64 * 1_000 / SAMPLE_RATE_HZ as u64
    }

    fn silence_frames_to_finalize_for(self, active_frames: usize) -> usize {
        let ordinary = self.silence_frames_to_finalize;
        if ordinary <= MIN_DYNAMIC_SILENCE_FRAMES {
            return ordinary;
        }
        let preferred = self
            .adaptive_silence_frames_to_finalize
            .max(MIN_DYNAMIC_SILENCE_FRAMES)
            .min(ordinary);
        let preferred_drop = ordinary.saturating_sub(preferred);
        if preferred_drop == 0 {
            return ordinary;
        }
        let calibration_frames = self
            .adaptive_silence_after_frames
            .saturating_mul(DYNAMIC_SILENCE_CALIBRATION_PERCENT)
            .div_ceil(100)
            .max(1);
        let maximum_drop = ordinary - MIN_DYNAMIC_SILENCE_FRAMES;
        let drop = preferred_drop
            .saturating_mul(active_frames)
            .checked_div(calibration_frames)
            .unwrap_or(maximum_drop)
            .min(maximum_drop);
        (ordinary - drop).max(MIN_DYNAMIC_SILENCE_FRAMES)
    }
}

/// State of an [`EndpointDetector`] after its most recent operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointState {
    /// No speech is active; incoming non-speech frames populate pre-roll.
    Listening,
    /// Speech is active, with the number of consecutive trailing silence frames.
    Speaking {
        /// Consecutive trailing frames whose VAD probability was below threshold.
        trailing_silence_frames: usize,
    },
}

/// A fully-owned completed utterance ready to pass to ASR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Utterance {
    /// Mono PCM16 samples at [`SAMPLE_RATE_HZ`].
    pub samples: Vec<i16>,
    /// Number of pre-speech frames prepended when the utterance began.
    pub pre_roll_frames: usize,
    /// Leading frames duplicated from the previous hard-limited utterance.
    pub overlap_frames: usize,
    /// Number of below-threshold frames included at the end of this utterance.
    pub trailing_silence_frames: usize,
    /// Why the detector ended this utterance.
    pub end_reason: UtteranceEndReason,
}

impl Utterance {
    /// Number of 32 ms model frames contained in [`Self::samples`].
    pub fn frame_count(&self) -> usize {
        self.samples.len() / FRAME_SAMPLES
    }
}

/// Reason an [`Utterance`] was returned to the caller.
///
/// A max-active limit is a normal, lossless segmentation boundary: send the
/// returned utterance to ASR exactly as for a silence-finalized utterance, then
/// continue feeding subsequent audio to the same detector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UtteranceEndReason {
    /// The configured amount of trailing silence finished speech.
    Silence,
    /// A shorter micro-pause ended an utterance that had already reached the
    /// preferred responsive duration.
    AdaptiveSilence,
    /// Active speech reached [`EndpointConfig::max_active_frames`].
    MaxActiveFrames,
    /// A change in speaker voiceprint forced early segmentation.
    SpeakerChange,
    /// The owner explicitly flushed the detector, for example at turn end.
    Flushed,
}

/// Result of accepting one frame into an [`EndpointDetector`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointEvent {
    /// No speech is active; the frame may have been retained as future pre-roll.
    Listening,
    /// The frame started speech and an utterance is now accumulating.
    SpeechStarted,
    /// An active utterance accepted another speech or trailing-silence frame.
    SpeechContinues {
        /// Consecutive below-threshold frames currently buffered at the end.
        trailing_silence_frames: usize,
    },
    /// Trailing silence or the active-frame safety limit completed the utterance.
    Finalized(Utterance),
}

/// Pure PCM endpointing state machine driven by VAD probabilities.
///
/// It deliberately does not load an ONNX model, which makes the segmentation
/// behavior deterministic and independently testable.  Call [`Self::push`] with
/// the output of [`SileroVad::infer`] in production.
#[derive(Debug)]
pub struct EndpointDetector {
    config: EndpointConfig,
    pre_roll: VecDeque<Vec<i16>>,
    hard_split_overlap: VecDeque<Vec<i16>>,
    opening_buffer: VecDeque<(Vec<i16>, bool)>,
    active: Option<ActiveUtterance>,
}

#[derive(Debug)]
struct ActiveUtterance {
    samples: Vec<i16>,
    pre_roll_frames: usize,
    overlap_frames: usize,
    active_frames: usize,
    trailing_silence_frames: usize,
}

impl EndpointDetector {
    /// Builds an endpoint detector after validating its finite numeric values.
    pub fn new(config: EndpointConfig) -> Result<Self, VadError> {
        Ok(Self {
            config: config.validate()?,
            pre_roll: VecDeque::with_capacity(config.pre_roll_frames),
            hard_split_overlap: VecDeque::with_capacity(config.max_active_overlap_frames),
            opening_buffer: VecDeque::with_capacity(config.opening_window_frames),
            active: None,
        })
    }

    /// Returns the detector's current state without exposing its internal audio.
    pub fn state(&self) -> EndpointState {
        match &self.active {
            Some(active) => EndpointState::Speaking {
                trailing_silence_frames: active.trailing_silence_frames,
            },
            None => EndpointState::Listening,
        }
    }

    /// Returns the validated immutable tuning values.
    pub const fn config(&self) -> EndpointConfig {
        self.config
    }

    /// Returns the number of post-start frames retained by the active utterance.
    ///
    /// This excludes bounded pre-roll.  It is zero while listening and never
    /// exceeds [`EndpointConfig::max_active_frames`].
    pub fn active_frame_count(&self) -> usize {
        self.active
            .as_ref()
            .map_or(0, |active| active.active_frames)
    }

    /// Accepts one PCM frame and its speech probability.
    ///
    /// Completed output contains pre-roll and all accepted active frames.  It
    /// ends after configured trailing silence or at the active-frame safety
    /// limit.  The next incoming frames begin a fresh pre-roll.
    pub fn push(&mut self, samples: &[i16], probability: f32) -> Result<EndpointEvent, VadError> {
        validate_frame(samples)?;
        validate_probability(probability)?;

        let is_speech = probability >= self.config.speech_threshold;

        if self.active.is_some() {
            let end_reason = {
                let active = self
                    .active
                    .as_mut()
                    .expect("active utterance was checked above");
                active.samples.extend_from_slice(samples);
                active.active_frames += 1;
                if is_speech {
                    active.trailing_silence_frames = 0;
                } else {
                    active.trailing_silence_frames += 1;
                }

                if !is_speech {
                    let silence_frames_to_finalize = self
                        .config
                        .silence_frames_to_finalize_for(active.active_frames);
                    if active.trailing_silence_frames >= silence_frames_to_finalize {
                        Some(
                            if silence_frames_to_finalize < self.config.silence_frames_to_finalize {
                                UtteranceEndReason::AdaptiveSilence
                            } else {
                                UtteranceEndReason::Silence
                            },
                        )
                    } else {
                        None
                    }
                } else if active.active_frames >= self.config.max_active_frames {
                    Some(UtteranceEndReason::MaxActiveFrames)
                } else {
                    None
                }
            };

            if let Some(reason) = end_reason {
                return Ok(EndpointEvent::Finalized(self.finalize_active(reason)));
            }
            return Ok(EndpointEvent::SpeechContinues {
                trailing_silence_frames: self
                    .active
                    .as_ref()
                    .expect("active utterance has not finalized")
                    .trailing_silence_frames,
            });
        }

        let overlap_frames = self.hard_split_overlap.len();
        if overlap_frames > 0 {
            if !is_speech {
                self.hard_split_overlap.clear();
                self.push_pre_roll(samples.to_vec());
                return Ok(EndpointEvent::Listening);
            }
            self.opening_buffer.clear();
            let mut utterance = Vec::with_capacity((overlap_frames + 1) * FRAME_SAMPLES);
            for overlap_frame in self.hard_split_overlap.drain(..) {
                utterance.extend_from_slice(&overlap_frame);
            }
            self.pre_roll.clear();
            utterance.extend_from_slice(samples);

            self.active = Some(ActiveUtterance {
                samples: utterance,
                pre_roll_frames: 0,
                overlap_frames,
                active_frames: 1,
                trailing_silence_frames: 0,
            });
            if self.config.max_active_frames <= 1 {
                return Ok(EndpointEvent::Finalized(
                    self.finalize_active(UtteranceEndReason::MaxActiveFrames),
                ));
            }
            return Ok(EndpointEvent::SpeechStarted);
        }

        if !is_speech && self.opening_buffer.is_empty() {
            self.hard_split_overlap.clear();
            self.push_pre_roll(samples.to_vec());
            return Ok(EndpointEvent::Listening);
        }

        if self.opening_buffer.len() >= self.config.opening_window_frames {
            let (popped_samples, popped_is_speech) = self
                .opening_buffer
                .pop_front()
                .expect("opening_buffer length was checked above");
            if !popped_is_speech {
                self.push_pre_roll(popped_samples);
            }
        }
        self.opening_buffer.push_back((samples.to_vec(), is_speech));

        let speech_count = self
            .opening_buffer
            .iter()
            .filter(|(_, speech)| *speech)
            .count();

        if speech_count < self.config.min_speech_frames_to_start {
            if speech_count == 0 {
                while let Some((silence_frame, _)) = self.opening_buffer.pop_front() {
                    self.push_pre_roll(silence_frame);
                }
            }
            return Ok(EndpointEvent::Listening);
        }

        let confirmed_opening_frames = self.opening_buffer.len();
        let pre_roll_frames = self.pre_roll.len();
        let mut utterance =
            Vec::with_capacity((pre_roll_frames + confirmed_opening_frames) * FRAME_SAMPLES);

        for pre_roll_frame in self.pre_roll.drain(..) {
            utterance.extend_from_slice(&pre_roll_frame);
        }
        for (opening_frame, _) in self.opening_buffer.drain(..) {
            utterance.extend_from_slice(&opening_frame);
        }

        self.active = Some(ActiveUtterance {
            samples: utterance,
            pre_roll_frames,
            overlap_frames: 0,
            active_frames: confirmed_opening_frames,
            trailing_silence_frames: 0,
        });

        if self.config.max_active_frames <= confirmed_opening_frames {
            return Ok(EndpointEvent::Finalized(
                self.finalize_active(UtteranceEndReason::MaxActiveFrames),
            ));
        }
        Ok(EndpointEvent::SpeechStarted)
    }

    /// Decodes one PCM16LE frame and accepts it using `probability`.
    pub fn push_pcm16le(
        &mut self,
        bytes: &[u8],
        probability: f32,
    ) -> Result<EndpointEvent, VadError> {
        let samples = decode_pcm16le_frame(bytes)?;
        self.push(&samples, probability)
    }

    /// Completes active speech immediately, for example when a microphone is stopped.
    ///
    /// Calling this while listening discards only retained pre-roll, since there
    /// is no utterance to submit to ASR.
    pub fn flush(&mut self) -> Option<Utterance> {
        self.opening_buffer.clear();
        self.pre_roll.clear();
        self.hard_split_overlap.clear();
        self.active
            .take()
            .map(|active| active.into_utterance(UtteranceEndReason::Flushed))
    }

    /// Forces an active utterance to finalize because a speaker voiceprint change was detected.
    /// Retains overlap frames for the incoming speaker turn.
    pub fn split_on_speaker_change(&mut self) -> Option<Utterance> {
        if self.active.is_some() {
            Some(self.finalize_active(UtteranceEndReason::SpeakerChange))
        } else {
            None
        }
    }

    /// Discards active speech and retained pre-roll without emitting an utterance.
    pub fn reset(&mut self) {
        self.opening_buffer.clear();
        self.pre_roll.clear();
        self.hard_split_overlap.clear();
        self.active = None;
    }

    fn finalize_active(&mut self, reason: UtteranceEndReason) -> Utterance {
        let active = self
            .active
            .take()
            .expect("finalization requires an active utterance");
        self.hard_split_overlap.clear();
        if reason == UtteranceEndReason::MaxActiveFrames
            || reason == UtteranceEndReason::SpeakerChange
        {
            let retained = self
                .config
                .max_active_overlap_frames
                .min(active.active_frames);
            let frame_count = active.samples.len() / FRAME_SAMPLES;
            let first_retained = frame_count.saturating_sub(retained);
            for frame in active
                .samples
                .chunks_exact(FRAME_SAMPLES)
                .skip(first_retained)
            {
                self.hard_split_overlap.push_back(frame.to_vec());
            }
        }
        active.into_utterance(reason)
    }

    fn push_pre_roll(&mut self, frame: Vec<i16>) {
        if self.config.pre_roll_frames == 0 {
            return;
        }
        if self.pre_roll.len() == self.config.pre_roll_frames {
            let _ = self.pre_roll.pop_front();
        }
        self.pre_roll.push_back(frame);
    }
}

impl ActiveUtterance {
    fn into_utterance(self, end_reason: UtteranceEndReason) -> Utterance {
        Utterance {
            samples: self.samples,
            pre_roll_frames: self.pre_roll_frames,
            overlap_frames: self.overlap_frames,
            trailing_silence_frames: self.trailing_silence_frames,
            end_reason,
        }
    }
}
