//! Private shortcut task, presented by the shared native floating renderer.
mod clipboard;
mod host;
use crate::{
    history::archive::vocabulary,
    overlay_ipc::QuickTranslationState,
    overlay_manager::OverlayManager,
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    },
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use std::collections::BTreeMap;
pub(crate) const OWNER: &str = "quick_translate";

#[derive(Clone)]
pub(crate) struct Sink(Sender<(String, TranslationEvent)>);
impl SessionEventSubscriber for Sink {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(OWNER)
    }
    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if let Some(id) = owner.operation_id() {
            let _ = self.0.send((id.to_owned(), event.clone()));
        }
    }
}

pub(crate) struct QuickTranslate {
    pub settings: crate::desktop_shortcut::Settings,
    #[cfg(windows)]
    pub shortcut_draft: String,
    #[cfg(windows)]
    pub shortcut_error: Option<String>,
    pub sink: Sink,
    #[cfg(windows)]
    listener: Option<crate::desktop_shortcut::Listener>,
    #[cfg(windows)]
    applied: Option<crate::desktop_shortcut::Settings>,
    view: Option<QuickTranslationState>,
    overlay: OverlayManager,
    clipboard: clipboard::Clipboard,
    events: Receiver<(String, TranslationEvent)>,
    operation: Option<String>,
    segments: BTreeMap<(String, u32), String>,
    route: Option<(String, String)>,
    bookmark: Option<(String, vocabulary::Reply)>,
}
impl QuickTranslate {
    pub fn new(settings: crate::desktop_shortcut::Settings) -> Self {
        let (tx, events) = unbounded();
        Self {
            #[cfg(windows)]
            shortcut_draft: settings.shortcut.clone(),
            settings,
            #[cfg(windows)]
            shortcut_error: None,
            sink: Sink(tx),
            #[cfg(windows)]
            listener: None,
            #[cfg(windows)]
            applied: None,
            view: None,
            overlay: OverlayManager::new(),
            clipboard: Default::default(),
            events,
            operation: None,
            segments: BTreeMap::new(),
            route: None,
            bookmark: None,
        }
    }
    #[cfg(windows)]
    pub fn apply_shortcut(&mut self) {
        self.settings.shortcut = self.shortcut_draft.trim().to_owned();
        self.applied = None;
    }
    #[cfg(windows)]
    pub fn register(&mut self, ctx: &egui::Context) {
        if self.applied.as_ref() == Some(&self.settings) {
            return;
        }
        self.listener = None;
        self.shortcut_error = None;
        self.applied = Some(self.settings.clone());
        if self.settings.enabled {
            match crate::desktop_shortcut::Listener::start(&self.settings, ctx) {
                Ok(listener) => self.listener = Some(listener),
                Err(error) => self.shortcut_error = Some(error),
            }
        }
    }
    #[cfg(windows)]
    pub fn take_capture(&mut self) -> Option<Result<String, String>> {
        let mut capture = None;
        for event in self.listener.as_ref()?.events.try_iter() {
            match event {
                crate::desktop_shortcut::Event::Captured(text) => capture = Some(text),
                crate::desktop_shortcut::Event::Unavailable(error) => {
                    self.shortcut_error = Some(error)
                }
            }
        }
        capture
    }
    pub fn source(&self) -> String {
        self.view
            .as_ref()
            .map(|view| view.source.clone())
            .unwrap_or_default()
    }
    pub fn begin(&mut self, source: String) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.operation = Some(id.clone());
        self.segments.clear();
        self.bookmark = None;
        self.view = Some(QuickTranslationState {
            id: id.clone(),
            source,
            busy: true,
            ..Default::default()
        });
        id
    }
    pub fn fail(&mut self, error: String) {
        let view = self.view.get_or_insert_with(|| QuickTranslationState {
            id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        });
        view.busy = false;
        view.error = Some(error);
        self.operation = None;
    }
    pub fn close(&mut self) {
        self.view = None;
        self.operation = None;
        self.segments.clear();
        self.bookmark = None;
        self.route = None;
        self.overlay.stop();
    }
    fn entry(&self) -> Option<vocabulary::Entry> {
        let view = self
            .view
            .as_ref()
            .filter(|view| view.can_save && !view.busy && view.error.is_none())?;
        Some(vocabulary::Entry {
            source: view.source.clone(),
            translated: view.translated.clone(),
            source_lang: view.source_lang.clone(),
            target_lang: view.target_lang.clone(),
        })
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut completed = false;
        for (id, event) in self.events.try_iter() {
            if self.operation.as_deref() != Some(id.as_str()) {
                continue;
            }
            let segment = match &event {
                TranslationEvent::Segment(segment) => Some(segment),
                TranslationEvent::ReplaceSegments(segments) => segments.first(),
                _ => None,
            };
            if let Some(segment) = segment
                && let Some(view) = &mut self.view
            {
                for (tag, code) in [
                    (&mut view.source_lang, &segment.source_lang),
                    (&mut view.target_lang, &segment.target_lang),
                ] {
                    if let Some(language) =
                        xrtranslate_engine::language::SupportedLanguage::parse(code)
                    {
                        *tag = language.code().into();
                    }
                }
            }
            match event {
                TranslationEvent::Segment(segment) => {
                    if let Some(text) = segment.translated {
                        self.segments
                            .insert((segment.turn_id, segment.segment_index), text);
                    }
                }
                TranslationEvent::ReplaceSegments(segments) => {
                    if let Some(first) = segments
                        .first()
                        .filter(|segment| segment.translated.is_some())
                    {
                        self.segments.retain(|(turn, _), _| turn != &first.turn_id);
                        for segment in segments {
                            if let Some(text) = segment.translated {
                                self.segments
                                    .insert((segment.turn_id, segment.segment_index), text);
                            }
                        }
                    }
                }
                TranslationEvent::Finished { outcome, .. } => {
                    let Some(view) = &mut self.view else { continue };
                    view.busy = false;
                    match outcome {
                        TranslationOutcome::Completed => {
                            let mut text = String::new();
                            for segment in self.segments.values() {
                                crate::streaming::append_segment(&mut text, segment);
                            }
                            if text.trim().is_empty() {
                                view.error = Some("No translation was returned. Try again.".into());
                            } else {
                                view.translated = text;
                                view.can_save = vocabulary::can_save(&view.source)
                                    && xrtranslate_engine::language::SupportedLanguage::parse(
                                        &view.source_lang,
                                    )
                                    .is_some()
                                    && xrtranslate_engine::language::SupportedLanguage::parse(
                                        &view.target_lang,
                                    )
                                    .is_some();
                                completed = true;
                            }
                        }
                        TranslationOutcome::Failed(error) => view.error = Some(error),
                        TranslationOutcome::Cancelled => {
                            view.error = Some("Translation stopped.".into())
                        }
                    }
                    self.operation = None;
                    self.segments.clear();
                }
                TranslationEvent::Preview(_) | TranslationEvent::StreamEnded { .. } => {}
            }
            ctx.request_repaint();
        }
        if let Some((id, result)) = self
            .bookmark
            .as_ref()
            .and_then(|(id, reply)| reply.try_recv().ok().map(|result| (id.clone(), result)))
        {
            self.bookmark = None;
            if let Some(view) = &mut self.view
                && view.id == id
            {
                view.saving = false;
                match result {
                    Ok(vocabulary::Output::Folders(folders)) => {
                        view.folders = folders;
                        view.save_error = None;
                    }
                    Err(error) => view.save_error = Some(error),
                    _ => {}
                }
            }
        }
        completed
    }
}
