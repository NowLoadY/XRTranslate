//! A small private text task. The host supplies languages and owns execution;
//! this feature owns only its shortcut, result and floating viewport.
mod host;
mod window;
use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub(crate) const OWNER: &str = "quick_translate";

#[derive(Default, Clone)]
struct View {
    open: bool,
    source: Arc<str>,
    translated: Arc<str>,
    error: Option<String>,
    busy: bool,
}
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
#[derive(Clone, Copy)]
pub(crate) enum Action {
    Retry,
    Close,
}
pub(crate) struct QuickTranslate {
    pub settings: crate::desktop_shortcut::Settings,
    pub shortcut_draft: String,
    pub shortcut_error: Option<String>,
    pub sink: Sink,
    listener: Option<crate::desktop_shortcut::Listener>,
    applied: Option<crate::desktop_shortcut::Settings>,
    view: Arc<Mutex<View>>,
    events: Receiver<(String, TranslationEvent)>,
    actions: Receiver<Action>,
    action_tx: Sender<Action>,
    operation: Option<String>,
    segments: BTreeMap<(String, u32), String>,
}
impl QuickTranslate {
    pub fn new(settings: crate::desktop_shortcut::Settings) -> Self {
        let (tx, events) = unbounded();
        let (action_tx, actions) = unbounded();
        Self {
            shortcut_draft: settings.shortcut.clone(),
            settings,
            shortcut_error: None,
            sink: Sink(tx),
            listener: None,
            applied: None,
            view: Arc::default(),
            events,
            actions,
            action_tx,
            operation: None,
            segments: BTreeMap::new(),
        }
    }
    pub fn apply_shortcut(&mut self) {
        self.settings.shortcut = self.shortcut_draft.trim().to_owned();
        self.applied = None;
    }
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
    pub fn take_action(&self) -> Option<Action> {
        self.actions.try_recv().ok()
    }
    pub fn source(&self) -> String {
        self.view.lock().unwrap().source.to_string()
    }
    pub fn begin(&mut self, source: String, ctx: &egui::Context) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.operation = Some(id.clone());
        self.segments.clear();
        *self.view.lock().unwrap() = View {
            open: true,
            source: source.into(),
            busy: true,
            ..Default::default()
        };
        ctx.send_viewport_cmd_to(window::id(), egui::ViewportCommand::Focus);
        id
    }
    pub fn fail(&mut self, error: String) {
        let mut view = self.view.lock().unwrap();
        view.open = true;
        view.busy = false;
        view.error = Some(error);
    }
    pub fn close(&mut self) {
        *self.view.lock().unwrap() = View::default();
        self.operation = None;
        self.segments.clear();
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        for (id, event) in self.events.try_iter() {
            if self.operation.as_deref() != Some(id.as_str()) {
                continue;
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
                    let mut view = self.view.lock().unwrap();
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
                                view.translated = text.into();
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
                TranslationEvent::StreamEnded { .. } => {}
            }
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_coordinator::{PluginSessionOwner, test_segment};
    #[test]
    fn replacing_or_closing_a_popup_rejects_results_from_the_previous_request() {
        let mut quick = QuickTranslate::new(Default::default());
        let ctx = egui::Context::default();
        let owner = |id| {
            TranslationSessionOwner::Plugin(PluginSessionOwner::new(
                OWNER,
                id,
                "Quick translation",
                "Quick translation",
                "Translating…",
            ))
        };
        let first = owner(quick.begin("first".into(), &ctx));
        let second = owner(quick.begin("second".into(), &ctx));
        quick.sink.on_translation_event(
            &first,
            &TranslationEvent::Segment(test_segment("first", "过时")),
        );
        quick.sink.on_translation_event(
            &second,
            &TranslationEvent::Segment(test_segment("second", "第二个")),
        );
        quick.sink.on_translation_event(
            &second,
            &TranslationEvent::Finished {
                stream_id: 1,
                outcome: TranslationOutcome::Completed,
            },
        );
        quick.poll(&ctx);
        assert_eq!(quick.view.lock().unwrap().translated.as_ref(), "第二个");
        assert!(!quick.view.lock().unwrap().busy);
        let third = owner(quick.begin("third".into(), &ctx));
        quick.close();
        quick.sink.on_translation_event(
            &third,
            &TranslationEvent::Finished {
                stream_id: 2,
                outcome: TranslationOutcome::Failed("late failure".into()),
            },
        );
        quick.poll(&ctx);
        let view = quick.view.lock().unwrap();
        assert!(!view.open);
        assert!(view.error.is_none());
    }
}
