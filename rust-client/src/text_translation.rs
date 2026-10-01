//! A reusable text conversation, independent of audio capture and media tasks.
use crate::{
    CaptureSource,
    backend::{BackendManager, BackendStart, BackendStatus},
    network::{self, ExternalAudioGate, SessionConfig, SessionEvent, SessionHandle},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use std::time::{Duration, Instant};
use xrtranslate_engine::language::LanguageSelection;
use xrtranslate_protocol::PromptGraphSet;

pub(crate) enum TextEvent {
    Accepted(String),
    Result(SessionEvent),
    Failed(String),
}

struct Request {
    text: String,
    languages: LanguageSelection,
}

pub(crate) struct TextTranslation {
    session: Option<SessionHandle>,
    pending: Option<Request>,
    connected: bool,
    outstanding: usize,
    backend_preparing: bool,
    started: Option<Instant>,
    next_poll: Instant,
    tx: Sender<SessionEvent>,
    rx: Receiver<SessionEvent>,
}

impl Default for TextTranslation {
    fn default() -> Self {
        let (tx, rx) = bounded(128);
        Self {
            session: None,
            pending: None,
            connected: false,
            outstanding: 0,
            backend_preparing: false,
            started: None,
            next_poll: Instant::now(),
            tx,
            rx,
        }
    }
}

impl TextTranslation {
    pub(crate) fn busy(&self) -> bool {
        self.preparing() || self.outstanding > 0
    }

    pub(crate) fn preparing(&self) -> bool {
        self.pending.is_some()
    }

    /// A pending startup owns one captured request, never the later UI draft.
    pub(crate) fn submit(
        &mut self,
        text: &str,
        languages: LanguageSelection,
    ) -> Result<bool, String> {
        if self.pending.is_some() {
            return Ok(false);
        }
        if self.connected {
            let (source, target) = languages.wire();
            self.session
                .as_ref()
                .ok_or("Text session is unavailable")?
                .translate_text(text, Some(source), Some(target))?;
            self.outstanding += 1;
            return Ok(true);
        }
        self.pending = Some(Request {
            text: text.to_owned(),
            languages,
        });
        self.started = Some(Instant::now());
        self.next_poll = Instant::now();
        Ok(false)
    }

    pub(crate) fn update_prompts(&self, graphs: PromptGraphSet) {
        if let Some(session) = &self.session {
            session.update_prompt_templates(graphs);
        }
    }

    pub(crate) fn reset(&mut self) {
        if let Some(session) = self.session.take() {
            session.cancel();
        }
        self.connected = false;
        self.outstanding = 0;
        self.backend_preparing = false;
        self.pending = None;
        self.started = None;
        // Detach old producers immediately, before a later request can connect.
        let (tx, rx) = bounded(128);
        self.tx = tx;
        self.rx = rx;
    }

    pub(crate) fn poll(
        &mut self,
        backend: &mut BackendManager,
        server_url: &str,
        graphs: PromptGraphSet,
        ctx: egui::Context,
    ) -> Vec<TextEvent> {
        let mut events = Vec::new();
        if self.pending.is_some() && self.session.is_none() && Instant::now() >= self.next_poll {
            self.next_poll = Instant::now() + Duration::from_millis(250);
            let state = if self.backend_preparing {
                match backend.status(server_url) {
                    BackendStatus::Ready => Ok(BackendStart::Ready),
                    BackendStatus::Starting(stage) => Ok(BackendStart::Starting(stage)),
                    BackendStatus::Failed(error) => Err(error),
                }
            } else {
                backend.prepare(server_url)
            };
            match state {
                Ok(BackendStart::Ready) => {
                    let (tx, rx) = bounded(128);
                    self.tx = tx;
                    self.rx = rx;
                    let request = self.pending.as_ref().unwrap();
                    self.session = Some(network::start_text_session(
                        self.tx.clone(),
                        SessionConfig {
                            server_url: server_url.to_owned(),
                            languages: request.languages,
                            external_audio_gate: ExternalAudioGate::default(),
                            publish_to_host_outputs: false,
                            tts: None,
                            egui_ctx: Some(ctx.clone()),
                            vad_threshold: 0.0,
                            vad_silence_ms: 0,
                            continuous_recognition: false,
                            audio_source: CaptureSource::Microphone,
                            finish_when_audio_ends: false,
                            prompt_graphs: Some(graphs),
                        },
                    ));
                }
                Ok(BackendStart::Starting(_)) => self.backend_preparing = true,
                Err(error) => {
                    self.reset();
                    events.push(TextEvent::Failed(error));
                }
            }
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                SessionEvent::Connected => {
                    self.connected = true;
                    if let Some(request) = self.pending.take() {
                        let (source, target) = request.languages.wire();
                        match self.session.as_ref().unwrap().translate_text(
                            &request.text,
                            Some(source),
                            Some(target),
                        ) {
                            Ok(()) => {
                                self.outstanding += 1;
                                events.push(TextEvent::Accepted(request.text));
                            }
                            Err(error) => events.push(TextEvent::Failed(error)),
                        }
                    }
                    self.started = None;
                }
                SessionEvent::Error(error) | SessionEvent::BackendError { message: error, .. } => {
                    self.reset();
                    events.push(TextEvent::Failed(error));
                    break;
                }
                SessionEvent::Disconnected(reason) => {
                    self.reset();
                    if reason != "Cancelled" {
                        events.push(TextEvent::Failed(reason));
                    }
                    break;
                }
                SessionEvent::Translation {
                    revisable,
                    segment_index,
                    segment_count,
                    ..
                } => {
                    if !revisable && segment_index >= segment_count.max(1) {
                        self.outstanding = self.outstanding.saturating_sub(1);
                    }
                    events.push(TextEvent::Result(event));
                }
                _ => {}
            }
        }
        if self
            .started
            .is_some_and(|at| at.elapsed() > Duration::from_secs(180))
        {
            self.reset();
            events.push(TextEvent::Failed(
                "Text translation startup timed out".into(),
            ));
        }
        if self.preparing() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        events
    }
}

impl Drop for TextTranslation {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_detaches_late_events_before_a_new_request() {
        let mut text = TextTranslation::default();
        let old = text.tx.clone();
        text.reset();
        assert!(old.send(SessionEvent::Connected).is_err());
        assert!(!text.busy());
        let languages = LanguageSelection::parse("en", "zh").unwrap();
        text.submit("new", languages).unwrap();
        assert_eq!(text.pending.as_ref().unwrap().text, "new");
        assert!(!text.connected);
    }

    #[test]
    fn multipart_text_result_stays_busy_until_the_last_one_based_segment() {
        let mut text = TextTranslation::default();
        text.outstanding = 1;
        let mut backend = BackendManager::load();
        let make_segment = |segment_index| SessionEvent::Translation {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            continuous: false,
            publish_to_host_outputs: false,
            source: "words".into(),
            translated: "文字".into(),
            turn_id: "text-1".into(),
            segment_index,
            segment_count: 2,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 0.0,
            timing: Default::default(),
            boundary: Default::default(),
            term_matches: Vec::new(),
            prompt_trace: None,
            revisable: false,
            overlap_ratio: 0.0,
            authoritative_snapshot: false,
            revision: 0,
        };
        text.tx.send(make_segment(1)).unwrap();
        text.poll(
            &mut backend,
            "",
            PromptGraphSet {
                graph: Default::default(),
            },
            egui::Context::default(),
        );
        assert!(text.busy());
        text.tx.send(make_segment(2)).unwrap();
        text.poll(
            &mut backend,
            "",
            PromptGraphSet {
                graph: Default::default(),
            },
            egui::Context::default(),
        );
        assert!(!text.busy());
    }

    #[test]
    fn preparation_keeps_the_first_request_and_its_language_pair() {
        let mut controller = TextTranslation::default();
        let languages = LanguageSelection::parse("en", "zh").unwrap();
        assert!(!controller.submit("first", languages).unwrap());
        assert!(!controller.submit("later draft", languages).unwrap());
        let request = controller.pending.as_ref().unwrap();
        assert_eq!(request.text, "first");
        assert_eq!(request.languages, languages);
        controller.reset();
        assert!(!controller.preparing());
    }
}
