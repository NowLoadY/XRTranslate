//! Streaming media-audio decoding for meeting and media player imports.
//!
//! Files are decoded a packet at a time, routed to the requested channels,
//! continuously resampled, and emitted in small `Vec<f32>` chunks. The worker
//! never keeps the complete recording in memory.

mod api;
mod decode;
mod http;
mod links;
mod mpv_extract;
#[cfg(feature = "mpv")]
mod mpv_stream;
#[cfg(test)]
mod online_tests;
pub(crate) mod discovery;
mod stream;
mod types;

pub use api::{AudioImportHandle, import_audio_file, import_audio_url};
#[allow(unused_imports)]
pub use types::{
    AudioFileInfo, AudioImportError, AudioImportEvent, AudioImportOptions, AudioImportOutcome,
    AudioImportPacing, AudioImportProgress, AudioImportStage, IMPORT_SAMPLE_RATE,
};
