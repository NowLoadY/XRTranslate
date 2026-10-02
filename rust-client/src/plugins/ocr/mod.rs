//! Screen text consumes the same owner-scoped translation results as other plugins.
use crate::{
    overlay_ipc::OcrOverlayState,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionEventSubscriber, SessionOutputPolicy,
        TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    },
};
use crossbeam_channel::{Receiver, Sender, unbounded};

#[derive(Clone)]
pub struct OcrTranslationSink {
    tx: Sender<(String, TranslationOutcome)>,
    rx: Receiver<(String, TranslationOutcome)>,
}

impl Default for OcrTranslationSink {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

impl SessionEventSubscriber for OcrTranslationSink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(super::PluginId::OCR.as_str())
    }

    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if self.accepts_owner(owner)
            && let TranslationEvent::Finished { outcome, .. } = event
            && let Some(operation) = owner.operation_id()
        {
            // Translation content already goes to the shared result bubbles.
            let _ = self.tx.send((operation.to_owned(), outcome.clone()));
        }
    }
}

#[derive(Default)]
pub struct OcrPlugin {
    pub event_sink: OcrTranslationSink,
    pub state: OcrOverlayState,
    owner: Option<PluginSessionOwner>,
    conversation: Option<String>,
    recognizing: bool,
    translating: bool,
    capture_error: Option<String>,
    translation_error: Option<String>,
}

impl OcrPlugin {
    pub fn clear(&mut self) {
        self.owner = None;
        self.conversation = None;
        self.recognizing = false;
        self.translating = false;
        self.capture_error = None;
        self.translation_error = None;
        self.state = OcrOverlayState::default();
    }

    pub fn set_recognizing(&mut self, recognizing: bool) {
        self.recognizing = recognizing;
        self.capture_error = None;
        self.refresh_status();
    }

    pub fn capture_failed(&mut self, error: String) {
        self.recognizing = false;
        self.capture_error = Some(error);
        self.refresh_status();
    }

    pub fn failed(&mut self, error: String) {
        self.owner = None;
        self.translating = false;
        self.recognizing = false;
        self.translation_error = Some(error);
        self.refresh_status();
    }

    pub fn recognized(&mut self, text: String) -> Option<PluginSessionBinding> {
        self.recognizing = false;
        self.capture_error = None;
        self.refresh_status();
        let text = text.trim().to_owned();
        if text
            .split_whitespace()
            .eq(self.state.source.split_whitespace())
            && self.owner.is_some()
        {
            return None;
        }
        self.owner = None;
        self.translating = false;
        self.translation_error = None;
        self.refresh_status();
        self.state.source = text;
        if self.state.source.is_empty() {
            return None;
        }
        let conversation = self
            .conversation
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string());
        let owner = PluginSessionOwner::new(
            super::PluginId::OCR.as_str(),
            uuid::Uuid::new_v4().to_string(),
            "Screen Translation",
            "Screen Translation",
            "Translating screen text",
        )
        .in_conversation(conversation.clone());
        self.owner = Some(owner.clone());
        self.translating = true;
        self.refresh_status();
        Some(PluginSessionBinding::text(owner, SessionOutputPolicy::Host))
    }

    pub fn poll(&mut self) {
        while let Ok((operation, outcome)) = self.event_sink.rx.try_recv() {
            if !self.translating
                || self
                    .owner
                    .as_ref()
                    .is_none_or(|owner| owner.operation_id() != operation)
            {
                continue;
            }
            self.translating = false;
            match outcome {
                TranslationOutcome::Completed => {}
                TranslationOutcome::Cancelled => self.owner = None,
                TranslationOutcome::Failed(error) => {
                    self.owner = None;
                    self.translation_error = Some(error);
                }
            }
            self.refresh_status();
        }
    }

    fn refresh_status(&mut self) {
        self.state.busy = self.recognizing || self.translating;
        self.state.status = self
            .translation_error
            .as_ref()
            .or(self.capture_error.as_ref())
            .cloned();
    }
}
