use std::{fmt, io};
pub use xrtranslate_client_core::media_import::*;

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
