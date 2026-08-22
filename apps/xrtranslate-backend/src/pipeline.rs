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

use std::{
    collections::VecDeque,
    mem,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::warn;

use xrtranslate_config::AppConfig;
use xrtranslate_denoise::GtcrnDenoiser;
use xrtranslate_engine::{
    TranslationSegmentPair, remove_asr_stutters, remove_transcript_overlap,
    strip_filler_edges_for_lang, translation_segment_pairs_for_final_text_with_lang,
};
use xrtranslate_inference::{
    AsrTranscript, InferenceError, ReqwestClient, TranslationAdapter, TranslationOptions,
    is_probable_asr_hallucination,
};
use xrtranslate_prompt::{
    AsrPromptContext, PromptExecutionTrace, PromptNodeGraph, TranslationPromptContext,
};
use xrtranslate_protocol::AudioSource;
use xrtranslate_speaker::{
    OnlineSpeakerDiarizer, StreamingDiarizerConfig, StreamingSpeakerEvent,
    StreamingSpeakerSegmenter, TrackerConfig,
};
use xrtranslate_vad::{
    EndpointConfig, EndpointDetector, EndpointEvent, FRAME_BYTES, FRAME_SAMPLES, SAMPLE_RATE_HZ,
    SileroVad, Utterance, UtteranceEndReason, decode_pcm16le_frame,
};

use crate::language::{
    AdaptiveLanguageRoute, AutoDecision, LanguageRoute, SupportedLanguage, is_traditional_chinese,
    to_traditional_chinese,
};
use crate::model_runtime::{NativeAsrAdapter, NativeAsrOptions, NativeProviderPlan};

use asr_prompt::{AsrPromptDelivery, AsrPromptPolicy};

const SILERO_VAD_MODEL: &str = "models/silero-vad/src/silero_vad/data/silero_vad.onnx";

#[derive(Debug)]
pub(crate) struct InferenceFailure {
    pub(crate) message: String,
    pub(crate) configuration_required: bool,
}

impl InferenceFailure {
    fn request(context: &str, error: InferenceError) -> Self {
        Self {
            configuration_required: error.requires_provider_configuration(),
            message: format!("{context}: {error}"),
        }
    }

    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            configuration_required: false,
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
    StreamEnded,
}

impl RecognizedOutput {
    pub(crate) fn apply_source_correction(&mut self, corrected: String) {
        if corrected == self.source_text {
            return;
        }
        self.source_text = corrected;
        self.segments = translation_segment_pairs_for_final_text_with_lang(
            &self.source_text,
            &self.source_language,
        );
    }

    pub(crate) fn prepare_revisable_snapshot(&mut self) {
        let cleaned = strip_filler_edges_for_lang(&self.source_text, &self.source_language);
        self.segments = vec![TranslationSegmentPair {
            source_text: self.source_text.clone(),
            translation_text: if cleaned.is_empty() {
                self.source_text.clone()
            } else {
                cleaned
            },
        }];
    }

    /// Removes text produced from the duplicated audio at a hard VAD boundary.
    /// Returns false when the new result contained only duplicated context.
    pub(crate) fn remove_overlap_with(&mut self, previous: &str) -> bool {
        self.source_text = remove_transcript_overlap(previous, &self.source_text);
        self.segments = translation_segment_pairs_for_final_text_with_lang(
            &self.source_text,
            &self.source_language,
        );
        !self.source_text.is_empty()
    }
}

/// A single provider translation emitted after a recognized source segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslationOutput {
    pub(crate) source_text: String,
    pub(crate) translated_text: String,
    pub(crate) source_language: String,
    pub(crate) target_language: String,
    pub(crate) term_matches: Vec<xrtranslate_protocol::CorpusTermMatch>,
    pub(crate) prompt_trace: Option<PromptExecutionTrace>,
    pub(crate) mt_elapsed: Duration,
}

/// Per-session state for the fully native default GGUF route.
pub(crate) struct NativePipeline {
    vad: SileroVad,
    endpoint: EndpointDetector,
    endpoint_config: EndpointConfig,
    denoiser: Option<GtcrnDenoiser>,
    streaming_speaker: Option<StreamingSpeakerSegmenter>,
    speaker_recognition_enabled: bool,
    fixed_window: Option<FixedWindow>,
    pending_pcm: Vec<u8>,
    pending_denoised_pcm: Vec<u8>,
    processed_samples: u64,
    vad_active: bool,
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
    const STREAM_END_FRAMES: usize = 62;

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
                    if self.idle_frames >= Self::STREAM_END_FRAMES {
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
            self.current_topic_turn_sequence = self.next_topic_turn_sequence;
            self.next_topic_turn_sequence = self
                .next_topic_turn_sequence
                .checked_add(1)
                .expect("continuous topic turn sequence overflow");
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
    asr: NativeAsrAdapter,
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
    /// Validates the selected native GGUF route and opens a stateful Silero model.
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
        let asr_http = model_plan
            .asr_http_client()
            .map_err(|error| error.to_string())?;
        let asr = model_plan
            .asr_adapter(asr_http)
            .map_err(|error| error.to_string())?;
        let translation_http = model_plan
            .translation_http_client()
            .map_err(|error| error.to_string())?;
        let translation = model_plan
            .translation_adapter(translation_http)
            .map_err(|error| error.to_string())?;
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

        let denoiser = if config.denoise.enabled {
            let model_path = if config.denoise.model_path.is_absolute() {
                config.denoise.model_path.clone()
            } else {
                project_root.join(&config.denoise.model_path)
            };
            if model_path.is_file() {
                match GtcrnDenoiser::from_file(&model_path, config.denoise.intra_threads) {
                    Ok(denoiser) => Some(denoiser),
                    Err(error) => {
                        warn!(
                            "failed to initialize GTCRN denoiser ({error}), continuing with bypass"
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            vad,
            endpoint,
            endpoint_config,
            denoiser,
            streaming_speaker,
            speaker_recognition_enabled: false,
            fixed_window: None,
            pending_pcm: Vec::new(),
            pending_denoised_pcm: Vec::new(),
            processed_samples: 0,
            vad_active: false,
            vad_transitions: Vec::new(),
            inference: NativeInference {
                asr,
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
            },
        })
    }

    /// Accepts arbitrary-sized mono PCM16LE transport chunks at 16 kHz.
    ///
    /// The network protocol does not prescribe a binary frame size.  This
    /// method therefore retains incomplete samples until a complete 512-sample
    /// Silero frame can be evaluated, rather than rejecting valid clients.
    pub(crate) fn push_pcm(&mut self, pcm: &[u8]) -> Result<Vec<PipelineEvent>, String> {
        validate_input_chunk_size(pcm.len())?;
        if !pcm.len().is_multiple_of(2) {
            return Err("PCM16LE audio must contain whole 16-bit samples".into());
        }
        self.pending_pcm.extend_from_slice(pcm);
        if let Some(denoiser) = self.denoiser.as_mut() {
            let denoised_pcm = match denoiser.process_pcm16le(pcm) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                _ => pcm.to_vec(),
            };
            self.pending_denoised_pcm.extend_from_slice(&denoised_pcm);
        } else {
            self.pending_denoised_pcm.extend_from_slice(pcm);
        }

        let complete_bytes =
            self.pending_pcm.len().min(self.pending_denoised_pcm.len()) / FRAME_BYTES * FRAME_BYTES;
        let raw_completed = self.pending_pcm.drain(..complete_bytes).collect::<Vec<_>>();
        let denoised_completed = self
            .pending_denoised_pcm
            .drain(..complete_bytes)
            .collect::<Vec<_>>();

        if self.fixed_window.is_some() {
            let mut utterances = Vec::new();
            for (raw_frame, denoised_frame) in raw_completed
                .chunks_exact(FRAME_BYTES)
                .zip(denoised_completed.chunks_exact(FRAME_BYTES))
            {
                let probability = self
                    .vad
                    .infer_pcm16le(denoised_frame)
                    .map_err(|error| error.to_string())?;
                let raw_samples =
                    decode_pcm16le_frame(raw_frame).map_err(|error| error.to_string())?;
                self.processed_samples =
                    self.processed_samples.saturating_add(FRAME_SAMPLES as u64);
                let threshold = self.endpoint_config.speech_threshold;
                let active = vad_is_active(probability, threshold);
                self.observe_vad_activity(active);
                let window_events = self
                    .fixed_window
                    .as_mut()
                    .map(|window| window.push(&raw_samples, active))
                    .unwrap_or_default();
                for event in window_events {
                    match event {
                        FixedWindowEvent::Utterance(utterance) => {
                            let topic_turn_sequence = self
                                .fixed_window
                                .as_ref()
                                .map(|window| window.current_topic_turn_sequence);
                            utterances.push(PipelineEvent::Utterance(self.with_timeline(
                                utterance,
                                0,
                                topic_turn_sequence,
                                None,
                            )))
                        }
                        FixedWindowEvent::StreamEnded => {
                            utterances.push(PipelineEvent::StreamEnded)
                        }
                    }
                }
            }
            return Ok(utterances);
        }

        let mut utterances = Vec::new();
        for (raw_frame, denoised_frame) in raw_completed
            .chunks_exact(FRAME_BYTES)
            .zip(denoised_completed.chunks_exact(FRAME_BYTES))
        {
            let probability = self
                .vad
                .infer_pcm16le(denoised_frame)
                .map_err(|error| error.to_string())?;
            let active = vad_is_active(probability, self.endpoint_config.speech_threshold);
            self.observe_vad_activity(active);
            let raw_samples = decode_pcm16le_frame(raw_frame).map_err(|error| error.to_string())?;
            let denoised_samples =
                decode_pcm16le_frame(denoised_frame).map_err(|error| error.to_string())?;

            if self.speaker_recognition_enabled
                && let Some(streaming) = &mut self.streaming_speaker
            {
                if active {
                    match streaming.push_speech_samples(&denoised_samples) {
                        Ok(Some(StreamingSpeakerEvent::SpeakerCut {
                            previous_speaker, ..
                        })) => {
                            if let Some(cut_utterance) = self.endpoint.split_on_speaker_change() {
                                utterances.push(PipelineEvent::Utterance(self.with_timeline(
                                    cut_utterance,
                                    0,
                                    None,
                                    Some(previous_speaker),
                                )));
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            warn!("streaming speaker recognition error: {error}");
                        }
                    }
                } else {
                    streaming.observe_silence(denoised_samples.len());
                }
            }

            let event = self
                .endpoint
                .push(&raw_samples, probability)
                .map_err(|error| error.to_string())?;
            self.processed_samples = self.processed_samples.saturating_add(FRAME_SAMPLES as u64);
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
                utterances.push(PipelineEvent::Utterance(
                    self.with_timeline(utterance, 0, None, speaker_id),
                ));
            }
        }
        Ok(utterances)
    }

    /// Completes the current utterance when a client ends a logical turn.
    pub(crate) fn flush(&mut self) -> Result<Option<PipelineEvent>, String> {
        if let Some(window) = &mut self.fixed_window {
            let utterance = window.flush();
            let event = utterance.map(|utterance| {
                let topic_turn_sequence = self
                    .fixed_window
                    .as_ref()
                    .map(|window| window.current_topic_turn_sequence);
                PipelineEvent::Utterance(self.with_timeline(
                    utterance,
                    0,
                    topic_turn_sequence,
                    None,
                ))
            });
            self.observe_vad_activity(false);
            return Ok(event);
        }
        let mut trailing_padding_samples = 0;
        let mut finalized = None;
        if !self.pending_pcm.is_empty() {
            let mut frame = mem::take(&mut self.pending_pcm);
            let received_samples = frame.len() / 2;
            trailing_padding_samples = FRAME_SAMPLES.saturating_sub(received_samples);
            frame.resize(FRAME_BYTES, 0);
            let probability = self
                .vad
                .infer_pcm16le(&frame)
                .map_err(|error| error.to_string())?;
            let event = self
                .endpoint
                .push_pcm16le(&frame, probability)
                .map_err(|error| error.to_string())?;
            if let EndpointEvent::Finalized(utterance) = event {
                finalized = Some(utterance);
            }
            self.processed_samples = self
                .processed_samples
                .saturating_add(received_samples as u64);
        }
        self.pending_denoised_pcm.clear();
        let speaker_id = if self.speaker_recognition_enabled {
            self.streaming_speaker.as_mut().map(|streaming| {
                streaming
                    .finalize_speech()
                    .unwrap_or_else(|| "speaker-unknown".into())
            })
        } else {
            None
        };
        let event = finalized
            .or_else(|| self.endpoint.flush())
            .map(|utterance| {
                PipelineEvent::Utterance(self.with_timeline(
                    utterance,
                    trailing_padding_samples,
                    None,
                    speaker_id,
                ))
            });
        self.observe_vad_activity(false);
        Ok(event)
    }

    /// Drops buffered audio and recurrent VAD state after a route change.
    pub(crate) fn reset(&mut self) {
        self.vad.reset();
        self.endpoint.reset();
        if let Some(denoiser) = &mut self.denoiser {
            denoiser.reset();
        }
        self.pending_pcm.clear();
        self.pending_denoised_pcm.clear();
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
        let mut config = self.endpoint_config;
        if let Some(threshold) = vad_threshold {
            config.speech_threshold = threshold;
        }
        if let Some(silence_ms) = vad_silence_ms {
            config.silence_frames_to_finalize = frames_for_ms(silence_ms);
        }
        self.endpoint = EndpointDetector::new(config).map_err(|error| error.to_string())?;
        self.endpoint_config = config;
        self.fixed_window = continuous_recognition.then(|| FixedWindow::new(audio_source));
        self.reset();
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

    fn with_timeline(
        &self,
        utterance: Utterance,
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

    /// Runs ASR for a complete utterance and applies the Python-compatible
    /// stutter removal and sentence/filler segmentation before any MT call.
    pub(crate) async fn transcribe(
        &self,
        samples: &[i16],
        source_language: &str,
        target_language: &str,
        adaptive_route: &mut AdaptiveLanguageRoute,
        prompt_graph: &PromptNodeGraph,
        prompt_context: AsrPromptContext,
        echo_candidates: &[String],
    ) -> Result<Option<RecognizedOutput>, InferenceFailure> {
        let pcm = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        let asr_started = Instant::now();
        let max_tokens = asr_max_tokens(samples.len()).min(self.asr_max_output_tokens);
        adaptive_route.configure(source_language, target_language);
        if source_language.eq_ignore_ascii_case("auto") && !adaptive_route.is_configured() {
            return Err(InferenceFailure::runtime(
                "Automatic input requires two different supported languages in the pair",
            ));
        }
        let active_targets = adaptive_route.active_targets(target_language);
        let delivery = self.asr_prompt.delivery(
            prompt_graph,
            source_language,
            &active_targets,
            &prompt_context,
        )?;
        let context_free_delivery = self.asr_prompt.delivery(
            prompt_graph,
            source_language,
            &active_targets,
            &AsrPromptContext::default(),
        )?;
        let Some(auto_result) = self
            .transcribe_attempt(
                &pcm,
                samples.len(),
                asr_language(source_language),
                delivery.clone(),
                context_free_delivery.clone(),
                echo_candidates,
                max_tokens,
            )
            .await?
        else {
            return Ok(None);
        };
        let explicit_route = explicit_language_route(source_language, target_language);
        let (result, route, route_switched) = if let Some(route) = explicit_route {
            (auto_result, route, None)
        } else {
            match adaptive_route.classify(
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
                    warn!(
                        detected_language = auto_result
                            .transcript
                            .language
                            .as_deref()
                            .unwrap_or("unknown"),
                        retry_language = language.model_name(),
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
                        &AsrPromptContext::default(),
                    )?;
                    let Some(mut forced) = self
                        .transcribe_attempt(
                            &pcm,
                            samples.len(),
                            Some(language.model_name().to_owned()),
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
                            &AsrPromptContext::default(),
                        )?;
                        let Some(alternate) = self
                            .transcribe_attempt(
                                &pcm,
                                samples.len(),
                                Some(forced_language.model_name().to_owned()),
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
            }
        };
        let AsrAttemptResult {
            transcript,
            prompt_trace,
        } = result;
        let asr_elapsed = asr_started.elapsed();
        let mut source_text = remove_asr_stutters(&transcript.text);
        if source_text.is_empty() {
            return Ok(None);
        }
        if is_traditional_chinese(route.source.code()) {
            source_text = to_traditional_chinese(&source_text);
        }
        Ok(Some(RecognizedOutput {
            segments: translation_segment_pairs_for_final_text_with_lang(
                &source_text,
                route.source.code(),
            ),
            source_text,
            source_language: route.source.code().to_owned(),
            target_language: route.target.code().to_owned(),
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
        max_tokens: u32,
    ) -> Result<Option<AsrAttemptResult>, InferenceFailure> {
        let quality_context = delivery.quality_context();
        let prompt_trace = delivery.prompt_trace.clone();
        let mut transcript = self
            .asr
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
            .map_err(|error| InferenceFailure::request("ASR request failed", error))?;

        if !is_probable_asr_hallucination(
            &transcript.text,
            sample_count,
            SAMPLE_RATE_HZ,
            quality_context.as_deref(),
            echo_candidates,
        ) {
            return Ok(Some(AsrAttemptResult {
                transcript,
                prompt_trace,
            }));
        }

        warn!("ASR output failed quality checks; retrying without optional context");
        let prompt_trace = context_free_delivery.prompt_trace.clone();
        transcript = self
            .asr
            .transcribe_pcm16(
                pcm,
                NativeAsrOptions {
                    language,
                    instruction_prompt: context_free_delivery.instruction_prompt,
                    context_bias: context_free_delivery.context_bias,
                    vocabulary_bias: context_free_delivery.vocabulary_bias,
                    max_tokens,
                },
            )
            .await
            .map_err(|error| InferenceFailure::request("ASR context-free retry failed", error))?;
        if is_probable_asr_hallucination(&transcript.text, sample_count, SAMPLE_RATE_HZ, None, &[])
        {
            warn!("suppressing ASR output after context-free retry failed quality checks");
            Ok(None)
        } else {
            Ok(Some(AsrAttemptResult {
                transcript,
                prompt_trace,
            }))
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
        let route = translation_route(source_language, target_language);
        if route.source_code == "zh" && is_traditional_chinese(&route.target_code) {
            let mt_started = Instant::now();
            let converted = to_traditional_chinese(&segment.translation_text);
            return Ok(TranslationOutput {
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
        if self.translation_supports_reference_context {
            options.prompt_context = prompt_context;
        }
        let mt_started = Instant::now();
        let mut context_window_retried = false;
        let mut rejected_output_retried = false;
        let translated = loop {
            match self
                .translation
                .translate(&segment.translation_text, options.clone())
                .await
            {
                Ok(translated) => break translated,
                Err(error)
                    if !context_window_retried
                        && options.prompt_context.has_reference_context()
                        && is_context_window_error(&error) =>
                {
                    warn!(
                        %error,
                        "translation context exceeded the provider window; retrying current segment without optional context"
                    );
                    context_window_retried = true;
                    options.prompt_context = TranslationPromptContext::default();
                }
                Err(error) if !rejected_output_retried && error.is_rejected_output() => {
                    warn!(
                        %error,
                        "translation output failed prompt-aware quality checks; regenerating current segment once"
                    );
                    rejected_output_retried = true;
                    options.prompt_context = TranslationPromptContext::default();
                }
                Err(error) if error.is_rejected_output() => {
                    warn!(
                        %error,
                        "suppressing translation after regenerated output failed prompt-aware quality checks"
                    );
                    return Err(InferenceFailure::runtime(format!(
                        "translation output remained invalid after one regeneration: {error}"
                    )));
                }
                Err(error) => {
                    let context = if rejected_output_retried || context_window_retried {
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

fn is_context_window_error(error: &InferenceError) -> bool {
    match error {
        InferenceError::HttpStatus {
            status,
            body_preview,
            ..
        } if *status == 400 => {
            let body = body_preview.to_ascii_lowercase();
            body.contains("exceed_context_size")
                || body.contains("exceeds the available context size")
                || body.contains("context window")
        }
        _ => false,
    }
}

fn asr_max_tokens(sample_count: usize) -> u32 {
    let seconds = sample_count as f64 / f64::from(SAMPLE_RATE_HZ);
    ((seconds * 18.0).ceil() as u32 + 16).clamp(24, 128)
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
    (source != "auto").then(|| language_name(&source).to_owned())
}

fn explicit_language_route(source: &str, target: &str) -> Option<LanguageRoute> {
    let source = SupportedLanguage::from_code(source)?;
    let target = target.split(',').find_map(SupportedLanguage::from_code)?;
    Some(LanguageRoute { source, target })
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
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    SupportedLanguage::from_code(&normalized)
        .map(|language| language.code().to_owned())
        .unwrap_or(normalized)
}

fn language_name(code: &str) -> &str {
    if let Some(language) = SupportedLanguage::from_code(code) {
        return language.model_name();
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
mod tests {
    use super::{
        FRAME_SAMPLES, FixedWindow, FixedWindowEvent, MAX_INPUT_PCM_BYTES, TimedUtterance,
        Utterance, UtteranceEndReason, asr_language, frames_for_ms, is_context_window_error,
        translation_route, vad_is_active, validate_input_chunk_size, validate_input_sample_rate,
    };
    use xrtranslate_inference::InferenceError;
    use xrtranslate_protocol::AudioSource;

    fn push_active_frames(window: &mut FixedWindow, frames: usize) -> Vec<FixedWindowEvent> {
        let frame = vec![1_i16; FRAME_SAMPLES];
        (0..frames)
            .flat_map(|_| window.push(&frame, true))
            .collect()
    }

    #[test]
    fn vad_activation_uses_only_the_model_probability() {
        assert!(!vad_is_active(0.49, 0.5));
        assert!(vad_is_active(0.5, 0.5));
    }

    #[test]
    fn explicit_language_is_used_for_asr_and_translation() {
        assert_eq!(asr_language("ja"), Some("Japanese".into()));
        assert_eq!(
            translation_route("ja", "en"),
            super::TranslationRoute {
                source: "Japanese".into(),
                target: "English".into(),
                source_code: "ja".into(),
                target_code: "en".into(),
            }
        );
        assert_eq!(asr_language("hi-IN"), Some("Hindi".into()));
        assert_eq!(
            translation_route("hi-IN", "vi-VN"),
            super::TranslationRoute {
                source: "Hindi".into(),
                target: "Vietnamese".into(),
                source_code: "hi".into(),
                target_code: "vi".into(),
            }
        );
    }

    #[test]
    fn initial_native_route_rejects_non_16khz_input() {
        assert!(validate_input_sample_rate(16_000).is_ok());
        assert!(validate_input_sample_rate(48_000).is_err());
    }

    #[test]
    fn input_audio_message_has_a_firm_memory_limit() {
        assert!(validate_input_chunk_size(MAX_INPUT_PCM_BYTES).is_ok());
        assert!(validate_input_chunk_size(MAX_INPUT_PCM_BYTES + 1).is_err());
    }

    #[test]
    fn llama_context_overflow_is_recognized_for_safe_retry() {
        assert!(is_context_window_error(&InferenceError::HttpStatus {
            endpoint: "http://127.0.0.1:8002".into(),
            status: 400,
            body_preview:
                r#"{"error":{"type":"exceed_context_size_error","message":"request exceeds the available context size"}}"#
                    .into(),
        }));
        assert!(!is_context_window_error(&InferenceError::HttpStatus {
            endpoint: "http://127.0.0.1:8002".into(),
            status: 500,
            body_preview: "internal error".into(),
        }));
    }

    #[test]
    fn endpoint_durations_round_up_to_complete_silero_frames() {
        assert_eq!(frames_for_ms(128), 4);
        assert_eq!(frames_for_ms(4_000), 125);
        assert_eq!(frames_for_ms(8_000), 250);
        assert_eq!(frames_for_ms(1), 1);
    }

    #[test]
    fn fixed_windows_warm_up_then_keep_one_second_of_overlap() {
        let mut window = FixedWindow::new(AudioSource::Microphone);
        let FixedWindowEvent::Utterance(first) = push_active_frames(&mut window, 62)
            .pop()
            .expect("first fixed window")
        else {
            panic!("expected utterance");
        };
        assert_eq!(first.samples.len(), FixedWindow::FIRST_WINDOW_SAMPLES);
        assert_eq!(first.overlap_frames, 0);
        let first_topic_turn = window.current_topic_turn_sequence;
        assert_eq!(
            window.samples.len(),
            FixedWindow::WINDOW_SAMPLES - FixedWindow::HOP_SAMPLES
        );

        let FixedWindowEvent::Utterance(second) = push_active_frames(&mut window, 62)
            .pop()
            .expect("steady fixed window")
        else {
            panic!("expected utterance");
        };
        assert_eq!(second.samples.len(), FixedWindow::WINDOW_SAMPLES);
        assert_eq!(second.overlap_frames, 32);
        assert_eq!(window.current_topic_turn_sequence, first_topic_turn);
        assert_eq!(
            window.samples.len(),
            FixedWindow::WINDOW_SAMPLES - FixedWindow::HOP_SAMPLES
        );
    }

    #[test]
    fn fixed_window_ignores_idle_audio_but_keeps_bounded_pre_roll() {
        let mut window = FixedWindow::new(AudioSource::Microphone);
        let frame = vec![0_i16; FRAME_SAMPLES];
        for _ in 0..100 {
            assert!(window.push(&frame, false).is_empty());
        }
        assert!(window.samples.is_empty());
        assert_eq!(window.pre_roll.len(), FixedWindow::PRE_ROLL_SAMPLES);
    }

    #[test]
    fn fixed_window_requires_sustained_activity_to_open() {
        let mut window = FixedWindow::new(AudioSource::Microphone);
        let active = vec![1_i16; FRAME_SAMPLES];
        let quiet = vec![0_i16; FRAME_SAMPLES];
        assert!(window.push(&active, true).is_empty());
        assert!(window.push(&quiet, false).is_empty());
        assert!(window.push(&active, true).is_empty());
        assert!(window.push(&active, true).is_empty());
        assert!(!window.gate_open);
        assert!(window.push(&active, true).is_empty());
        assert!(window.gate_open);
    }

    #[test]
    fn fixed_window_survives_brief_gaps_and_flushes_short_activity() {
        let mut window = FixedWindow::new(AudioSource::Microphone);
        let silence = vec![0_i16; FRAME_SAMPLES];
        let speech = vec![1_i16; FRAME_SAMPLES];
        for _ in 0..FixedWindow::PRE_ROLL_FRAMES {
            assert!(window.push(&silence, false).is_empty());
        }
        for _ in 0..FixedWindow::OPENING_FRAMES {
            assert!(window.push(&speech, true).is_empty());
        }
        for _ in 0..FixedWindow::RELEASE_FRAMES - 1 {
            assert!(window.push(&silence, false).is_empty());
        }
        assert!(window.gate_open);
        let events = window.push(&silence, false);
        let FixedWindowEvent::Utterance(utterance) = &events[0] else {
            panic!("expected utterance when the audio gate closes");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(utterance.end_reason, UtteranceEndReason::Silence);
        assert_eq!(
            utterance.samples.len(),
            (FixedWindow::PRE_ROLL_FRAMES + FixedWindow::RELEASE_FRAMES) * FRAME_SAMPLES
        );
        assert!(!window.gate_open);

        for _ in 0..FixedWindow::OPENING_FRAMES {
            assert!(window.push(&speech, true).is_empty());
        }
        assert!(window.gate_open);
        assert_eq!(window.current_topic_turn_sequence, 2);
    }

    #[test]
    fn fixed_window_ends_stream_without_retranscribing_overlap_only_audio() {
        let mut window = FixedWindow::new(AudioSource::Microphone);
        assert!(matches!(
            push_active_frames(&mut window, 62).as_slice(),
            [FixedWindowEvent::Utterance(_)]
        ));
        let silence = vec![0_i16; FRAME_SAMPLES];
        for _ in 0..FixedWindow::STREAM_END_FRAMES - 1 {
            assert!(window.push(&silence, false).is_empty());
        }
        assert!(matches!(
            window.push(&silence, false).as_slice(),
            [FixedWindowEvent::StreamEnded]
        ));
    }

    #[test]
    fn system_audio_context_expands_and_recovers_with_density_hysteresis() {
        let mut window = FixedWindow::new(AudioSource::SystemAudio);
        window.observe_text_density(2);
        assert_eq!(window.window_samples(), FixedWindow::WINDOW_SAMPLES);
        window.observe_text_density(3);
        assert_eq!(
            window.window_samples(),
            FixedWindow::EXPANDED_WINDOW_SAMPLES
        );
        window.observe_text_density(9);
        assert_eq!(
            window.window_samples(),
            FixedWindow::EXPANDED_WINDOW_SAMPLES
        );
        window.observe_text_density(10);
        assert_eq!(window.window_samples(), FixedWindow::WINDOW_SAMPLES);
    }

    #[test]
    fn shrinking_expanded_context_trims_old_overlap_without_underflow() {
        let mut window = FixedWindow::new(AudioSource::SystemAudio);
        assert!(matches!(
            push_active_frames(&mut window, 62).as_slice(),
            [FixedWindowEvent::Utterance(_)]
        ));
        window.observe_text_density(2);
        window.observe_text_density(3);
        assert!(matches!(
            push_active_frames(&mut window, 94).as_slice(),
            [FixedWindowEvent::Utterance(_)]
        ));
        assert_eq!(window.overlap_samples, 64 * FRAME_SAMPLES);

        window.observe_text_density(9);
        window.observe_text_density(10);
        assert_eq!(window.overlap_samples, 32 * FRAME_SAMPLES);
        assert!(matches!(
            push_active_frames(&mut window, 62).as_slice(),
            [FixedWindowEvent::Utterance(_)]
        ));
    }

    #[test]
    fn expanded_context_keeps_the_warm_window_advancing() {
        let mut window = FixedWindow::new(AudioSource::SystemAudio);
        window.observe_text_density(2);
        window.observe_text_density(3);
        assert_eq!(
            window.window_samples(),
            FixedWindow::EXPANDED_WINDOW_SAMPLES
        );
        let events = push_active_frames(&mut window, 62);
        let [FixedWindowEvent::Utterance(utterance)] = events.as_slice() else {
            panic!("expected warm utterance");
        };
        assert_eq!(utterance.samples.len(), FixedWindow::FIRST_WINDOW_SAMPLES);
        assert_eq!(window.overlap_samples, 32 * FRAME_SAMPLES);
        assert_eq!(window.samples.len(), 32 * FRAME_SAMPLES);
    }

    #[test]
    fn timed_utterance_preserves_streaming_speaker_id_and_end_reason() {
        let utterance = Utterance {
            samples: vec![0; 512 * 4],
            pre_roll_frames: 1,
            overlap_frames: 0,
            trailing_silence_frames: 0,
            end_reason: UtteranceEndReason::SpeakerChange,
        };
        let timed = TimedUtterance {
            utterance,
            source_start_ms: 0.0,
            source_end_ms: 128.0,
            revisable: false,
            topic_turn_sequence: None,
            speaker_id: Some("speaker-01".to_string()),
        };
        assert_eq!(
            timed.utterance.end_reason,
            UtteranceEndReason::SpeakerChange
        );
        assert_eq!(timed.speaker_id.as_deref(), Some("speaker-01"));
        assert_eq!(timed.source_end_ms, 128.0);
    }
}
