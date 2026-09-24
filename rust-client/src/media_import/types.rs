use std::{fmt, io, path::PathBuf, time::Duration};

pub const IMPORT_SAMPLE_RATE: u32 = 16_000;

/// Controls how quickly decoded audio is delivered to the recognition session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioImportPacing {
    /// Match the recording's playback clock. This is safe with bounded or
    /// unbounded output channels and is the recommended/default backend mode.
    Realtime,
    /// Decode as quickly as the receiver consumes data. This mode requires a
    /// bounded output channel so a long recording cannot be queued in memory.
    AsFastAsPossible,
}

#[derive(Debug, Clone)]
pub struct AudioImportOptions {
    /// Number of mono 16 kHz frames per emitted message. 1600 is 100 ms.
    pub chunk_frames: usize,
    pub output_sample_rate: u32,
    pub pacing: AudioImportPacing,
    pub recognition_channels: Vec<usize>,
    pub gate_threshold_db: Option<f32>,
}

impl Default for AudioImportOptions {
    fn default() -> Self {
        Self {
            chunk_frames: 1_600,
            output_sample_rate: IMPORT_SAMPLE_RATE,
            pacing: AudioImportPacing::Realtime,
            recognition_channels: Vec::new(),
            gate_threshold_db: Some(crate::audio_processing::DEFAULT_GATE_THRESHOLD_DB),
        }
    }
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioImportStage {
    #[allow(dead_code)]
    Extracting,
    Recognizing,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct AudioImportProgress {
    pub stage: AudioImportStage,
    pub decoded_source_frames: u64,
    pub total_source_frames: Option<u64>,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub fraction: Option<f32>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AudioImportEvent {
    Started(AudioFileInfo),
    Progress(AudioImportProgress),
    Completed { output_frames: u64 },
    Stopped { output_frames: u64 },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioImportOutcome {
    Completed { output_frames: u64 },
    Stopped { output_frames: u64 },
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum AudioImportError {
    InvalidOptions(String),
    Io(io::Error),
    Unsupported(String),
    Decode(String),
    Resample(String),
    OutputClosed,
    WorkerPanicked,
    Cancelled,
}

impl fmt::Display for AudioImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOptions(message) => write!(formatter, "invalid import options: {message}"),
            Self::Io(error) => write!(formatter, "audio file I/O failed: {error}"),
            Self::Unsupported(message) => write!(formatter, "unsupported audio file: {message}"),
            Self::Decode(message) => write!(formatter, "audio decoding failed: {message}"),
            Self::Resample(message) => write!(formatter, "audio resampling failed: {message}"),
            Self::OutputClosed => formatter.write_str("audio consumer disconnected"),
            Self::WorkerPanicked => formatter.write_str("audio import worker panicked"),
            Self::Cancelled => formatter.write_str("audio import stopped"),
        }
    }
}

impl std::error::Error for AudioImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for AudioImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
