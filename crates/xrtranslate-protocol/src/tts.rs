//! Provider-neutral speech synthesis and voice selection.

use serde::{Deserialize, Serialize};

/// Synthesize text as mono PCM16 WAV, using the selected voice by default.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechRequest {
    pub text: String,
    pub language: String,
    pub voice: Option<VoiceSelection>,
    /// Optional mono PCM16 WAV reference, encoded as Base64. Applies only to this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_audio: Option<String>,
}

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
    /// Local reference identity. With no preset or reference, use the microphone voice.
    #[serde(default, alias = "builtin_id")]
    pub voice_id: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub provider: String,
    pub selection: VoiceSelection,
    pub ready: bool,
    pub personal_available: bool,
    #[serde(default)]
    pub supports_cloning: bool,
    #[serde(default)]
    pub supports_reference_audio: bool,
    #[serde(default)]
    pub preset_voices: Vec<String>,
}
