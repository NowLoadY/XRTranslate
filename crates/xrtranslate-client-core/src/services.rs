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
