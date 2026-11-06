pub mod discovery;
pub mod http;
pub mod links;
use std::{path::PathBuf, time::Duration};
pub const IMPORT_SAMPLE_RATE: u32 = 16_000;

/// Controls how quickly decoded audio is delivered to the recognition session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioImportPacing {
    /// Match the recording's playback clock. This is safe with bounded or
    /// unbounded output channels and is the recommended/default backend mode.
    Realtime,
    /// Decode as quickly as the receiver consumes data. This mode requires a
    /// bounded output channel so a long recording cannot be queued in memory.
    AsFastAsPossible,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioImportOptions {
    #[serde(default)]
    pub save_separated_audio: bool,
    /// Optional finite source interval. Output timestamps begin at zero.
    pub range_ms: Option<(u64, u64)>,
    /// Number of interleaved sample frames per emitted message.
    pub chunk_frames: usize,
    pub output_sample_rate: u32,
    /// Interleaved channel count delivered to the recognition session.
    pub output_channels: usize,
    pub pacing: AudioImportPacing,
    /// Explicit libmpv audio track id; select before decoding or preprocessing.
    pub audio_track: Option<i64>,
    pub recognition_channels: Vec<usize>,
    pub gate_threshold_db: Option<f32>,
}

impl Default for AudioImportOptions {
    fn default() -> Self {
        Self {
            save_separated_audio: false,
            range_ms: None,
            chunk_frames: 1_600,
            output_sample_rate: IMPORT_SAMPLE_RATE,
            output_channels: 1,
            pacing: AudioImportPacing::Realtime,
            audio_track: None,
            recognition_channels: Vec::new(),
            gate_threshold_db: Some(crate::audio_processing::DEFAULT_GATE_THRESHOLD_DB),
        }
    }
}

impl AudioImportOptions {
    /// Finite recordings run at the receiver's pace, with the selected processor's format.
    pub fn for_recognition(preprocessing: xrtranslate_protocol::AudioPreprocessing) -> Self {
        use xrtranslate_protocol::AudioPreprocessing;
        Self {
            pacing: AudioImportPacing::AsFastAsPossible,
            output_sample_rate: if preprocessing == AudioPreprocessing::VocalSeparation {
                44_100
            } else {
                IMPORT_SAMPLE_RATE
            },
            output_channels: if preprocessing == AudioPreprocessing::VocalSeparation {
                2
            } else {
                1
            },
            gate_threshold_db: (preprocessing == AudioPreprocessing::Gtcrn)
                .then_some(crate::audio_processing::DEFAULT_GATE_THRESHOLD_DB),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct AudioFileInfo {
    pub path: PathBuf,
    pub codec: String,
    pub source_sample_rate: u32,
    pub source_channels: usize,
    pub total_source_frames: Option<u64>,
    pub duration: Option<Duration>,
    pub output_sample_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioImportStage {
    #[allow(dead_code)]
    Extracting,
    Recognizing,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct AudioImportProgress {
    pub stage: AudioImportStage,
    pub decoded_source_frames: u64,
    pub total_source_frames: Option<u64>,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub fraction: Option<f32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub enum AudioImportEvent {
    Started(AudioFileInfo),
    Progress(AudioImportProgress),
    Completed {
        output_frames: u64,
    },
    Stopped {
        output_frames: u64,
    },
    /// The downstream session owns the reason for closing the audio channel.
    OutputClosed,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioImportOutcome {
    Completed { output_frames: u64 },
    Stopped { output_frames: u64 },
}

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma", "ape", "alac", "aiff", "aif", "oga",
];
