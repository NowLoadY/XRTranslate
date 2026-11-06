//! Screen text consumes the same owner-scoped translation results as other plugins.
use crate::{
    screen_text::ScreenTextState as OcrOverlayState,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationOutcome,
    },
};
#[derive(Default)]
pub struct OcrPlugin {
    pub state: OcrOverlayState,
    owner: Option<PluginSessionOwner>,
    recognizing: bool,
    translating: bool,
    capture_error: Option<String>,
    translation_error: Option<String>,
}

impl OcrPlugin {
    pub fn clear(&mut self) {
        self.owner = None;
        self.recognizing = false;
        self.translating = false;
        self.capture_error = None;
        self.translation_error = None;
        self.state = OcrOverlayState::default();
    }

    pub fn set_recognizing(&mut self, recognizing: bool) {
        self.recognizing = recognizing;
        // Starting a retry does not mean capture has recovered. Keep its error
        // visible until the next successful observation arrives.
        if !recognizing {
            self.capture_error = None;
        }
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
        let owner = PluginSessionOwner::new(
            "ocr",
            uuid::Uuid::new_v4().to_string(),
            "Screen Translation",
            "Screen Translation",
            "Translating screen text",
        );
        self.owner = Some(owner.clone());
        self.translating = true;
        self.refresh_status();
        Some(PluginSessionBinding::text(owner, SessionOutputPolicy::Host))
    }

    pub fn finished(&mut self, operation: &str, outcome: TranslationOutcome) {
        if !self.translating
            || self
                .owner
                .as_ref()
                .is_none_or(|owner| owner.operation_id() != operation)
        {
            return;
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

    fn refresh_status(&mut self) {
        // Capturing changed pixels can yield exactly the same text. Once we
        // have a source, those background checks should not pulse the UI.
        self.state.busy = self.translating || (self.recognizing && self.state.source.is_empty());
        self.state.status = self
            .translation_error
            .as_ref()
            .or(self.capture_error.as_ref())
            .cloned();
    }
}
