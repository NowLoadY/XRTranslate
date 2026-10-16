//! Declarative online service catalogue and the wire contracts implemented by
//! XRTranslate. Provider names are configuration identities, never dispatch keys.

use crate::ModelCapability;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// A connector names a request/response format, independently of its host.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteConnector {
    OpenaiChat,
    OpenaiSpeech,
    GeminiSpeech,
    OpenaiTranscription,
    OpenaiAudioChat,
    DashscopeAsr,
    DashscopeAsrWebsocket,
}

impl RemoteConnector {
    pub fn capability(self) -> ModelCapability {
        match self {
            Self::OpenaiChat => ModelCapability::Translation,
            Self::OpenaiSpeech | Self::GeminiSpeech => ModelCapability::Tts,
            _ => ModelCapability::Asr,
        }
    }

    pub fn transport(self) -> &'static str {
        match self {
            Self::OpenaiChat
            | Self::OpenaiTranscription
            | Self::OpenaiAudioChat
            | Self::OpenaiSpeech => "openai",
            Self::GeminiSpeech => "gemini",
            Self::DashscopeAsr => "dashscope",
            Self::DashscopeAsrWebsocket => "websocket",
        }
    }

    /// Resolve old configurations once at the boundary, without vendor/model guesses.
    pub fn from_transport(section: &str, transport: &str, url: &str) -> Option<Self> {
        match (section, transport) {
            ("translation", "openai") => Some(Self::OpenaiChat),
            ("tts", "openai") => Some(Self::OpenaiSpeech),
            ("tts", "gemini") => Some(Self::GeminiSpeech),
            ("asr", "openai") => Some(
                if url
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .ends_with("/audio/transcriptions")
                {
                    Self::OpenaiTranscription
                } else {
                    Self::OpenaiAudioChat
                },
            ),
            ("asr", "dashscope") => Some(Self::DashscopeAsr),
            ("asr", "websocket") => Some(Self::DashscopeAsrWebsocket),
            _ => None,
        }
    }

    pub fn supports_prompt(self, mode: AsrPromptMode) -> bool {
        match mode {
            AsrPromptMode::None => true,
            AsrPromptMode::Instruction => self == Self::OpenaiAudioChat,
            AsrPromptMode::ContextBias => matches!(
                self,
                Self::OpenaiTranscription | Self::DashscopeAsr | Self::DashscopeAsrWebsocket
            ),
        }
    }

    pub fn supports_vocabulary(self) -> bool {
        matches!(self, Self::DashscopeAsr | Self::DashscopeAsrWebsocket)
    }

    pub fn supports_audio_streaming(self) -> bool {
        self == Self::DashscopeAsrWebsocket
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrPromptMode {
    #[default]
    None,
    Instruction,
    ContextBias,
}

/// Actual output supported by the application connector, not server marketing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteStreaming {
    #[default]
    None,
    Delta,
    Cumulative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrResponseFormat {
    Json,
    VerboseJson,
}

impl AsrResponseFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::VerboseJson => "verbose_json",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteCapabilities {
    pub streaming: RemoteStreaming,
    pub translation_prompt_profile: crate::TranslationPromptProfile,
    /// Supports freeform instructions in addition to translation.
    pub supports_chat: bool,
    pub supports_context: bool,
    pub asr_prompt_mode: AsrPromptMode,
    /// Override the transcription connector's optional language parameter.
    pub asr_language_hint: Option<bool>,
    /// Select richer transcription metadata when the model supports it.
    pub asr_response_format: Option<AsrResponseFormat>,
    pub context_max_chars: Option<usize>,
    pub vocabulary_bias: bool,
    pub single_user_message: bool,
    /// Optional text or audio chat-completions reasoning level.
    pub reasoning_effort: Option<String>,
}

/// Synthesis capabilities shared by catalogue, configuration and connectors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtsCapabilities {
    pub sample_rate: u32,
    pub max_input_chars: usize,
    #[serde(default)]
    pub voice_cloning: bool,
    pub voices: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteModelManifest {
    pub model: String,
    pub connector: RemoteConnector,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub language_aliases: Vec<(String, String)>,
    pub capabilities: RemoteCapabilities,
    #[serde(default)]
    pub native_text_polishing: Option<bool>,
    #[serde(default)]
    pub tts: Option<TtsCapabilities>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteServiceManifest {
    pub capability: ModelCapability,
    pub defaults: serde_json::Map<String, serde_json::Value>,
    pub models: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProviderManifest {
    pub id: String,
    pub guide_url: String,
    pub services: Vec<RemoteServiceManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCatalog {
    pub providers: Vec<RemoteProviderManifest>,
    pub models: Vec<RemoteModelManifest>,
}

pub fn catalog() -> &'static RemoteCatalog {
    static CATALOG: OnceLock<RemoteCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../remote_catalog.json"))
            .expect("invalid bundled remote model catalogue")
    })
}

pub fn remote_model(
    connector: RemoteConnector,
    model: &str,
) -> Option<&'static RemoteModelManifest> {
    catalog()
        .models
        .iter()
        .find(|card| card.connector == connector && card.model == model)
}

/// Built-in choices for one service, shared by welcome and catalogue consumers.
pub fn service(
    provider: &str,
    capability: ModelCapability,
) -> Option<&'static RemoteServiceManifest> {
    catalog()
        .providers
        .iter()
        .find(|entry| entry.id == provider)?
        .services
        .iter()
        .find(|service| service.capability == capability)
}
