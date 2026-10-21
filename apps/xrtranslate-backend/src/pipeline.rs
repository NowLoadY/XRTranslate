// SPDX-FileCopyrightText: 2026 febilly
// SPDX-FileCopyrightText: 2026 NowLoadY
// SPDX-License-Identifier: AGPL-3.0-only

//! Provider-neutral native audio pipeline and shared recognition policies.
//!
//! Every `NativePipeline` is owned by a single WebSocket session.  In
//! particular, the Silero ONNX recurrent state must never be shared between
//! microphone streams.  Model servers are shared out-of-process through their
//! local llama.cpp HTTP endpoints.
//!
//! The continuous-mode idle gate, bounded pre-roll, and delayed release were
//! informed by febilly's MIT-licensed Yakutan local VAD pipeline. XRTranslate
//! retains fixed overlapping windows and runs each source in its own session.

mod asr_prompt;
mod audio;
mod preprocessing;
mod streaming;
use audio::{AudioBuffer, AudioFrames, SpeechGate};
use preprocessing::{AudioPreprocessor, VocalActivity};
use streaming::LiveCapture;
pub(crate) use streaming::LiveUtterance;

use std::{
    collections::VecDeque,
    mem,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::warn;

use xrtranslate_config::AppConfig;
use xrtranslate_engine::{
    TranslationSegmentPair, remove_asr_stutters, remove_transcript_overlap,
    strip_filler_edges_for_lang, translation_segment_pairs_for_text_with_lang,
};
use xrtranslate_inference::{
    AsrTranscript, InferenceError, ReqwestClient, TranslationAdapter, TranslationOptions,
    is_probable_asr_hallucination,
};
use xrtranslate_prompt::{
    AsrPromptContext, PromptExecutionTrace, PromptNodeGraph, TranslationPromptContext,
};
use xrtranslate_protocol::{AudioPreprocessing, AudioSource, InferenceWorkload};
use xrtranslate_speaker::{
    OnlineSpeakerDiarizer, StreamingDiarizerConfig, StreamingSpeakerEvent,
    StreamingSpeakerSegmenter, TrackerConfig,
};
use xrtranslate_vad::{
    EndpointConfig, EndpointDetector, EndpointEvent, FRAME_BYTES, FRAME_SAMPLES, SAMPLE_RATE_HZ,
    SileroVad, Utterance, UtteranceEndReason, decode_pcm16le_frame,
};

use xrtranslate_engine::language::{LanguageCapabilities, LanguageMode};

use crate::language::{
    AdaptiveLanguageRoute, AutoDecision, SupportedLanguage, is_traditional_chinese,
    to_traditional_chinese,
};
use crate::model_runtime::{NativeAsrAdapter, NativeAsrOptions, NativeProviderPlan};

use asr_prompt::{AsrPromptDelivery, AsrPromptPolicy};

const STREAM_END_FRAMES: usize = 62;

const SILERO_VAD_MODEL: &str = "models/silero-vad/src/silero_vad/data/silero_vad.onnx";

#[derive(Debug)]
pub(crate) struct InferenceFailure {
    pub(crate) message: String,
}

impl InferenceFailure {
    fn request(context: &str, error: InferenceError) -> Self {
        Self {
            message: format!("{context}: {error}"),
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
/// Largest binary WebSocket message accepted from a microphone client.
///
/// At 16 kHz mono PCM16 this is eight seconds.  Longer audio must arrive in
/// multiple WebSocket messages so a client cannot allocate an unbounded VAD,
/// WAV, and base64 working set in one request.
pub(crate) const MAX_INPUT_PCM_BYTES: usize = 256 * 1024;

/// The normalized ASR result and every emittable translation segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecognizedOutput {
    pub(crate) source_text: String,
    pub(crate) segments: Vec<TranslationSegmentPair>,
    pub(crate) source_language: String,
    pub(crate) target_language: String,
    pub(crate) asr_elapsed: Duration,
    pub(crate) route_switched: Option<String>,
    pub(crate) prompt_trace: Option<PromptExecutionTrace>,
}

/// VAD output paired with its absolute position inside the current audio epoch.
#[derive(Debug)]
pub(crate) struct TimedUtterance {
    pub(crate) utterance: Utterance,
    pub(crate) source_start_ms: f64,
    pub(crate) source_end_ms: f64,
    pub(crate) revisable: bool,
    pub(crate) topic_turn_sequence: Option<u64>,
    pub(crate) speaker_id: Option<String>,
}

#[derive(Debug)]
pub(crate) enum PipelineEvent {
    Utterance(TimedUtterance),
    Streaming(LiveUtterance),
    StreamEnded,
}

impl RecognizedOutput {
    /// Terminology belongs to an output scope. Keep the recognition sentence
    /// identities stable even when a replacement introduces sentence punctuation,
    /// so another output can correct and translate the same original segments.
    pub(crate) fn apply_terminology(
        &mut self,
        corrections: &[xr_corpus_protocol::CorpusRecognitionCorrection],
        contexts: &[xr_corpus_protocol::SegmentContext],
    ) {
        use crate::terminology::rewrite_recognition_terms;
        self.source_text = rewrite_recognition_terms(&self.source_text, corrections).corrected_text;
        for (segment, context) in self.segments.iter_mut().zip(contexts) {
            segment.translation_text =
                rewrite_recognition_terms(&segment.translation_text, &context.source_corrections)
                    .corrected_text;
            segment.source_text =
                rewrite_recognition_terms(&segment.source_text, &context.source_corrections)
                    .corrected_text;
        }
    }

    pub(crate) fn apply_source_correction(&mut self, corrected: String) {
        if corrected == self.source_text {
            return;
        }
        self.source_text = corrected;
        self.segments =
            translation_segment_pairs_for_text_with_lang(&self.source_text, &self.source_language);
    }

    pub(crate) fn prepare_sentence_snapshot(&mut self) {
        let cleaned = strip_filler_edges_for_lang(&self.source_text, &self.source_language);
        self.segments =
            translation_segment_pairs_for_text_with_lang(&self.source_text, &self.source_language);
        if self.segments.is_empty() {
            self.segments = vec![TranslationSegmentPair {
                source_text: self.source_text.clone(),
                translation_text: if cleaned.is_empty() {
                    self.source_text.clone()
                } else {
                    cleaned
                },
            }];
        }
    }

    /// Removes text produced from the duplicated audio at a hard VAD boundary.
    /// Returns false when the new result contained only duplicated context.
    pub(crate) fn remove_overlap_with(&mut self, previous: &str) -> bool {
        self.source_text = remove_transcript_overlap(previous, &self.source_text);
        self.segments =
            translation_segment_pairs_for_text_with_lang(&self.source_text, &self.source_language);
        !self.source_text.is_empty()
    }
}

/// A single provider translation emitted after a recognized source segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslationOutput {
    pub(crate) additional_source_text: Option<String>,
    pub(crate) additional_translations: Vec<xrtranslate_protocol::AdditionalTranslation>,
    pub(crate) asr_only: bool,
    pub(crate) source_text: String,
    pub(crate) translated_text: String,
    pub(crate) source_language: String,
    pub(crate) target_language: String,
    pub(crate) term_matches: Vec<xrtranslate_protocol::CorpusTermMatch>,
    pub(crate) prompt_trace: Option<PromptExecutionTrace>,
    pub(crate) mt_elapsed: Duration,
}

/// Per-session audio preprocessing and segmentation for every ASR connector.
pub(crate) struct NativePipeline {
    vad: SileroVad,
    endpoint: EndpointDetector,
    endpoint_config: EndpointConfig,
    base_endpoint_config: EndpointConfig,
    preprocessing: AudioPreprocessor,
    preprocessing_mode: AudioPreprocessing,
    vocal_activity: VocalActivity,
    streaming_speaker: Option<StreamingSpeakerSegmenter>,
    speaker_recognition_enabled: bool,
    fixed_window: Option<FixedWindow>,
    live_capture: Option<LiveCapture>,
    audio: AudioBuffer,
    speech_gate: SpeechGate,
    processed_samples: u64,
    vad_active: bool,
    sentence_idle_frames: Option<usize>,
    vad_transitions: Vec<bool>,
    inference: NativeInference,
}

struct FixedWindow {
    samples: Vec<i16>,
    pre_roll: VecDeque<i16>,
    gate_open: bool,
    quiet_frames: usize,
    overlap_samples: usize,
    has_new_activity: bool,
    emitted: bool,
    opening_frames: usize,
    adaptive_context: bool,
    expanded_context: bool,
    sparse_results: u8,
    dense_results: u8,
    stream_open: bool,
    idle_frames: usize,
    current_topic_turn_sequence: u64,
    next_topic_turn_sequence: u64,
}

impl FixedWindow {
    const FIRST_WINDOW_SAMPLES: usize = 62 * FRAME_SAMPLES;
    const WINDOW_SAMPLES: usize = 94 * FRAME_SAMPLES;
    const EXPANDED_WINDOW_SAMPLES: usize = 126 * FRAME_SAMPLES;
    const HOP_SAMPLES: usize = 62 * FRAME_SAMPLES;
    const PRE_ROLL_FRAMES: usize = 6;
    const PRE_ROLL_SAMPLES: usize = Self::PRE_ROLL_FRAMES * FRAME_SAMPLES;
    const RELEASE_FRAMES: usize = 16;
    const OPENING_FRAMES: usize = 3;

    fn new(audio_source: AudioSource) -> Self {
        Self {
            samples: Vec::with_capacity(Self::EXPANDED_WINDOW_SAMPLES),
            pre_roll: VecDeque::with_capacity(Self::PRE_ROLL_SAMPLES),
            gate_open: false,
            quiet_frames: 0,
            overlap_samples: 0,
            has_new_activity: false,
            emitted: false,
            opening_frames: 0,
            adaptive_context: audio_source == AudioSource::SystemAudio,
            expanded_context: false,
            sparse_results: 0,
            dense_results: 0,
            stream_open: false,
            idle_frames: 0,
            current_topic_turn_sequence: 0,
            next_topic_turn_sequence: 1,
        }
    }

    fn push(&mut self, frame: &[i16], active: bool) -> Vec<FixedWindowEvent> {
        if !self.gate_open {
            self.push_pre_roll(frame);
            if !active {
                self.opening_frames = 0;
                if self.stream_open {
                    self.idle_frames += 1;
                    if self.idle_frames >= STREAM_END_FRAMES {
                        self.stream_open = false;
                        self.idle_frames = 0;
                        return vec![FixedWindowEvent::StreamEnded];
                    }
                }
                return Vec::new();
            }
            self.opening_frames += 1;
            if self.opening_frames < Self::OPENING_FRAMES {
                return Vec::new();
            }
            self.gate_open = true;
            // Short pauses close the audio gate, but they do not end the
            // logical streaming turn. Keep one identity until StreamEnded so
            // overlapping revisions replace one corpus/history record.
            if !self.stream_open {
                self.current_topic_turn_sequence = self.next_topic_turn_sequence;
                self.next_topic_turn_sequence = self
                    .next_topic_turn_sequence
                    .checked_add(1)
                    .expect("continuous topic turn sequence overflow");
            }
            self.stream_open = true;
            self.idle_frames = 0;
            self.quiet_frames = 0;
            self.has_new_activity = true;
            self.samples.extend(self.pre_roll.drain(..));
        } else if active {
            self.quiet_frames = 0;
            self.has_new_activity = true;
            self.samples.extend_from_slice(frame);
        } else {
            self.quiet_frames += 1;
            self.samples.extend_from_slice(frame);
        }

        let required_samples = if self.emitted {
            self.window_samples()
        } else {
            Self::FIRST_WINDOW_SAMPLES
        };
        if self.samples.len() >= required_samples {
            return vec![FixedWindowEvent::Utterance(self.take_window())];
        }
        if self.quiet_frames >= Self::RELEASE_FRAMES {
            return self.close_gate();
        }
        Vec::new()
    }

    fn push_pre_roll(&mut self, frame: &[i16]) {
        if frame.len() >= Self::PRE_ROLL_SAMPLES {
            self.pre_roll.clear();
            self.pre_roll.extend(
                frame[frame.len() - Self::PRE_ROLL_SAMPLES..]
                    .iter()
                    .copied(),
            );
            return;
        }
        let excess = self
            .pre_roll
            .len()
            .saturating_add(frame.len())
            .saturating_sub(Self::PRE_ROLL_SAMPLES);
        if excess > 0 {
            self.pre_roll.drain(..excess.min(self.pre_roll.len()));
        }
        self.pre_roll.extend(frame.iter().copied());
    }

    fn take_window(&mut self) -> Utterance {
        let window_samples = if self.emitted {
            self.window_samples()
        } else {
            Self::FIRST_WINDOW_SAMPLES
        };
        let previous_overlap_samples = self.overlap_samples;
        let overlap_samples = if self.emitted {
            self.window_samples().saturating_sub(Self::HOP_SAMPLES)
        } else {
            Self::WINDOW_SAMPLES - Self::HOP_SAMPLES
        }
        .min(window_samples.saturating_sub(FRAME_SAMPLES));
        let hop_samples = window_samples - overlap_samples;
        let samples = self.samples[..window_samples].to_vec();
        self.samples.drain(..hop_samples);
        self.overlap_samples = overlap_samples;
        self.has_new_activity = false;
        self.emitted = true;
        Utterance {
            samples,
            pre_roll_frames: 0,
            overlap_frames: previous_overlap_samples / FRAME_SAMPLES,
            trailing_silence_frames: self.quiet_frames,
            end_reason: UtteranceEndReason::MaxActiveFrames,
        }
    }

    fn window_samples(&self) -> usize {
        if self.expanded_context {
            Self::EXPANDED_WINDOW_SAMPLES
        } else {
            Self::WINDOW_SAMPLES
        }
    }

    fn observe_text_density(&mut self, units: usize) {
        if !self.adaptive_context {
            return;
        }
        if units <= 4 {
            self.sparse_results = self.sparse_results.saturating_add(1);
            self.dense_results = 0;
            if self.sparse_results >= 2 {
                self.expanded_context = true;
            }
        } else if units >= 8 {
            self.dense_results = self.dense_results.saturating_add(1);
            self.sparse_results = 0;
            if self.dense_results >= 2 {
                self.expanded_context = false;
                let desired_overlap = Self::WINDOW_SAMPLES - Self::HOP_SAMPLES;
                let excess = self.overlap_samples.saturating_sub(desired_overlap);
                if excess > 0 {
                    self.samples.drain(..excess.min(self.samples.len()));
                    self.overlap_samples = desired_overlap;
                }
            }
        } else {
            self.sparse_results = 0;
            self.dense_results = 0;
        }
    }

    fn close_gate(&mut self) -> Vec<FixedWindowEvent> {
        let mut events = Vec::with_capacity(1);
        let idle_frames = self.quiet_frames;
        if self.has_new_activity {
            events.push(FixedWindowEvent::Utterance(Utterance {
                samples: mem::take(&mut self.samples),
                pre_roll_frames: 0,
                overlap_frames: self.overlap_samples / FRAME_SAMPLES,
                trailing_silence_frames: self.quiet_frames,
                end_reason: UtteranceEndReason::Silence,
            }));
        }
        self.samples.clear();
        self.pre_roll.clear();
        self.gate_open = false;
        self.quiet_frames = 0;
        self.overlap_samples = 0;
        self.has_new_activity = false;
        self.emitted = false;
        self.opening_frames = 0;
        self.idle_frames = idle_frames;
        events
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.pre_roll.clear();
        self.gate_open = false;
        self.quiet_frames = 0;
        self.overlap_samples = 0;
        self.has_new_activity = false;
        self.emitted = false;
        self.opening_frames = 0;
        self.stream_open = false;
        self.idle_frames = 0;
    }

    fn flush(&mut self) -> Option<Utterance> {
        if !self.gate_open {
            self.pre_roll.clear();
            return None;
        }
        let utterance = self.has_new_activity.then(|| Utterance {
            samples: mem::take(&mut self.samples),
            pre_roll_frames: 0,
            overlap_frames: self.overlap_samples / FRAME_SAMPLES,
            trailing_silence_frames: self.quiet_frames,
            end_reason: UtteranceEndReason::Flushed,
        });
        self.reset();
        utterance
    }
}

enum FixedWindowEvent {
    Utterance(Utterance),
    StreamEnded,
}

/// Cloneable, stateless network side of the native pipeline.
///
/// A session actor keeps [`NativePipeline`] on its audio/VAD owner task while
/// moving this value to a bounded inference worker.  That prevents slow local
/// model HTTP calls from blocking WebSocket control and microphone intake.
#[derive(Clone)]
pub(crate) struct NativeInference {
    languages: LanguageCapabilities,
    asr: Option<NativeAsrAdapter>,
    asr_streaming: bool,
    asr_prompt: AsrPromptPolicy,
    translation: TranslationAdapter<ReqwestClient>,
    translation_supports_reference_context: bool,
    asr_max_output_tokens: u32,
    translation_max_output_tokens: u32,
    asr_context_window_tokens: u32,
    translation_context_window_tokens: u32,
    speaker: Option<SpeakerInferenceConfig>,
}

struct AsrAttemptResult {
    transcript: AsrTranscript,
    prompt_trace: Option<PromptExecutionTrace>,
}

#[derive(Clone)]
struct SpeakerInferenceConfig {
    model_path: PathBuf,
    tracker: TrackerConfig,
    min_utterance_ms: u32,
    intra_threads: usize,
}

impl NativePipeline {
    /// Validates the selected route and opens a session-owned Silero model.
    ///
    /// The model servers themselves are deliberately not contacted here.  The
    /// first request reports a precise endpoint error while permitting the
    /// backend launcher to bring llama.cpp up concurrently with the client.
    pub(crate) fn new(
        config: &AppConfig,
        project_root: &Path,
        model_plan: &NativeProviderPlan,
    ) -> Result<Self, String> {
        model_plan.check_assets()?;
        let vad_path = project_root.join(SILERO_VAD_MODEL);
        let vad = SileroVad::from_file(&vad_path)
            .map_err(|error| format!("cannot load Silero VAD {}: {error}", vad_path.display()))?;
        let threshold = config.asr.vad_threshold as f32;
        let endpoint_config = EndpointConfig {
            speech_threshold: threshold,
            silence_frames_to_finalize: frames_for_ms(config.asr.vad_silence_ms),
            adaptive_silence_after_frames: frames_for_ms(config.asr.vad_adaptive_after_ms),
            adaptive_silence_frames_to_finalize: frames_for_ms(config.asr.vad_adaptive_silence_ms),
            // Retain the legacy ~320 ms capture pre-roll.
            pre_roll_frames: 10,
            max_active_frames: frames_for_ms(config.asr.vad_max_utterance_ms),
            max_active_overlap_frames: frames_for_ms(config.asr.vad_overlap_ms),
            min_speech_frames_to_start: 3,
            opening_window_frames: 4,
        };
        let endpoint = EndpointDetector::new(endpoint_config).map_err(|error| error.to_string())?;
        let speaker = if config.speaker.enabled {
            let model_path = if config.speaker.model_path.is_absolute() {
                config.speaker.model_path.clone()
            } else {
                project_root.join(&config.speaker.model_path)
            };
            if !model_path.is_file() {
                return Err(format!(
                    "speaker recognition is enabled but the ERes2NetV2 ONNX model is missing: {}",
                    model_path.display()
                ));
            }
            let tracker = TrackerConfig {
                similarity_threshold: config.speaker.similarity_threshold as f32,
                same_speaker_hysteresis: config.speaker.same_speaker_hysteresis as f32,
                speaker_switch_margin: config.speaker.speaker_switch_margin as f32,
                max_speakers: config.speaker.max_speakers,
            }
            .validate()
            .map_err(|error| error.to_string())?;
            if config.speaker.min_utterance_ms == 0 {
                return Err("speaker.min_utterance_ms must be greater than zero".into());
            }
            if !(1..=64).contains(&config.speaker.intra_threads) {
                return Err("speaker.intra_threads must be within 1..=64".into());
            }
            Some(SpeakerInferenceConfig {
                model_path,
                tracker,
                min_utterance_ms: config.speaker.min_utterance_ms,
                intra_threads: config.speaker.intra_threads,
            })
        } else {
            None
        };

        let streaming_speaker = if let Some(speaker_cfg) = &speaker {
            let streaming_cfg = StreamingDiarizerConfig {
                tracker: speaker_cfg.tracker,
                min_speech_samples: (SAMPLE_RATE_HZ as f32
                    * (speaker_cfg.min_utterance_ms as f32 / 1000.0))
                    .max(4800.0) as usize,
                ..StreamingDiarizerConfig::default()
            };
            StreamingSpeakerSegmenter::from_file(
                &speaker_cfg.model_path,
                speaker_cfg.intra_threads,
                streaming_cfg,
            )
            .ok()
        } else {
            None
        };

        // Optional preprocessing is loaded by ConfigAudio after the input choice is known.
        let preprocessing = AudioPreprocessor::None;
        let preprocessing_mode = AudioPreprocessing::None;
        Ok(Self {
            vad,
            endpoint,
            endpoint_config,
            base_endpoint_config: endpoint_config,
            preprocessing,
            preprocessing_mode,
            vocal_activity: VocalActivity::default(),
            streaming_speaker,
            speaker_recognition_enabled: false,
            fixed_window: None,
            live_capture: None,
            audio: AudioBuffer::default(),
            speech_gate: SpeechGate::new(
                (endpoint_config.max_active_frames
                    + endpoint_config.pre_roll_frames
                    + endpoint_config.max_active_overlap_frames
                    + endpoint_config.opening_window_frames)
                    .max(FixedWindow::EXPANDED_WINDOW_SAMPLES / FRAME_SAMPLES),
            ),
            processed_samples: 0,
            vad_active: false,
            sentence_idle_frames: None,
            vad_transitions: Vec::new(),
            inference: NativeInference::new(model_plan, speaker)?,
        })
    }

    /// Accepts PCM16LE in the session's configured format; preparation feeds
    /// the same mono 16 kHz recognition path for every input owner.
    ///
    /// The network protocol does not prescribe a binary frame size.  This
    /// method therefore retains incomplete samples until a complete 512-sample
    /// Silero frame can be evaluated, rather than rejecting valid clients.
    pub(crate) fn push_pcm(&mut self, pcm: &[u8]) -> Result<Vec<PipelineEvent>, String> {
        validate_input_chunk_size(pcm.len())?;
        if !pcm.len().is_multiple_of(2) {
            return Err("PCM16LE audio must contain whole 16-bit samples".into());
        }
        let (source, detection) = self.prepare_audio(pcm)?;
        self.audio.push(&source, detection.as_deref());
        let frames = self.audio.take_ready();
        self.process_frames(frames)
    }

    fn prepare_audio(&mut self, pcm: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        match self.preprocessing.push(pcm) {
            Ok(prepared) => Ok(prepared),
            Err(error) if self.preprocessing_mode == AudioPreprocessing::Gtcrn => {
                // A failed recurrent enhancer cannot safely resume mid-stream.
                // Bypass for this session and realign all outstanding raw input.
                warn!("denoiser failed ({error}); using raw audio for detection");
                self.preprocessing = AudioPreprocessor::None;
                self.vad.reset();
                Ok((pcm.to_vec(), None))
            }
            Err(error) => Err(error),
        }
    }

    fn process_frames(&mut self, frames: AudioFrames) -> Result<Vec<PipelineEvent>, String> {
        let mut utterances = Vec::new();
        for (index, (raw_frame, denoised_frame)) in frames
            .raw
            .chunks_exact(FRAME_BYTES)
            .zip(frames.denoised.chunks_exact(FRAME_BYTES))
            .enumerate()
        {
            let real_samples = frames
                .real_samples
                .saturating_sub(index * FRAME_SAMPLES)
                .min(FRAME_SAMPLES);
            let padding = FRAME_SAMPLES - real_samples;
            let mut probability = self
                .vad
                .infer_pcm16le(denoised_frame)
                .map_err(|error| error.to_string())?;
            let raw_samples = decode_pcm16le_frame(raw_frame).map_err(|error| error.to_string())?;
            if self.preprocessing.is_vocal() {
                probability = self
                    .vocal_activity
                    .evidence(&raw_samples[..real_samples], probability);
            }
            let active = vad_is_active(probability, self.endpoint_config.speech_threshold);
            self.observe_vad_activity(active);

            if self.fixed_window.is_some() {
                self.speech_gate.observe(probability, real_samples);
                self.processed_samples = self.processed_samples.saturating_add(real_samples as u64);
                let window = self.fixed_window.as_mut().unwrap();
                let events = window.push(&raw_samples, active);
                let topic_turn_sequence = Some(window.current_topic_turn_sequence);
                for event in events {
                    match event {
                        FixedWindowEvent::Utterance(utterance) => {
                            let timed =
                                self.with_timeline(utterance, padding, topic_turn_sequence, None);
                            if let Some(event) = self.finish_utterance(timed) {
                                utterances.push(event);
                            }
                        }
                        FixedWindowEvent::StreamEnded => {
                            utterances.push(PipelineEvent::StreamEnded)
                        }
                    }
                }
                continue;
            }

            let denoised_samples =
                decode_pcm16le_frame(denoised_frame).map_err(|error| error.to_string())?;
            if self.speaker_recognition_enabled
                && !self.preprocessing.is_vocal()
                && let Some(streaming) = &mut self.streaming_speaker
            {
                if active {
                    match streaming.push_speech_samples(&denoised_samples) {
                        Ok(Some(StreamingSpeakerEvent::SpeakerCut {
                            previous_speaker, ..
                        })) => {
                            if let Some(utterance) = self.endpoint.split_on_speaker_change() {
                                // The speaker cut ends before the current frame.
                                let timed =
                                    self.with_timeline(utterance, 0, None, Some(previous_speaker));
                                if let Some(event) = self.finish_utterance(timed) {
                                    utterances.push(event);
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => warn!("streaming speaker recognition error: {error}"),
                    }
                } else {
                    streaming.observe_silence(denoised_samples.len());
                }
            }

            self.speech_gate.observe(probability, real_samples);
            let event = self
                .endpoint
                .push(&raw_samples, probability)
                .map_err(|error| error.to_string())?;
            self.processed_samples = self.processed_samples.saturating_add(real_samples as u64);
            if let Some(capture) = &self.live_capture {
                capture.push(&raw_samples[..real_samples])?;
            } else if self.inference.asr_streaming
                && !self.preprocessing.is_vocal()
                && self.speech_gate.allows_onset(
                    self.endpoint.active_frame_count() * FRAME_SAMPLES,
                    self.endpoint_config.speech_threshold,
                )
            {
                // Every live connector waits for the same speech evidence as
                // buffered recognition. Include the retained onset once ready.
                let (capture, input) = LiveCapture::begin(
                    self.endpoint_config,
                    &self.endpoint.active_samples()
                        [..self.endpoint.active_samples().len() - padding],
                    self.processed_samples,
                )?;
                self.live_capture = Some(capture);
                utterances.push(PipelineEvent::Streaming(input));
            }
            if let EndpointEvent::Finalized(utterance) = event {
                let speaker_id = if self.speaker_recognition_enabled {
                    self.streaming_speaker.as_mut().map(|streaming| {
                        streaming
                            .finalize_speech()
                            .unwrap_or_else(|| "speaker-unknown".into())
                    })
                } else {
                    None
                };
                let timed = self.with_timeline(utterance, padding, None, speaker_id);
                if let Some(event) = self.finish_utterance(timed) {
                    utterances.push(event);
                }
            }

            // Complete final ASR sentences translate independently of silence.
            // Idle still releases an unpunctuated final tail.
            if active {
                self.sentence_idle_frames = Some(0);
            } else if let Some(frames) = &mut self.sentence_idle_frames {
                *frames += 1;
                if *frames >= STREAM_END_FRAMES
                    && matches!(
                        self.endpoint.state(),
                        xrtranslate_vad::EndpointState::Listening
                    )
                {
                    self.sentence_idle_frames = None;
                    utterances.push(PipelineEvent::StreamEnded);
                }
            }
        }
        Ok(utterances)
    }

    /// Drains delayed enhancement and every remaining raw sample before EOF.
    pub(crate) fn flush(&mut self) -> Result<Vec<PipelineEvent>, String> {
        let (source, tail) = match self.preprocessing.finish() {
            Ok(tail) => tail,
            Err(error) if self.preprocessing_mode == AudioPreprocessing::Gtcrn => {
                warn!("denoiser failed to drain ({error}); using pending raw audio");
                self.preprocessing = AudioPreprocessor::None;
                self.vad.reset();
                (Vec::new(), None)
            }
            Err(error) => return Err(error),
        };
        self.audio.push(&source, Some(&[]));
        let frames = self.audio.finish(tail.as_deref());
        let padding = frames.padding_samples();
        let mut events = self.process_frames(frames)?;
        let (utterance, topic_turn_sequence, speaker_id) =
            if let Some(window) = &mut self.fixed_window {
                let sequence = window.current_topic_turn_sequence;
                (window.flush(), Some(sequence), None)
            } else {
                let speaker = if self.speaker_recognition_enabled {
                    self.streaming_speaker.as_mut().map(|streaming| {
                        streaming
                            .finalize_speech()
                            .unwrap_or_else(|| "speaker-unknown".into())
                    })
                } else {
                    None
                };
                (self.endpoint.flush(), None, speaker)
            };
        if let Some(utterance) = utterance {
            let timed = self.with_timeline(utterance, padding, topic_turn_sequence, speaker_id);
            if let Some(event) = self.finish_utterance(timed) {
                events.push(event);
            }
        }
        self.preprocessing.reset();
        self.vocal_activity.reset();
        self.vad.reset();
        self.speech_gate.reset();
        self.sentence_idle_frames = None;
        self.observe_vad_activity(false);
        Ok(events)
    }

    /// Drops buffered audio and recurrent VAD state after a route change.
    pub(crate) fn reset(&mut self) {
        self.live_capture = None;
        self.sentence_idle_frames = None;
        self.vad.reset();
        self.endpoint.reset();
        self.preprocessing.reset();
        self.vocal_activity.reset();
        self.audio.reset();
        self.speech_gate.reset();
        self.observe_vad_activity(false);
        if let Some(window) = &mut self.fixed_window {
            window.reset();
        }
        if let Some(streaming) = &mut self.streaming_speaker {
            streaming.reset();
        }
        self.processed_samples = 0;
    }

    pub(crate) fn set_speaker_recognition_enabled(&mut self, enabled: bool) {
        if self.speaker_recognition_enabled == enabled {
            return;
        }
        self.speaker_recognition_enabled = enabled;
        if let Some(streaming) = &mut self.streaming_speaker {
            streaming.reset();
        }
    }

    pub(crate) fn take_vad_transitions(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.vad_transitions)
    }

    fn observe_vad_activity(&mut self, active: bool) {
        if self.vad_active != active {
            self.vad_active = active;
            self.vad_transitions.push(active);
        }
    }

    pub(crate) fn configure_segmentation(
        &mut self,
        vad_threshold: Option<f32>,
        vad_silence_ms: Option<u32>,
        continuous_recognition: bool,
        audio_source: AudioSource,
    ) -> Result<(), String> {
        let mut config = self.base_endpoint_config;
        if let Some(threshold) = vad_threshold {
            config.speech_threshold = threshold;
        }
        if let Some(silence_ms) = vad_silence_ms {
            config.silence_frames_to_finalize = frames_for_ms(silence_ms);
        }
        self.base_endpoint_config = config;
        if self.preprocessing.is_vocal() {
            config = preprocessing::vocal_endpoint_config(config);
        }
        self.endpoint = EndpointDetector::new(config).map_err(|error| error.to_string())?;
        self.endpoint_config = config;
        self.speech_gate = SpeechGate::new(
            (config.max_active_frames
                + config.pre_roll_frames
                + config.max_active_overlap_frames
                + config.opening_window_frames)
                .max(FixedWindow::EXPANDED_WINDOW_SAMPLES / FRAME_SAMPLES),
        );
        // Pseudo-streaming is explicit; otherwise endpointing selects the model's
        // native stream or a complete utterance from its declared capabilities.
        self.fixed_window = (continuous_recognition && !self.preprocessing.is_vocal())
            .then(|| FixedWindow::new(audio_source));
        self.reset();
        Ok(())
    }

    pub(crate) fn configure_preprocessing(
        &mut self,
        selected: Option<AudioPreprocessing>,
        sample_rate: u32,
        channels: u16,
        workload: InferenceWorkload,
        config: &AppConfig,
        project_root: &Path,
        model_plan: &NativeProviderPlan,
    ) -> Result<(), String> {
        let mode = selected.unwrap_or(if config.denoise.enabled {
            AudioPreprocessing::Gtcrn
        } else {
            AudioPreprocessing::None
        });
        validate_preprocessing_format(mode, sample_rate, channels, workload)?;
        if mode == self.preprocessing_mode {
            return Ok(());
        }
        let next = match mode {
            AudioPreprocessing::None => AudioPreprocessor::None,
            AudioPreprocessing::Gtcrn => {
                let path = if config.denoise.model_path
                    == Path::new(xrtranslate_config::RuntimeLayout::DENOISE_MODEL_PATH)
                {
                    model_plan.preprocessing_model(xrtranslate_assets::ModelAssetId::GtcrnOnnx)?
                } else if config.denoise.model_path.is_absolute() {
                    config.denoise.model_path.clone()
                } else {
                    project_root.join(&config.denoise.model_path)
                };
                AudioPreprocessor::gtcrn(&path, config.denoise.intra_threads)?
            }
            AudioPreprocessing::VocalSeparation => {
                let path = model_plan
                    .preprocessing_model(xrtranslate_assets::ModelAssetId::UvrMdxnet9482Onnx)?;
                let cores = std::thread::available_parallelism().map_or(2, usize::from);
                AudioPreprocessor::vocals(&path, (cores / 2).clamp(1, 4))?
            }
        };
        self.preprocessing = next;
        self.preprocessing_mode = mode;
        Ok(())
    }

    pub(crate) fn observe_text_density(&mut self, units: usize) {
        if let Some(window) = &mut self.fixed_window {
            window.observe_text_density(units);
        }
    }

    /// Makes a request-capable handle for the session's bounded worker.
    pub(crate) fn inference(&self) -> NativeInference {
        self.inference.clone()
    }

    pub(crate) fn is_vocal(&self) -> bool {
        self.preprocessing.is_vocal()
    }

    fn finish_utterance(&mut self, timed: TimedUtterance) -> Option<PipelineEvent> {
        let valid = self
            .speech_gate
            .accept(&timed.utterance, self.endpoint_config.speech_threshold);
        if let Some(capture) = self.live_capture.take() {
            capture.finish(valid.then_some(timed));
            None
        } else {
            valid.then_some(PipelineEvent::Utterance(timed))
        }
    }

    fn with_timeline(
        &self,
        mut utterance: Utterance,
        trailing_padding_samples: usize,
        topic_turn_sequence: Option<u64>,
        speaker_id: Option<String>,
    ) -> TimedUtterance {
        let real_samples = utterance
            .samples
            .len()
            .saturating_sub(trailing_padding_samples) as u64;
        let end_samples = self.processed_samples;
        let start_samples = end_samples.saturating_sub(real_samples);
        utterance.samples.truncate(real_samples as usize);
        TimedUtterance {
            utterance,
            source_start_ms: samples_to_ms(start_samples),
            source_end_ms: samples_to_ms(end_samples),
            revisable: self.fixed_window.is_some(),
            topic_turn_sequence,
            speaker_id,
        }
    }
}

fn vad_is_active(probability: f32, threshold: f32) -> bool {
    probability >= threshold
}

impl NativeInference {
    pub(crate) async fn ask(&self, prompt: &str) -> Result<String, InferenceFailure> {
        self.translation
            .ask(
                prompt,
                self.translation_context_window_tokens,
                self.translation_max_output_tokens,
            )
            .await
            .map_err(|error| InferenceFailure::request("Ask failed", error))
    }

    pub(crate) fn for_diagnostics(model_plan: &NativeProviderPlan) -> Result<Self, String> {
        Self::new(model_plan, None)
    }

    fn new(
        model_plan: &NativeProviderPlan,
        speaker: Option<SpeakerInferenceConfig>,
    ) -> Result<Self, String> {
        Self::build(model_plan, speaker, true)
    }

    pub(crate) fn for_text(model_plan: &NativeProviderPlan) -> Result<Self, String> {
        Self::build(model_plan, None, false)
    }

    fn build(
        model_plan: &NativeProviderPlan,
        speaker: Option<SpeakerInferenceConfig>,
        audio: bool,
    ) -> Result<Self, String> {
        let asr = if audio {
            let asr_http = model_plan
                .asr_http_client()
                .map_err(|error| error.to_string())?;
            let asr = model_plan
                .asr_adapter(asr_http)
                .map_err(|error| error.to_string())?;
            Some(asr)
        } else {
            None
        };
        let translation_http = model_plan
            .translation_http_client()
            .map_err(|error| error.to_string())?;
        let translation = model_plan
            .translation_adapter(translation_http)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            languages: model_plan.language_capabilities,
            asr,
            asr_streaming: model_plan.asr_streaming(),
            asr_prompt: AsrPromptPolicy::new(
                model_plan.asr_prompt_mode(),
                model_plan.asr_context_max_chars(),
                model_plan.asr_supports_vocabulary_bias(),
                model_plan.asr_vocabulary_weight(),
            ),
            translation,
            translation_supports_reference_context: model_plan
                .translation_supports_reference_context(),
            asr_max_output_tokens: model_plan.asr_runtime().max_tokens,
            translation_max_output_tokens: model_plan.translation_runtime().max_tokens,
            asr_context_window_tokens: model_plan.asr_runtime().context_window_tokens,
            translation_context_window_tokens: model_plan
                .translation_runtime()
                .context_window_tokens,
            speaker,
        })
    }

    /// Opens one stateless embedding model and generation-local tracker on the
    /// bounded inference worker. Disabled configurations allocate nothing.
    pub(crate) fn speaker_diarizer(&self) -> Result<Option<OnlineSpeakerDiarizer>, String> {
        self.speaker
            .as_ref()
            .map(|config| {
                OnlineSpeakerDiarizer::from_file(
                    &config.model_path,
                    config.intra_threads,
                    config.tracker,
                )
                .map_err(|error| {
                    format!(
                        "cannot load speaker embedding model {}: {error}",
                        config.model_path.display()
                    )
                })
            })
            .transpose()
    }

    pub(crate) fn asr_supports_recognition_bias(&self) -> bool {
        self.asr_prompt.supports_recognition_bias()
    }

    pub(crate) fn speaker_min_utterance_ms(&self) -> Option<u32> {
        self.speaker.as_ref().map(|config| config.min_utterance_ms)
    }

    pub(crate) fn speaker_is_available(&self) -> bool {
        self.speaker.is_some()
    }

    pub(crate) fn prompt_context_token_budgets(&self) -> (usize, usize) {
        const REQUEST_RESERVE: u32 = 256;
        let available = |window: u32, output: u32| {
            usize::try_from(
                window
                    .saturating_sub(output)
                    .saturating_sub(REQUEST_RESERVE),
            )
            .unwrap_or(usize::MAX)
        };
        (
            available(self.asr_context_window_tokens, self.asr_max_output_tokens).min(384),
            available(
                self.translation_context_window_tokens,
                self.translation_max_output_tokens,
            ),
        )
    }

    pub(crate) async fn transcribe_stream<F, Fut>(
        &self,
        audio: tokio::sync::mpsc::Receiver<Vec<u8>>,
        source_language: &str,
        target_language: &str,
        prompt_graph: &PromptNodeGraph,
        context: &AsrPromptContext,
        on_update: F,
    ) -> Result<AsrTranscript, InferenceFailure>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
    {
        let delivery =
            self.asr_prompt
                .delivery(prompt_graph, source_language, target_language, context)?;
        self.asr
            .as_ref()
            .expect("audio session requires ASR")
            .transcribe_stream(
                audio,
                NativeAsrOptions {
                    language: asr_language(source_language),
                    instruction_prompt: delivery.instruction_prompt,
                    context_bias: delivery.context_bias,
                    vocabulary_bias: delivery.vocabulary_bias,
                    max_tokens: self.asr_max_output_tokens,
                },
                on_update,
            )
            .await
            .map_err(|error| InferenceFailure::request("Streaming ASR failed", error))
    }

    /// Accepts a live result or recognizes a completed window, then applies
    /// shared language routing, quality checks and sentence preparation.
    pub(crate) async fn transcribe(
        &self,
        samples: &[i16],
        source_language: &str,
        target_language: &str,
        asr_only: bool,
        adaptive_route: &mut AdaptiveLanguageRoute,
        prompt_graph: &PromptNodeGraph,
        mut prompt_context: AsrPromptContext,
        echo_candidates: &[String],
        streamed: Option<(AsrTranscript, Duration)>,
    ) -> Result<Option<RecognizedOutput>, InferenceFailure> {
        let pcm = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        let asr_started = Instant::now();
        let max_tokens = asr_max_tokens(samples.len()).min(self.asr_max_output_tokens);
        let selection = self
            .languages
            .select_with_options(source_language, target_language, None, asr_only)
            .map_err(InferenceFailure::runtime)?;
        adaptive_route.configure(source_language, target_language);
        adaptive_route.constrain(self.languages.sources());
        let active_targets = adaptive_route.active_targets(target_language);
        let delivery = self.asr_prompt.delivery(
            prompt_graph,
            source_language,
            &active_targets,
            &prompt_context,
        )?;
        let context_free_prompt = prompt_context.without_recognition_context();
        let context_free_delivery = self.asr_prompt.delivery(
            prompt_graph,
            source_language,
            &active_targets,
            &context_free_prompt,
        )?;
        let (streamed, stream_elapsed) = streamed
            .map_or((None, Duration::ZERO), |(result, elapsed)| {
                (Some(result), elapsed)
            });
        let auto_result = if let Some(transcript) = streamed {
            if transcript.text.trim().is_empty() {
                return Ok(None);
            }
            if is_probable_asr_hallucination(
                &transcript.text,
                samples.len(),
                SAMPLE_RATE_HZ,
                delivery.quality_context().as_deref(),
                echo_candidates,
            ) {
                self.transcribe_attempt(
                    &pcm,
                    samples.len(),
                    asr_language(source_language),
                    context_free_delivery.clone(),
                    context_free_delivery.clone(),
                    &[],
                    max_tokens,
                )
                .await?
            } else {
                Some(AsrAttemptResult {
                    transcript,
                    prompt_trace: delivery.prompt_trace.clone(),
                })
            }
        } else {
            self.transcribe_attempt(
                &pcm,
                samples.len(),
                asr_language(source_language),
                delivery.clone(),
                context_free_delivery.clone(),
                echo_candidates,
                max_tokens,
            )
            .await?
        };
        let Some(auto_result) = auto_result else {
            return Ok(None);
        };
        let direct_route = match selection.mode {
            LanguageMode::Fixed { source, target } => Some((Some(source), target)),
            LanguageMode::Detect { target } => {
                let source = auto_result
                    .transcript
                    .language
                    .as_deref()
                    .and_then(|labels| labels.split(',').find_map(SupportedLanguage::parse));
                if let Some(source) = source {
                    self.languages
                        .select(source.code(), target.code())
                        .map_err(InferenceFailure::runtime)?;
                }
                Some((source, target))
            }
            LanguageMode::AsrOnly { source } => {
                let source = source.or_else(|| {
                    auto_result
                        .transcript
                        .language
                        .as_deref()
                        .and_then(SupportedLanguage::parse)
                });
                let target = source
                    .or_else(|| self.languages.recognition_sources().iter().next())
                    .ok_or_else(|| {
                        InferenceFailure::runtime("No recognition language is available")
                    })?;
                Some((source, target))
            }
            LanguageMode::Bidirectional(_) => None,
        };
        let (result, source, target, route_switched) = if let Some((source, target)) = direct_route
        {
            (auto_result, source, target, None)
        } else {
            let (result, route, switched) = match adaptive_route.classify(
                auto_result.transcript.language.as_deref(),
                &auto_result.transcript.text,
            ) {
                AutoDecision::Accept(route) => (auto_result, route, None),
                AutoDecision::Switched { route, active } => {
                    (auto_result, route, Some(active.target_lang()))
                }
                AutoDecision::Retry {
                    language,
                    candidate: _,
                } => {
                    prompt_context.recent_phrase.clear();
                    warn!(
                        detected_language = auto_result
                            .transcript
                            .language
                            .as_deref()
                            .unwrap_or("unknown"),
                        retry_language = language.name(),
                        "ASR result needs constrained recovery"
                    );
                    let forced_delivery = self.asr_prompt.delivery(
                        prompt_graph,
                        language.code(),
                        &active_targets,
                        &prompt_context,
                    )?;
                    let forced_context_free_delivery = self.asr_prompt.delivery(
                        prompt_graph,
                        language.code(),
                        &active_targets,
                        &context_free_prompt,
                    )?;
                    let Some(mut forced) = self
                        .transcribe_attempt(
                            &pcm,
                            samples.len(),
                            Some(language.code().to_owned()),
                            forced_delivery,
                            forced_context_free_delivery,
                            echo_candidates,
                            max_tokens,
                        )
                        .await?
                    else {
                        return Ok(None);
                    };
                    let mut forced_language = language;
                    if !adaptive_route.evidence(forced_language, &forced.transcript.text) {
                        forced_language = adaptive_route.alternate(forced_language);
                        let alternate_delivery = self.asr_prompt.delivery(
                            prompt_graph,
                            forced_language.code(),
                            &active_targets,
                            &prompt_context,
                        )?;
                        let alternate_context_free_delivery = self.asr_prompt.delivery(
                            prompt_graph,
                            forced_language.code(),
                            &active_targets,
                            &context_free_prompt,
                        )?;
                        let Some(alternate) = self
                            .transcribe_attempt(
                                &pcm,
                                samples.len(),
                                Some(forced_language.code().to_owned()),
                                alternate_delivery,
                                alternate_context_free_delivery,
                                echo_candidates,
                                max_tokens,
                            )
                            .await?
                        else {
                            return Ok(None);
                        };
                        forced = alternate;
                    }
                    let route = adaptive_route.recovery(forced_language);
                    (forced, route, None)
                }
            };
            (result, Some(route.source), route.target, switched)
        };
        let AsrAttemptResult {
            transcript,
            prompt_trace,
        } = result;
        let asr_elapsed = stream_elapsed + asr_started.elapsed();
        let mut source_text = remove_asr_stutters(&transcript.text);
        if source_text.is_empty() {
            return Ok(None);
        }
        let source_code = source.map_or("auto", SupportedLanguage::code);
        if is_traditional_chinese(source_code) {
            source_text = to_traditional_chinese(&source_text);
        }
        Ok(Some(RecognizedOutput {
            segments: translation_segment_pairs_for_text_with_lang(&source_text, source_code),
            source_text,
            source_language: source_code.to_owned(),
            target_language: if asr_only {
                source_code.to_owned()
            } else {
                target.code().to_owned()
            },
            asr_elapsed,
            route_switched,
            prompt_trace,
        }))
    }

    async fn transcribe_attempt(
        &self,
        pcm: &[u8],
        sample_count: usize,
        language: Option<String>,
        delivery: AsrPromptDelivery,
        context_free_delivery: AsrPromptDelivery,
        echo_candidates: &[String],
        mut max_tokens: u32,
    ) -> Result<Option<AsrAttemptResult>, InferenceFailure> {
        let asr = self
            .asr
            .as_ref()
            .expect("ASR jobs require an audio session");
        for (retried, delivery) in [(false, delivery), (true, context_free_delivery)] {
            let quality_context = if retried {
                None
            } else {
                delivery.quality_context()
            };
            let transcript = match asr
                .transcribe_pcm16(
                    pcm,
                    NativeAsrOptions {
                        language: language.clone(),
                        instruction_prompt: delivery.instruction_prompt,
                        context_bias: delivery.context_bias,
                        vocabulary_bias: delivery.vocabulary_bias,
                        max_tokens,
                    },
                )
                .await
            {
                Ok(transcript) => transcript,
                Err(error) if error.is_output_limit() => {
                    if retried {
                        warn!(%error, max_tokens, sample_count,
                            "skipping ASR utterance after its retry reached the output limit");
                        return Ok(None);
                    }
                    let retry_max_tokens =
                        expanded_output_budget(max_tokens, self.asr_context_window_tokens);
                    warn!(%error, max_tokens, retry_max_tokens, sample_count,
                        "ASR output reached its limit; retrying the same audio without optional context");
                    max_tokens = retry_max_tokens;
                    continue;
                }
                Err(error) => {
                    return Err(InferenceFailure::request(
                        if retried {
                            "ASR context-free retry failed"
                        } else {
                            "ASR request failed"
                        },
                        error,
                    ));
                }
            };
            if !is_probable_asr_hallucination(
                &transcript.text,
                sample_count,
                SAMPLE_RATE_HZ,
                quality_context.as_deref(),
                if retried { &[] } else { echo_candidates },
            ) {
                return Ok(Some(AsrAttemptResult {
                    transcript,
                    prompt_trace: delivery.prompt_trace,
                }));
            }
            if retried {
                warn!("suppressing ASR output after context-free retry failed quality checks");
            } else {
                warn!("ASR output failed quality checks; retrying without optional context");
            }
        }
        Ok(None)
    }

    pub(crate) fn recognition_output(
        segment: &TranslationSegmentPair,
        source_language: &str,
    ) -> TranslationOutput {
        TranslationOutput {
            additional_source_text: None,
            additional_translations: Vec::new(),
            asr_only: true,
            source_text: segment.source_text.clone(),
            translated_text: segment.source_text.clone(),
            source_language: source_language.to_owned(),
            target_language: source_language.to_owned(),
            term_matches: Vec::new(),
            prompt_trace: None,
            mt_elapsed: Duration::ZERO,
        }
    }

    /// Translates one already-normalized, user-visible source segment.
    pub(crate) async fn translate_segment(
        &self,
        segment: &TranslationSegmentPair,
        source_language: &str,
        target_language: &str,
        prompt_graph: PromptNodeGraph,
        prompt_context: TranslationPromptContext,
    ) -> Result<TranslationOutput, InferenceFailure> {
        self.translate_segment_streaming(
            segment,
            source_language,
            target_language,
            prompt_graph,
            prompt_context,
            |_| std::future::ready(()),
            |_| {},
        )
        .await
    }

    pub(crate) async fn translate_segment_streaming<F, Fut, T>(
        &self,
        segment: &TranslationSegmentPair,
        source_language: &str,
        target_language: &str,
        prompt_graph: PromptNodeGraph,
        prompt_context: TranslationPromptContext,
        mut on_update: F,
        on_trace: T,
    ) -> Result<TranslationOutput, InferenceFailure>
    where
        F: FnMut(String) -> Fut + Send,
        Fut: Future<Output = ()> + Send,
        T: Fn(&PromptExecutionTrace) + Send + Sync,
    {
        self.languages
            .for_text()
            .select(source_language, target_language)
            .map_err(InferenceFailure::runtime)?;
        let route = translation_route(source_language, target_language);
        if route.source_code == "zh" && is_traditional_chinese(&route.target_code) {
            let mt_started = Instant::now();
            let converted = to_traditional_chinese(&segment.translation_text);
            on_update(converted.clone()).await;
            return Ok(TranslationOutput {
                additional_source_text: None,
                additional_translations: Vec::new(),
                asr_only: false,
                source_text: segment.source_text.clone(),
                translated_text: converted,
                source_language: route.source_code,
                target_language: route.target_code,
                term_matches: Vec::new(),
                prompt_trace: None,
                mt_elapsed: mt_started.elapsed(),
            });
        }
        let mut options = TranslationOptions::new(route.source.clone(), route.target.clone());
        options.prompt_graph = prompt_graph;
        options.context_window_tokens = self.translation_context_window_tokens;
        options.max_tokens = self.translation_max_output_tokens;
        options.prompt_context = if self.translation_supports_reference_context {
            prompt_context
        } else {
            prompt_context.without_reference_context()
        };
        let mt_started = Instant::now();
        let mut output_retried = false;
        let translated = loop {
            match self
                .translation
                .translate_streaming(
                    &segment.translation_text,
                    options.clone(),
                    |text| {
                        on_update(if is_traditional_chinese(&route.target_code) {
                            to_traditional_chinese(&text)
                        } else {
                            text
                        })
                    },
                    &on_trace,
                )
                .await
            {
                Ok(translated) => break translated,
                Err(error)
                    if !output_retried
                        && (error.is_rejected_output() || error.is_output_limit()) =>
                {
                    if error.is_output_limit() {
                        options.max_tokens = expanded_output_budget(
                            options.max_tokens,
                            options.context_window_tokens,
                        );
                    }
                    warn!(
                        %error,
                        max_tokens = options.max_tokens,
                        "translation output was incomplete or invalid; regenerating current segment once without optional context"
                    );
                    output_retried = true;
                    options.prompt_context = options.prompt_context.without_reference_context();
                    on_update(String::new()).await;
                }
                Err(error) if error.is_rejected_output() => {
                    on_update(String::new()).await;
                    warn!(
                        %error,
                        "suppressing translation after regenerated output failed prompt-aware quality checks"
                    );
                    return Err(InferenceFailure::runtime(format!(
                        "translation output remained invalid after one regeneration: {error}"
                    )));
                }
                Err(error) => {
                    on_update(String::new()).await;
                    let context = if output_retried {
                        "translation retry failed"
                    } else {
                        "translation request failed"
                    };
                    return Err(InferenceFailure::request(context, error));
                }
            }
        };

        let mut final_translated_text = translated.text;
        if is_traditional_chinese(&route.target_code) {
            final_translated_text = to_traditional_chinese(&final_translated_text);
        }

        Ok(TranslationOutput {
            additional_source_text: None,
            additional_translations: Vec::new(),
            asr_only: false,
            source_text: segment.source_text.clone(),
            translated_text: final_translated_text,
            source_language: route.source_code,
            target_language: route.target_code,
            term_matches: Vec::new(),
            prompt_trace: Some(translated.prompt_trace),
            mt_elapsed: mt_started.elapsed(),
        })
    }
}

fn asr_max_tokens(sample_count: usize) -> u32 {
    let seconds = sample_count as f64 / f64::from(SAMPLE_RATE_HZ);
    ((seconds * 18.0).ceil() as u32).saturating_add(16).max(24)
}

fn expanded_output_budget(current: u32, context_window: u32) -> u32 {
    // Leave half the window for input, without lowering a larger configured budget.
    current
        .saturating_mul(2)
        .min(context_window / 2)
        .max(current)
}

fn samples_to_ms(samples: u64) -> f64 {
    samples as f64 * 1_000.0 / f64::from(SAMPLE_RATE_HZ)
}

fn frames_for_ms(milliseconds: u32) -> usize {
    let samples = u64::from(milliseconds) * u64::from(SAMPLE_RATE_HZ);
    let frame_samples = FRAME_SAMPLES as u64 * 1_000;
    usize::try_from(samples.div_ceil(frame_samples))
        .unwrap_or(usize::MAX)
        .max(1)
}

/// The final source and target names for a translation request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TranslationRoute {
    source: String,
    target: String,
    source_code: String,
    target_code: String,
}

fn asr_language(source_language: &str) -> Option<String> {
    let source = normalized_code(source_language);
    (source != "auto").then_some(source)
}

fn translation_route(source_language: &str, target_language: &str) -> TranslationRoute {
    let source = normalized_code(source_language);
    let target = target_language
        .split(',')
        .next()
        .map(normalized_code)
        .unwrap_or_default();
    TranslationRoute {
        source: language_name(&source).to_owned(),
        target: language_name(&target).to_owned(),
        source_code: source,
        target_code: target,
    }
}

fn normalized_code(value: &str) -> String {
    SupportedLanguage::parse(value)
        .map(|language| language.code().to_owned())
        .unwrap_or_else(|| value.trim().to_ascii_lowercase().replace('_', "-"))
}

fn language_name(code: &str) -> &str {
    if let Some(language) = SupportedLanguage::from_code(code) {
        return language.name();
    }
    match code {
        "auto" => "automatically detected language",
        _ => code,
    }
}

/// Ensures the wire stream is compatible with the initial no-resample path.
pub(crate) fn validate_input_sample_rate(sample_rate: u32) -> Result<(), String> {
    if sample_rate == SAMPLE_RATE_HZ {
        Ok(())
    } else {
        Err(format!(
            "the native ASR route requires {SAMPLE_RATE_HZ} Hz mono PCM; received {sample_rate} Hz"
        ))
    }
}

fn validate_preprocessing_format(
    mode: AudioPreprocessing,
    sample_rate: u32,
    channels: u16,
    workload: InferenceWorkload,
) -> Result<(), String> {
    if mode == AudioPreprocessing::VocalSeparation {
        if workload != InferenceWorkload::Offline {
            return Err("Vocal separation is available for finite audio tasks only.".into());
        }
        if sample_rate != xrtranslate_separation::SAMPLE_RATE_HZ || channels != 2 {
            return Err("Vocal separation requires 44100 Hz stereo PCM.".into());
        }
        Ok(())
    } else {
        validate_input_sample_rate(sample_rate)?;
        if channels != 1 {
            return Err("Speech input requires mono PCM.".into());
        }
        Ok(())
    }
}

/// Rejects an oversized WebSocket audio message before it reaches the VAD
/// working buffers.  The transport remains free to send arbitrary frame sizes
/// below this limit.
pub(crate) fn validate_input_chunk_size(bytes: usize) -> Result<(), String> {
    if bytes <= MAX_INPUT_PCM_BYTES {
        Ok(())
    } else {
        Err(format!(
            "PCM WebSocket message is {bytes} bytes; the native backend limit is {MAX_INPUT_PCM_BYTES} bytes"
        ))
    }
}

#[cfg(test)]
mod preprocessing_policy_tests {
    use super::*;

    #[test]
    fn vocal_format_is_explicit_and_limited_to_finite_tasks() {
        assert!(
            validate_preprocessing_format(
                AudioPreprocessing::VocalSeparation,
                44_100,
                2,
                InferenceWorkload::Offline
            )
            .is_ok()
        );
        for (rate, channels, workload) in [
            (44_100, 2, InferenceWorkload::Realtime),
            (16_000, 1, InferenceWorkload::Offline),
            (44_100, 1, InferenceWorkload::Offline),
        ] {
            assert!(
                validate_preprocessing_format(
                    AudioPreprocessing::VocalSeparation,
                    rate,
                    channels,
                    workload
                )
                .is_err()
            );
        }
        for mode in [AudioPreprocessing::None, AudioPreprocessing::Gtcrn] {
            assert!(
                validate_preprocessing_format(mode, 16_000, 1, InferenceWorkload::Realtime).is_ok()
            );
            assert!(
                validate_preprocessing_format(mode, 44_100, 2, InferenceWorkload::Offline).is_err()
            );
        }
    }
}
