//! Streaming media-audio decoding for meeting and media player imports.
//!
//! Files are decoded a packet at a time, routed to the requested channels,
//! continuously resampled, and emitted in small `Vec<f32>` chunks. The worker
//! never keeps the complete recording in memory.

mod api;
mod decode;
pub(crate) use xrtranslate_client_core::media_import::http;
mod mpv_extract;
#[cfg(feature = "mpv")]
mod mpv_stream;
#[cfg(test)]
mod online_tests;
#[cfg(test)]
use xrtranslate_client_core::media_import::discovery;
mod stream;
mod types;

pub use api::{AudioImportHandle, import_audio_file, import_audio_url};
#[allow(unused_imports)]
pub use types::{
    AudioFileInfo, AudioImportError, AudioImportEvent, AudioImportOptions, AudioImportOutcome,
    AudioImportPacing, AudioImportProgress, AudioImportStage, IMPORT_SAMPLE_RATE,
};
