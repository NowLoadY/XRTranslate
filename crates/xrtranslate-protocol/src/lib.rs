//! WebSocket wire contract shared by the desktop client and the native backend.
//!
//! JSON control messages use a stable `action` or `event` discriminator. Audio
//! frames are sent as raw binary WebSocket messages; see [`PcmFormat`] and
//! [`PcmFrame`] for their deliberately header-free representation.

#![forbid(unsafe_code)]

pub mod tts;

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
pub use xr_corpus_protocol::{CorpusRecognitionCorrection, CorpusTermMatch, CorpusTermSource};
use xrtranslate_prompt::{PromptExecutionTrace, PromptNodeGraph};

/// The current WebSocket contract version.
///
/// The legacy Python backend does not exchange this number on the wire yet.
/// It is exported so the Rust client and backend can reject incompatible peers
/// once a handshake is added without changing the individual DTOs.
pub const PROTOCOL_VERSION: u16 = 3;

const fn is_false(value: &bool) -> bool {
    !*value
}

/// The encoded sample format of every binary audio WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcmSampleFormat {
    /// A signed, little-endian 16-bit PCM sample.
    S16Le,
}

/// Capture route for source-aware stream buffering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSource {
    #[default]
    Microphone,
    SystemAudio,
}

/// Metadata negotiated out-of-band for a stream of raw PCM frames.
///
/// Client-to-server frames use the sample rate in [`EventControl::ConfigAudio`]
/// (normally 16 kHz). Server-to-client TTS frames use `audio.tts_sample_rate`
/// (normally 48 kHz). Both directions are mono signed 16-bit little-endian
/// PCM. No binary frame has a custom header or length prefix: one WebSocket
/// binary message is exactly one [`PcmFrame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_format: PcmSampleFormat,
}

impl PcmFormat {
    /// Mono signed-16-bit little-endian PCM at `sample_rate` Hz.
    pub const fn mono_s16le(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            channels: 1,
            sample_format: PcmSampleFormat::S16Le,
        }
    }

    /// Number of bytes in one interleaved sample frame.
    pub const fn bytes_per_sample_frame(self) -> usize {
        self.channels as usize * 2
    }
}

/// A raw WebSocket binary payload whose bytes are PCM16LE samples.
///
/// This wrapper has no `Serialize` implementation on purpose. Serializing it
/// as JSON would hide the protocol's binary-frame requirement (and commonly
/// turn PCM into base64 by accident).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame(Vec<u8>);

impl PcmFrame {
    /// Validates and owns a raw PCM frame for `format`.
    pub fn new(bytes: Vec<u8>, format: PcmFormat) -> Result<Self, PcmFrameError> {
        let frame_width = format.bytes_per_sample_frame();
        if format.sample_rate == 0 {
            return Err(PcmFrameError::ZeroSampleRate);
        }
        if format.channels == 0 {
            return Err(PcmFrameError::ZeroChannels);
        }
        if !bytes.len().is_multiple_of(frame_width) {
            return Err(PcmFrameError::PartialSampleFrame {
                bytes: bytes.len(),
                frame_width,
            });
        }
        Ok(Self(bytes))
    }

    /// Borrows the raw binary WebSocket payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns the binary WebSocket payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// Number of PCM sample frames contained in this message.
    pub fn sample_frames(&self, format: PcmFormat) -> usize {
        self.0.len() / format.bytes_per_sample_frame()
    }
}

/// A malformed raw binary PCM frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcmFrameError {
    ZeroSampleRate,
    ZeroChannels,
    PartialSampleFrame { bytes: usize, frame_width: usize },
}

impl fmt::Display for PcmFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSampleRate => {
                formatter.write_str("PCM sample rate must be greater than zero")
            }
            Self::ZeroChannels => {
                formatter.write_str("PCM channel count must be greater than zero")
            }
            Self::PartialSampleFrame { bytes, frame_width } => write!(
                formatter,
                "PCM payload has {bytes} bytes, which is not divisible by its {frame_width}-byte sample frame"
            ),
        }
    }
}

impl Error for PcmFrameError {}

/// All JSON controls sent from a WebSocket client to the backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientControl {
    Action(ActionControl),
    Event(EventControl),
}

/// One immutable graph contains every recognition mode and provider page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptGraphSet {
    pub graph: PromptNodeGraph,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PromptGraphSetWire {
    Current {
        graph: PromptNodeGraph,
    },
    ModeSeparated {
        ordinary: PromptNodeGraph,
        pseudo_streaming: PromptNodeGraph,
    },
    ModeAndDomainSeparated {
        ordinary_translation: PromptNodeGraph,
        ordinary_asr: PromptNodeGraph,
        pseudo_streaming_translation: PromptNodeGraph,
        pseudo_streaming_asr: PromptNodeGraph,
    },
    Bare(PromptNodeGraph),
}

impl<'de> Deserialize<'de> for PromptGraphSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match PromptGraphSetWire::deserialize(deserializer)? {
            PromptGraphSetWire::Current { graph } => Self { graph },
            PromptGraphSetWire::ModeSeparated {
                ordinary,
                pseudo_streaming,
            } => Self {
                graph: PromptNodeGraph::merge_complete_mode_graphs(ordinary, &pseudo_streaming),
            },
            PromptGraphSetWire::ModeAndDomainSeparated {
                ordinary_translation,
                ordinary_asr,
                pseudo_streaming_translation,
                pseudo_streaming_asr,
            } => {
                let pages = [
                    xrtranslate_prompt::PromptNodePage::AsrInstruction,
                    xrtranslate_prompt::PromptNodePage::AsrContextBias,
                ];
                let mut ordinary = ordinary_translation;
                ordinary.replace_provider_pages(&ordinary_asr, &pages);
                let mut pseudo_streaming = pseudo_streaming_translation;
                pseudo_streaming.replace_provider_pages(&pseudo_streaming_asr, &pages);
                Self {
                    graph: PromptNodeGraph::merge_complete_mode_graphs(ordinary, &pseudo_streaming),
                }
            }
            PromptGraphSetWire::Bare(graph) => Self {
                graph: PromptNodeGraph::unify_mode_graphs(
                    graph,
                    &PromptNodeGraph::builtin_pseudo_streaming(),
                ),
            },
        })
    }
}

/// Independent output intent, captured together with the primary language route.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_target_lang: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub asr_only: bool,
}
impl TranslationOptions {
    pub fn is_default(&self) -> bool {
        self.additional_target_lang.is_none() && !self.asr_only
    }
}

/// A completed translation of the same source with its own target and context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdditionalTranslation {
    pub target_lang: String,
    pub translated_text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub term_matches: Vec<CorpusTermMatch>,
}

/// Action-discriminated client controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionControl {
    /// Updates the active translation route. It is also the initial route
    /// supplied immediately after a WebSocket connection opens.
    SessionConfig {
        source_lang: String,
        target_lang: String,
        #[serde(default, skip_serializing_if = "TranslationOptions::is_default")]
        translation_options: TranslationOptions,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<u32>,
        #[serde(
            default,
            alias = "prompt_graph",
            skip_serializing_if = "Option::is_none"
        )]
        prompt_graphs: Option<PromptGraphSet>,
    },
    /// Replaces the unified graph for future inference requests.
    #[serde(alias = "set_prompt_graph")]
    SetPromptGraphs {
        #[serde(alias = "prompt_graph")]
        prompt_graphs: PromptGraphSet,
    },
    /// Enables or disables a session feature.
    ToggleFeature { feature: Feature, enabled: bool },
    /// Submits a direct text turn for standard translation processing (segmenting,
    /// XR Corpus terminology matching & context retrieval, prompt graph execution,
    /// translation model inference, terminology post-rewriting, and history commit).
    TranslateText {
        text: String,
        #[serde(default, skip_serializing_if = "TranslationOptions::is_default")]
        translation_options: TranslationOptions,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_lang: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_lang: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream_id: Option<u64>,
    },
    /// Arms one bounded voice-cloning capture for this audio session.
    BeginVoiceClone,
}

/// Event-discriminated client controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventControl {
    /// Sets the microphone PCM sample rate and, optionally, the route before
    /// binary audio is sent.
    ConfigAudio {
        sample_rate: u32,
        source_lang: String,
        target_lang: String,
        #[serde(default, skip_serializing_if = "TranslationOptions::is_default")]
        translation_options: TranslationOptions,
        #[serde(default, skip_serializing_if = "is_default_audio_source")]
        audio_source: AudioSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vad_threshold: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vad_silence_ms: Option<u32>,
        #[serde(default, skip_serializing_if = "is_false")]
        continuous_recognition: bool,
        #[serde(default, skip_serializing_if = "is_realtime_workload")]
        workload: InferenceWorkload,
    },
    /// Flushes the active turn and temporarily rejects further binary audio.
    /// The WebSocket, timeline, and speaker state remain alive.
    Pause,
    /// Allows binary audio to enter a paused pipeline again.
    Resume,
    /// Flushes the active turn and gracefully finishes this session after all
    /// queued inference results have been emitted.
    Finish,
    /// Signals that a finite input (for example, an imported audio file) has
    /// reached EOF. Its drain behavior is the same as [`Self::Finish`].
    InputEnded,
    /// Legacy graceful-finish spelling retained for older clients.
    Stop,
    /// Marks the beginning of microphone audio for a logical turn.
    TurnStarted { turn_id: String },
}

/// Scheduling intent for a stream. It affects admission and fairness, never
/// recognition or translation semantics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceWorkload {
    #[default]
    Realtime,
    Offline,
}

/// Session features that can be changed while connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Tts,
    SpeakerRecognition,
}

const fn is_default_audio_source(source: &AudioSource) -> bool {
    matches!(source, AudioSource::Microphone)
}

const fn is_realtime_workload(workload: &InferenceWorkload) -> bool {
    matches!(workload, InferenceWorkload::Realtime)
}

/// JSON events sent from the backend to a WebSocket client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    SessionReady(SessionReady),
    VadActivity(VadActivity),
    AsrResult(AsrResult),
    SourceSegmentReady(SourceSegmentReady),
    TranslationActivity(TranslationActivity),
    TranslationPreview(TranslationPreview),
    TranslationReady(TranslationReady),
    RecognitionStreamEnded(RecognitionStreamEnded),
    PipelineDrained(PipelineDrained),
    TtsFinished(TtsFinished),
    VoiceCloneState(VoiceCloneState),
    RouteChanged(RouteChanged),
    Error(ErrorEvent),
}

/// Actual translation inference, excluding recognition, queued work and cached results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationActivity {
    pub request_id: u64,
    pub active: bool,
}

/// Confirms that every inference result preceding a pause or terminal input
/// boundary has been placed on the ordered WebSocket output queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineDrained {
    pub reason: DrainReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainReason {
    Paused,
    Finished,
    InputEnded,
    /// A drain requested through the legacy `stop` control.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VadActivity {
    pub active: bool,
}

/// Identifies a newly-created backend session and its initial language route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReady {
    pub session_id: String,
    pub source_lang: String,
    pub target_lang: String,
    /// Actual TTS execution provider after backend warm-up. Older backends and
    /// sessions without TTS omit this diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_backend: Option<String>,
    /// Managed CUDA ABI used by the active TTS provider, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_cuda_version: Option<String>,
}

/// An incremental or completed ASR result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrResult {
    #[serde(rename = "type")]
    pub kind: AsrResultKind,
    pub text: String,
    pub delta: String,
    #[serde(default)]
    pub turn_id: String,
    /// Unix timestamp in seconds, as emitted by the existing backend.
    pub ts: Option<f64>,
}

/// The stability of an ASR result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrResultKind {
    Partial,
    Stable,
    Final,
    Blank,
}

/// Provenance of a source segment's time range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentTiming {
    /// Older producers did not describe timing provenance.
    #[default]
    Unknown,
    /// The range is the observed VAD utterance window.
    UtteranceWindow,
    /// The range was proportionally estimated within an utterance from text.
    EstimatedTextPartition,
    /// The range spans multiple recognition windows merged by the client.
    MergedWindows,
    /// The range came from an authored subtitle source such as SRT.
    Authored,
}

/// Why the recognition window ended at this boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentBoundary {
    #[default]
    Unknown,
    Silence,
    AdaptiveSilence,
    DurationLimit,
    SpeakerChange,
    InputBoundary,
}

/// A source-language segment placed on the translation queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSegmentReady {
    pub source_text: String,
    /// Prompt Studio execution that produced the ASR instruction or lexical
    /// context for this recognition window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_trace: Option<PromptExecutionTrace>,
    #[serde(default)]
    pub activation_matches: Vec<CorpusTermMatch>,
    #[serde(default)]
    pub context_matches: Vec<CorpusTermMatch>,
    pub turn_id: String,
    pub segment_index: u32,
    pub segment_count: u32,
    pub speaker_id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    #[serde(default)]
    pub timing: SegmentTiming,
    #[serde(default)]
    pub boundary: SegmentBoundary,
    /// True while this source snapshot can change, including an unfinished
    /// sentence retained across bounded audio chunks.
    pub revisable: bool,
    /// Fraction of this window which repeats audio from its predecessor.
    pub overlap_ratio: f32,
    /// This event contains the complete current logical-turn snapshot. A
    /// consumer must replace this turn's provisional value atomically instead
    /// of merging text or replacing other turns. A final (`revisable: false`)
    /// snapshot may finalize a provisional snapshot at the same revision.
    #[serde(default)]
    pub authoritative_snapshot: bool,
    /// Monotonic backend-issued revision of the logical live turn.
    #[serde(default)]
    pub revision: u64,
}

/// A replaceable display preview. It never commits history or triggers speech.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationPreview {
    pub source_text: String,
    pub translated_text: String,
    pub turn_id: String,
    pub segment_index: u32,
    pub revision: u64,
    pub speaker_id: String,
}

/// A completed translation and its latency information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationReady {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_translations: Vec<AdditionalTranslation>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub asr_only: bool,
    pub source_text: String,
    pub translated_text: String,
    #[serde(default)]
    pub term_matches: Vec<CorpusTermMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_trace: Option<PromptExecutionTrace>,
    pub turn_id: String,
    pub segment_index: u32,
    pub segment_count: u32,
    pub speaker_id: String,
    /// Absolute position inside the current audio epoch.
    #[serde(default)]
    pub source_start_ms: f64,
    /// Exclusive end position inside the current audio epoch.
    #[serde(default)]
    pub source_end_ms: f64,
    #[serde(default)]
    pub timing: SegmentTiming,
    #[serde(default)]
    pub boundary: SegmentBoundary,
    /// True when this result replaces the revisable tail of a continuous stream.
    pub revisable: bool,
    /// Fraction of this window which repeats audio from its predecessor.
    pub overlap_ratio: f32,
    /// True when source and translation are one versioned, authoritative
    /// snapshot of the current logical turn.
    #[serde(default)]
    pub authoritative_snapshot: bool,
    /// Must equal the source snapshot revision used for this translation.
    #[serde(default)]
    pub revision: u64,
    pub clone_audio_path: String,
    pub tts_audio_path: String,
    pub metrics: LatencyMetrics,
}

/// Marks the ordered end of a continuous recognition span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionStreamEnded {
    #[serde(default)]
    pub turn_id: String,
}

/// Latency values reported to the client in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyMetrics {
    #[serde(default)]
    pub queue_ms: u64,
    pub asr_ms: u64,
    pub mt_ms: u64,
    pub tts_ms: u64,
    #[serde(default)]
    pub total_ms: u64,
}

/// Signals that the preceding binary TTS audio has been fully sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsFinished {
    pub text: String,
}

/// Progress for the explicitly armed, single-use voice-cloning buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCloneState {
    pub state: VoiceClonePhase,
    pub collected_seconds: f32,
    pub required_seconds: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceClonePhase {
    Collecting,
    Registering,
    Ready,
    Failed,
}

/// A recoverable backend error for the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub message: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub configuration_required: bool,
}

/// Notifies the client that the active language route was dynamically adapted or updated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteChanged {
    pub source_lang: String,
    pub target_lang: String,
}
