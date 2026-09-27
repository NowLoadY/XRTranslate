//! Provider-neutral voice selection for the TTS center.

use serde::{Deserialize, Serialize};

/// Query for a canonical mono PCM16 / 16 kHz reference body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceLanguage {
    pub language: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceTranscript {
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VoiceSelection {
    /// None selects the user's microphone reference.
    #[serde(default, alias = "builtin_id")]
    pub voice_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub provider: String,
    pub selection: VoiceSelection,
    pub ready: bool,
    pub personal_available: bool,
}
