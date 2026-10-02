//! Screen text consumes the same owner-scoped translation results as other plugins.
use std::collections::BTreeMap;

use crate::{
    i18n::{UiLanguage, tr},
    overlay_ipc::OcrOverlayState,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionEventSubscriber, SessionOutputPolicy,
        TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    },
    ui::components,
};
use crossbeam_channel::{Receiver, Sender, unbounded};

#[derive(Clone)]
pub struct OcrTranslationSink {
    tx: Sender<(String, TranslationEvent)>,
    rx: Receiver<(String, TranslationEvent)>,
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
            && let Some(operation) = owner.operation_id()
        {
            let _ = self.tx.send((operation.to_owned(), event.clone()));
        }
    }
}

#[derive(Default)]
pub struct OcrPlugin {
    pub event_sink: OcrTranslationSink,
    pub state: OcrOverlayState,
    owner: Option<PluginSessionOwner>,
    recognizing: bool,
    translating: bool,
    capture_error: Option<String>,
    translation_error: Option<String>,
    translated_segments: BTreeMap<u32, String>,
}

impl OcrPlugin {
    pub fn owner(&self) -> Option<TranslationSessionOwner> {
        self.owner.clone().map(TranslationSessionOwner::Plugin)
    }

    pub fn clear(&mut self) {
        self.owner = None;
        self.recognizing = false;
        self.translating = false;
        self.capture_error = None;
        self.translation_error = None;
        self.translated_segments.clear();
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
        self.state.translated.clear();
        self.translated_segments.clear();
        if self.state.source.is_empty() {
            return None;
        }
        let owner = PluginSessionOwner::new(
            super::PluginId::OCR.as_str(),
            uuid::Uuid::new_v4().to_string(),
            "Screen Translation",
            "Screen Translation",
            "Translating screen text",
        );
        self.owner = Some(owner.clone());
        self.translating = true;
        self.refresh_status();
        Some(PluginSessionBinding::text(
            owner,
            SessionOutputPolicy::PluginOnly,
        ))
    }

    pub fn poll(&mut self) {
        while let Ok((operation, event)) = self.event_sink.rx.try_recv() {
            if !self.translating
                || self
                    .owner
                    .as_ref()
                    .is_none_or(|owner| owner.operation_id() != operation)
            {
                continue;
            }
            match event {
                TranslationEvent::Segment(segment) => {
                    if let Some(text) = segment.translated {
                        self.translated_segments.insert(segment.segment_index, text);
                        self.refresh_translation();
                    }
                }
                TranslationEvent::ReplaceSegments(segments) => {
                    self.translated_segments = segments
                        .into_iter()
                        .filter_map(|segment| {
                            segment.translated.map(|text| (segment.segment_index, text))
                        })
                        .collect();
                    self.refresh_translation();
                }
                TranslationEvent::Finished { outcome, .. } => {
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
                TranslationEvent::StreamEnded { .. } => {}
            }
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

    fn refresh_translation(&mut self) {
        self.state.translated = self
            .translated_segments
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
    }

    /// The host enables the input and opens its shared window when needed.
    pub fn render(
        &self,
        ui: &mut eframe::egui::Ui,
        language: UiLanguage,
        enabled: bool,
    ) -> Option<bool> {
        ui.label(tr(language, "Move and resize the frame over text. For video subtitles, frame only the subtitle area. Translation appears in a separate window you can drag freely."));
        ui.add_space(12.0);
        let mut enabled = enabled;
        let changed = ui
            .horizontal(|ui| {
                let changed = components::pill_toggle(ui, &mut enabled).changed();
                ui.label(tr(language, "Screen Translation"));
                changed
            })
            .inner;
        if self.state.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr(
                    language,
                    if self.translating {
                        "Translating screen text"
                    } else {
                        "Screen text recognition"
                    },
                ));
            });
        }
        if let Some(status) = &self.state.status {
            components::error_notice(ui, language, status);
        }
        if !self.state.source.is_empty() {
            ui.add_space(12.0);
            components::card(ui, |ui| {
                ui.label(&self.state.source);
            });
        }
        if !self.state.translated.is_empty() {
            ui.add_space(8.0);
            components::card(ui, |ui| {
                ui.label(&self.state.translated);
            });
        }
        changed.then_some(enabled)
    }
}
