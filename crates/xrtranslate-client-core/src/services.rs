//! Shared host capabilities used by native plugin controllers.
use crate::session_coordinator::{TranslationEvent, TranslationSessionOwner, TranslationTask};
use crate::{i18n::UiLanguage, ui::components::AudioPreprocessingModels};
use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use xrtranslate_plugin::{Client, Request};
use xrtranslate_protocol::{AskError, AskRequest};

/// A live task can contain both microphone and system-audio streams.
#[derive(Default)]
pub struct ActiveTasks(Vec<(TranslationSessionOwner, usize)>);
impl ActiveTasks {
    pub fn begin(&mut self, task: &TranslationTask) {
        let owner = task.owner();
        self.remove(&owner);
        self.0.push((
            owner,
            task.input
                .live_source()
                .map_or(1, |source| source.routes().len()),
        ));
    }
    pub fn observe(&mut self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if matches!(event, TranslationEvent::Finished { .. }) {
            if let Some((_, remaining)) = self.0.iter_mut().find(|(current, _)| current == owner) {
                *remaining = remaining.saturating_sub(1);
            }
            self.0.retain(|(_, remaining)| *remaining > 0);
        }
    }
    pub fn remove(&mut self, owner: &TranslationSessionOwner) {
        self.0.retain(|(current, _)| current != owner);
    }
    pub fn take(&mut self) -> impl Iterator<Item = TranslationSessionOwner> {
        std::mem::take(&mut self.0)
            .into_iter()
            .map(|(owner, _)| owner)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub source_language: String,
    pub target_language: String,
    pub capture: crate::recognition::CaptureSource,
    pub profiles: Vec<(
        crate::recognition::CaptureSource,
        crate::recognition::RecognitionSettings,
    )>,
    pub microphone_enabled: bool,
    pub system_audio_enabled: bool,
    pub language: UiLanguage,
    pub languages: xrtranslate_engine::language::LanguageCapabilities,
    pub preprocessing: AudioPreprocessingModels,
    pub default_preprocessing: xrtranslate_protocol::AudioPreprocessing,
    pub download_proxy: String,
    pub ask: bool,
    pub agent: bool,
    pub tts: bool,
    #[serde(default)]
    pub tts_reference_audio: bool,
    pub text: TextState,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextState {
    pub preparing: bool,
    pub asr_only: bool,
    pub additional_target: Option<String>,
    pub microphone_sync: bool,
    #[serde(default)]
    pub input_budget: Option<usize>,
    #[serde(default)]
    pub can_copy: bool,
    #[serde(default)]
    pub recognition: Option<RecognitionInput>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognitionInput {
    pub active: bool,
    pub revision: u64,
    pub text: String,
    pub can_copy: bool,
}

/// The same application actions used by the translation page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TextAction {
    Clear,
    Copy,
    DismissCopy,
    CopyRecognition,
    CollectRecognition {
        enabled: bool,
        text: String,
    },
    Submit {
        text: String,
    },
    Language {
        source: String,
        target: String,
        asr_only: bool,
    },
    MicrophoneSync {
        enabled: bool,
    },
}

pub type Reply = Receiver<Result<String, AskError>>;
#[derive(Default)]
pub struct Inference {
    pending: Vec<(Request, Sender<Result<String, AskError>>)>,
}
impl Inference {
    pub fn request(&mut self, client: &Client, prompt: String) -> Reply {
        let (send, reply) = crossbeam_channel::bounded(1);
        self.pending.push((
            client.request(
                "inference.request",
                serde_json::to_value(AskRequest::Prompt { prompt }).unwrap(),
            ),
            send,
        ));
        reply
    }
    pub fn poll(&mut self) {
        self.pending.retain(|(request, send)| {
            if let Some(result) = request.poll() {
                let result = result.map_err(AskError::from).and_then(|value| {
                    match value.get("text").and_then(|v| v.as_str()) {
                        Some(text) => Ok(text.to_owned()),
                        None => Err(
                            serde_json::from_value(value["error"].clone()).unwrap_or_else(|_| {
                                AskError::from("Invalid inference response".to_owned())
                            }),
                        ),
                    }
                });
                let _ = send.send(result);
                false
            } else {
                true
            }
        });
    }
}

/// Running means every input is capturing and its inference session is ready.
/// Terminal states are published only after the task has released its inputs.
#[derive(Clone, Serialize, Deserialize)]
pub enum LiveState {
    Running,
    Stopped,
    Failed(String),
}

pub fn select_text_languages(
    text: &str,
    source: &str,
    target: &str,
    capabilities: xrtranslate_engine::language::LanguageCapabilities,
    additional_target: Option<&str>,
    asr_only: bool,
) -> Result<xrtranslate_engine::language::LanguageSelection, String> {
    let capabilities = capabilities.for_text();
    if asr_only {
        return capabilities.select_with_options(source, target, None, true);
    }
    if let Some((source, target)) =
        xrtranslate_engine::auto_route_language_pair(text, source, target)
        && let Ok(selection) =
            capabilities.select_with_options(source, target, additional_target, false)
    {
        return Ok(selection);
    }
    capabilities.select_with_options(source, target, additional_target, false)
}
