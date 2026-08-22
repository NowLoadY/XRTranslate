//! WebSocket wire contract shared by the desktop client and the native backend.
//!
//! JSON control messages use a stable `action` or `event` discriminator. Audio
//! frames are sent as raw binary WebSocket messages; see [`PcmFormat`] and
//! [`PcmFrame`] for their deliberately header-free representation.

#![forbid(unsafe_code)]

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

/// Action-discriminated client controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionControl {
    /// Updates the active translation route. It is also the initial route
    /// supplied immediately after a WebSocket connection opens.
    SessionConfig {
        source_lang: String,
        target_lang: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_graph: Option<PromptNodeGraph>,
    },
    /// Replaces the prompt composition for future translation requests without
    /// resetting the active audio recognition pipeline.
    SetPromptGraph { prompt_graph: PromptNodeGraph },
    /// Enables or disables a session feature.
    ToggleFeature { feature: Feature, enabled: bool },
    /// Submits a direct text turn for standard translation processing (segmenting,
    /// XR Corpus terminology matching & context retrieval, prompt graph execution,
    /// translation model inference, terminology post-rewriting, and history commit).
    TranslateText {
        text: String,
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
    TranslationReady(TranslationReady),
    RecognitionStreamEnded(RecognitionStreamEnded),
    PipelineDrained(PipelineDrained),
    TtsFinished(TtsFinished),
    VoiceCloneState(VoiceCloneState),
    RouteChanged(RouteChanged),
    Error(ErrorEvent),
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
    /// True when this segment belongs to a revisable continuous window.
    pub revisable: bool,
    /// Fraction of this window which repeats audio from its predecessor.
    pub overlap_ratio: f32,
}

/// A completed translation and its latency information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationReady {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_session_ready_defaults_tts_runtime_diagnostics() {
        let event: ServerEvent = serde_json::from_str(
            r#"{"action":"session_ready","data":{"session_id":"s1","source_lang":"en","target_lang":"zh"}}"#,
        )
        .unwrap();
        assert!(matches!(
            event,
            ServerEvent::SessionReady(SessionReady {
                tts_backend: None,
                tts_cuda_version: None,
                ..
            })
        ));
    }

    #[test]
    fn session_ready_reports_actual_cuda_runtime() {
        let event = ServerEvent::SessionReady(SessionReady {
            session_id: "s1".into(),
            source_lang: "en".into(),
            target_lang: "zh".into(),
            tts_backend: Some("cuda".into()),
            tts_cuda_version: Some("13.3".into()),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""tts_backend":"cuda""#));
        assert!(json.contains(r#""tts_cuda_version":"13.3""#));
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), event);
    }

    #[test]
    fn legacy_error_event_defaults_to_no_configuration_redirect() {
        let event: ServerEvent =
            serde_json::from_str(r#"{"action":"error","data":{"message":"temporary failure"}}"#)
                .unwrap();
        assert!(matches!(
            event,
            ServerEvent::Error(ErrorEvent {
                configuration_required: false,
                ..
            })
        ));
    }

    #[test]
    fn route_changed_event_matches_the_wire_shape() {
        let event = ServerEvent::RouteChanged(RouteChanged {
            source_lang: "auto".into(),
            target_lang: "ja,zh".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"action":"route_changed","data":{"source_lang":"auto","target_lang":"ja,zh"}}"#
        );
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), event);
    }

    #[test]
    fn session_config_serializes_to_the_legacy_json_shape() {
        let control = ClientControl::Action(ActionControl::SessionConfig {
            source_lang: "auto".into(),
            target_lang: "zh,en".into(),
            sample_rate: None,
            prompt_graph: None,
        });

        assert_eq!(
            serde_json::to_string(&control).unwrap(),
            r#"{"action":"session_config","source_lang":"auto","target_lang":"zh,en"}"#
        );
    }

    #[test]
    fn prompt_graph_controls_use_a_typed_json_object() {
        let graph = PromptNodeGraph::builtin_default();
        let control = ClientControl::Action(ActionControl::SetPromptGraph {
            prompt_graph: graph.clone(),
        });
        let json = serde_json::to_value(&control).unwrap();
        assert!(json["prompt_graph"].is_object());
        assert_eq!(
            serde_json::from_value::<ClientControl>(json).unwrap(),
            control
        );
    }

    #[test]
    fn config_audio_round_trips_as_an_event_control() {
        let json = r#"{"event":"config_audio","sample_rate":16000,"source_lang":"auto","target_lang":"zh,en"}"#;
        let control: ClientControl = serde_json::from_str(json).unwrap();

        assert_eq!(
            control,
            ClientControl::Event(EventControl::ConfigAudio {
                sample_rate: 16_000,
                source_lang: "auto".into(),
                target_lang: "zh,en".into(),
                audio_source: AudioSource::Microphone,
                vad_threshold: None,
                vad_silence_ms: None,
                continuous_recognition: false,
                workload: InferenceWorkload::Realtime,
            })
        );
        assert_eq!(serde_json::to_string(&control).unwrap(), json);
    }

    #[test]
    fn offline_workload_is_explicit_while_realtime_stays_wire_compatible() {
        let control = ClientControl::Event(EventControl::ConfigAudio {
            sample_rate: 16_000,
            source_lang: "ja".into(),
            target_lang: "zh".into(),
            audio_source: AudioSource::SystemAudio,
            vad_threshold: None,
            vad_silence_ms: None,
            continuous_recognition: false,
            workload: InferenceWorkload::Offline,
        });
        let json = serde_json::to_string(&control).unwrap();
        assert!(json.contains(r#""workload":"offline""#));
        assert_eq!(
            serde_json::from_str::<ClientControl>(&json).unwrap(),
            control
        );
    }

    #[test]
    fn speaker_recognition_has_an_independent_feature_toggle() {
        let control = ClientControl::Action(ActionControl::ToggleFeature {
            feature: Feature::SpeakerRecognition,
            enabled: true,
        });
        assert_eq!(
            serde_json::to_string(&control).unwrap(),
            r#"{"action":"toggle_feature","feature":"speaker_recognition","enabled":true}"#
        );
    }

    #[test]
    fn voice_clone_controls_and_progress_have_stable_wire_shapes() {
        assert_eq!(
            serde_json::to_string(&ClientControl::Action(ActionControl::BeginVoiceClone)).unwrap(),
            r#"{"action":"begin_voice_clone"}"#
        );
        let event = ServerEvent::VoiceCloneState(VoiceCloneState {
            state: VoiceClonePhase::Collecting,
            collected_seconds: 0.25,
            required_seconds: 0.5,
            message: None,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), event);
    }

    #[test]
    fn translate_text_action_has_stable_wire_shape() {
        let control = ClientControl::Action(ActionControl::TranslateText {
            text: "Hello world".into(),
            source_lang: Some("en".into()),
            target_lang: Some("zh".into()),
            stream_id: Some(42),
        });
        let json = serde_json::to_string(&control).unwrap();
        assert_eq!(
            json,
            r#"{"action":"translate_text","text":"Hello world","source_lang":"en","target_lang":"zh","stream_id":42}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientControl>(&json).unwrap(),
            control
        );

        let minimal = ClientControl::Action(ActionControl::TranslateText {
            text: "Hello".into(),
            source_lang: None,
            target_lang: None,
            stream_id: None,
        });
        let min_json = serde_json::to_string(&minimal).unwrap();
        assert_eq!(min_json, r#"{"action":"translate_text","text":"Hello"}"#);
        assert_eq!(
            serde_json::from_str::<ClientControl>(&min_json).unwrap(),
            minimal
        );
    }

    #[test]
    fn meeting_lifecycle_controls_have_stable_wire_shapes() {
        for (event, expected) in [
            (EventControl::Pause, r#"{"event":"pause"}"#),
            (EventControl::Resume, r#"{"event":"resume"}"#),
            (EventControl::Finish, r#"{"event":"finish"}"#),
            (EventControl::InputEnded, r#"{"event":"input_ended"}"#),
            (EventControl::Stop, r#"{"event":"stop"}"#),
        ] {
            let control = ClientControl::Event(event);
            assert_eq!(serde_json::to_string(&control).unwrap(), expected);
        }
    }

    #[test]
    fn pipeline_drained_reports_why_the_boundary_was_requested() {
        let event = ServerEvent::PipelineDrained(PipelineDrained {
            reason: DrainReason::InputEnded,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"action":"pipeline_drained","data":{"reason":"input_ended"}}"#
        );
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), event);
    }

    #[test]
    fn translation_event_matches_the_existing_backend_shape() {
        let json = r#"{
            "action":"translation_ready",
            "data":{
                "source_text":"hello",
                "translated_text":"你好",
                "turn_id":"native-1",
                "segment_index":1,
                "segment_count":1,
                "speaker_id":"",
                "revisable":false,
                "overlap_ratio":0.0,
                "clone_audio_path":"",
                "tts_audio_path":"",
                "metrics":{"asr_ms":12,"mt_ms":34,"tts_ms":0}
            }
        }"#;

        let event: ServerEvent = serde_json::from_str(json).unwrap();
        assert_eq!(
            event,
            ServerEvent::TranslationReady(TranslationReady {
                source_text: "hello".into(),
                translated_text: "你好".into(),
                term_matches: Vec::new(),
                prompt_trace: None,
                turn_id: "native-1".into(),
                segment_index: 1,
                segment_count: 1,
                speaker_id: String::new(),
                source_start_ms: 0.0,
                source_end_ms: 0.0,
                timing: SegmentTiming::Unknown,
                boundary: SegmentBoundary::Unknown,
                revisable: false,
                overlap_ratio: 0.0,
                clone_audio_path: String::new(),
                tts_audio_path: String::new(),
                metrics: LatencyMetrics {
                    queue_ms: 0,
                    asr_ms: 12,
                    mt_ms: 34,
                    tts_ms: 0,
                    total_ms: 0,
                },
            })
        );
    }

    #[test]
    fn translation_prompt_trace_round_trips_with_node_outputs() {
        let trace = PromptNodeGraph::builtin_default()
            .render_with_trace(
                xrtranslate_prompt::PromptProviderTarget::Hunyuan,
                "hello",
                "English",
                "Chinese",
                &xrtranslate_prompt::TranslationPromptContext::default(),
            )
            .unwrap()
            .trace;
        let event = ServerEvent::TranslationReady(TranslationReady {
            source_text: "hello".into(),
            translated_text: "你好".into(),
            term_matches: Vec::new(),
            prompt_trace: Some(trace.clone()),
            turn_id: "turn-1".into(),
            segment_index: 1,
            segment_count: 1,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 1.0,
            timing: SegmentTiming::Unknown,
            boundary: SegmentBoundary::Unknown,
            revisable: false,
            overlap_ratio: 0.0,
            clone_audio_path: String::new(),
            tts_audio_path: String::new(),
            metrics: LatencyMetrics {
                queue_ms: 0,
                asr_ms: 0,
                mt_ms: 1,
                tts_ms: 0,
                total_ms: 1,
            },
        });

        let json = serde_json::to_string(&event).unwrap();
        let decoded: ServerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
        assert!(json.contains(r#""node_id":"hunyuan-current-input""#));
    }

    #[test]
    fn segment_timing_and_boundary_have_stable_wire_values() {
        let event = ServerEvent::SourceSegmentReady(SourceSegmentReady {
            source_text: "hello".into(),
            prompt_trace: None,
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
            turn_id: "turn-1".into(),
            segment_index: 1,
            segment_count: 2,
            speaker_id: "speaker-01".into(),
            source_start_ms: 100.0,
            source_end_ms: 400.0,
            timing: SegmentTiming::EstimatedTextPartition,
            boundary: SegmentBoundary::SpeakerChange,
            revisable: false,
            overlap_ratio: 0.0,
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""timing":"estimated_text_partition""#));
        assert!(json.contains(r#""boundary":"speaker_change""#));
        assert_eq!(serde_json::from_str::<ServerEvent>(&json).unwrap(), event);
    }

    #[test]
    fn source_segment_prompt_trace_round_trips_with_asr_target() {
        let trace = PromptNodeGraph::builtin_default()
            .render_asr_with_trace(
                xrtranslate_prompt::PromptProviderTarget::AsrInstruction,
                "English",
                "Chinese",
                &xrtranslate_prompt::AsrPromptContext::default(),
            )
            .unwrap()
            .trace;
        let event = ServerEvent::SourceSegmentReady(SourceSegmentReady {
            source_text: "hello".into(),
            prompt_trace: Some(trace),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
            turn_id: "turn-1".into(),
            segment_index: 1,
            segment_count: 1,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 1.0,
            timing: SegmentTiming::Unknown,
            boundary: SegmentBoundary::Unknown,
            revisable: false,
            overlap_ratio: 0.0,
        });

        let json = serde_json::to_string(&event).unwrap();
        let decoded: ServerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
        assert!(json.contains(r#""target":"asr_instruction""#));
    }

    #[test]
    fn translation_snapshot_requires_current_stream_fields() {
        let json = r#"{
            "action":"translation_ready",
            "data":{
                "source_text":"hello",
                "translated_text":"你好",
                "turn_id":"native-1",
                "segment_index":1,
                "segment_count":1,
                "speaker_id":"",
                "clone_audio_path":"",
                "tts_audio_path":"",
                "metrics":{"asr_ms":12,"mt_ms":34,"tts_ms":0}
            }
        }"#;

        assert!(serde_json::from_str::<ServerEvent>(json).is_err());
    }

    #[test]
    fn pcm_frames_are_header_free_and_require_full_samples() {
        let format = PcmFormat::mono_s16le(16_000);
        let frame = PcmFrame::new(vec![0, 1, 2, 3], format).unwrap();
        assert_eq!(frame.as_bytes(), &[0, 1, 2, 3]);
        assert_eq!(frame.sample_frames(format), 2);
        assert!(matches!(
            PcmFrame::new(vec![0, 1, 2], format),
            Err(PcmFrameError::PartialSampleFrame { .. })
        ));
    }
}
