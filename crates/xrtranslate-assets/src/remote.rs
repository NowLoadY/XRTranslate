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
    DashscopeTranslation,
    ChatgptResponses,
    OpenaiSpeech,
    GeminiSpeech,
    MinimaxSpeech,
    DashscopeSpeech,
    OpenaiTranscription,
    OpenaiAudioChat,
    DashscopeAsr,
    DashscopeAsrWebsocket,
    DoubaoWebAsr,
}

/// User-facing connection channels are independent from the wire transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionChannel {
    Local,
    Api,
    Account,
}

pub use xrtranslate_connections::AccountProvider;

impl RemoteConnector {
    pub fn capability(self) -> ModelCapability {
        match self {
            Self::OpenaiChat | Self::DashscopeTranslation | Self::ChatgptResponses => {
                ModelCapability::Translation
            }
            Self::OpenaiSpeech
            | Self::GeminiSpeech
            | Self::MinimaxSpeech
            | Self::DashscopeSpeech => ModelCapability::Tts,
            _ => ModelCapability::Asr,
        }
    }

    pub fn transport(self) -> &'static str {
        match self {
            Self::OpenaiChat
            | Self::DashscopeTranslation
            | Self::ChatgptResponses
            | Self::OpenaiTranscription
            | Self::OpenaiAudioChat
            | Self::OpenaiSpeech => "openai",
            Self::GeminiSpeech => "gemini",
            Self::MinimaxSpeech => "minimax",
            Self::DashscopeAsr | Self::DashscopeSpeech => "dashscope",
            Self::DashscopeAsrWebsocket | Self::DoubaoWebAsr => "websocket",
        }
    }

    pub const fn account_provider(self) -> Option<AccountProvider> {
        match self {
            Self::DoubaoWebAsr => Some(AccountProvider::Doubao),
            Self::ChatgptResponses => Some(AccountProvider::Chatgpt),
            _ => None,
        }
    }

    pub const fn channel(self) -> ConnectionChannel {
        if self.account_provider().is_some() {
            ConnectionChannel::Account
        } else {
            ConnectionChannel::Api
        }
    }

    /// Resolve old configurations once at the boundary, without vendor/model guesses.
    pub fn from_transport(section: &str, transport: &str, url: &str) -> Option<Self> {
        match (section, transport) {
            ("translation", "openai") => Some(Self::OpenaiChat),
            ("tts", "openai") => Some(Self::OpenaiSpeech),
            ("tts", "gemini") => Some(Self::GeminiSpeech),
            ("tts", "minimax") => Some(Self::MinimaxSpeech),
            ("tts", "dashscope") => Some(Self::DashscopeSpeech),
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
        matches!(self, Self::DashscopeAsrWebsocket | Self::DoubaoWebAsr)
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
    /// Supports bounded, single-request text editing instructions.
    pub supports_ask: bool,
    /// Supports general conversation and decisions using supplied history.
    pub supports_chat: bool,
    /// Connector accepts JSON Schema output for structured inference.
    pub supports_json_output: bool,
    pub supports_context: bool,
    pub asr_prompt_mode: AsrPromptMode,
    /// Override the transcription connector's optional language parameter.
    pub asr_language_hint: Option<bool>,
    /// Select richer transcription metadata when the model supports it.
    pub asr_response_format: Option<AsrResponseFormat>,
    pub context_max_chars: Option<usize>,
    pub vocabulary_bias: bool,
    pub single_user_message: bool,
    /// Optional reasoning level; each connector maps it to its wire format.
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
    /// Clone a reference for one synthesis request without selecting a saved voice.
    #[serde(default)]
    pub reference_audio: bool,
    pub voices: Vec<String>,
    #[serde(default)]
    pub voice_languages: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub voice_labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteModelManifest {
    pub model: String,
    /// A service-managed profile has a stable catalogue key, not a selectable
    /// upstream model version. Its connector must not send this key as a model ID.
    #[serde(default)]
    pub server_managed: bool,
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
    /// Former provider IDs folded into this provider at the configuration boundary.
    #[serde(default)]
    pub provider_aliases: Vec<String>,
    /// Contract for text models discovered through this service's signed-in account.
    /// The account connector validates the selected ID against its live catalogue.
    #[serde(default)]
    pub account_models: Option<RemoteCapabilities>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProviderManifest {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
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

pub fn preset_voice_label(voice: &str) -> &str {
    catalog()
        .models
        .iter()
        .filter_map(|model| model.tts.as_ref())
        .find_map(|tts| tts.voice_labels.get(voice))
        .map_or(voice, String::as_str)
}

pub fn preset_voice_language(voice: &str) -> &str {
    catalog()
        .models
        .iter()
        .find_map(|model| {
            let tts = model.tts.as_ref()?;
            if !tts.voices.iter().any(|preset| preset == voice) {
                return None;
            }
            let languages = tts.voice_languages.get(voice).unwrap_or(&model.languages);
            Some(if languages.iter().any(|language| language == "zh") {
                "zh"
            } else {
                languages.first().map_or("en", String::as_str)
            })
        })
        .unwrap_or("zh")
}

pub fn account_model_capabilities(
    connector: RemoteConnector,
) -> Option<&'static RemoteCapabilities> {
    catalog()
        .providers
        .iter()
        .flat_map(|p| &p.services)
        .find_map(|service| {
            let selected = service
                .defaults
                .get("connector")
                .and_then(|v| serde_json::from_value::<RemoteConnector>(v.clone()).ok());
            (selected == Some(connector))
                .then_some(service.account_models.as_ref())
                .flatten()
        })
}

pub fn model_capabilities(
    connector: RemoteConnector,
    model: &str,
) -> Option<&'static RemoteCapabilities> {
    remote_model(connector, model)
        .map(|card| &card.capabilities)
        .or_else(|| account_model_capabilities(connector))
}

/// A provider can serve the same capability through several model-specific protocols.
/// Keep their model choices together while retaining each connector's wire contract.
pub fn services(
    provider: &str,
    capability: ModelCapability,
) -> impl Iterator<Item = &'static RemoteServiceManifest> {
    catalog()
        .providers
        .iter()
        .filter(move |entry| entry.id == provider)
        .flat_map(|entry| &entry.services)
        .filter(move |service| service.capability == capability)
}
