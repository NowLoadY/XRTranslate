use super::{catalog::CatalogController, editor::VoiceDraft};
use crate::backend::{BackendManager, BackendStart, BackendStatus};
use eframe::egui;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use xrtranslate_protocol::tts::{VoiceSelection, VoiceStatus};

#[derive(Clone)]
enum Request {
    Read,
    Select(VoiceSelection),
    Import(VoiceDraft),
}

#[derive(Default)]
pub struct TtsCenterController {
    pub status: VoiceStatus,
    pub error: Option<String>,
    pub loaded: bool,
    pub previewing: Option<String>,
    pub catalog: CatalogController,
    pub draft: Option<VoiceDraft>,
    pending: Option<Request>,
    deadline: Option<Instant>,
    next_poll: Option<Instant>,
    response: Option<mpsc::Receiver<Result<VoiceStatus, String>>>,
}

impl TtsCenterController {
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.response.is_some()
    }

    pub fn refresh(&mut self) {
        self.enqueue(Request::Read);
    }

    pub fn select(&mut self, voice_id: Option<String>) {
        self.enqueue(Request::Select(VoiceSelection { voice_id }));
    }

    pub fn import(&mut self, draft: VoiceDraft, ctx: &egui::Context) {
        self.error = None;
        if draft.manual_transcript {
            self.catalog.import(draft, None, ctx);
        } else {
            self.enqueue(Request::Import(draft));
        }
    }

    fn enqueue(&mut self, request: Request) {
        if self.busy() {
            return;
        }
        self.pending = Some(request);
        self.deadline = None;
        self.next_poll = None;
        self.error = None;
        if !matches!(self.pending, Some(Request::Import(_))) {
            self.loaded = true;
        }
    }

    pub fn poll(
        &mut self,
        backend: &mut BackendManager,
        server_url: &str,
        ctx: &egui::Context,
        visible: bool,
    ) {
        if visible || self.catalog.busy() {
            match self
                .catalog
                .poll(backend.runtime_layout().voice_clones_directory(), ctx)
            {
                Ok(true) => {
                    self.draft = None;
                    self.error = None;
                }
                Ok(false) => {}
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(response) = &self.response {
            match response.try_recv() {
                Ok(result) => {
                    self.response = None;
                    match result {
                        Ok(status) => self.status = status,
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.response = None;
                    self.error = Some("Voice selection worker stopped.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.next_poll.is_some_and(|next| Instant::now() < next) {
            return;
        }
        self.next_poll = Some(Instant::now() + Duration::from_millis(250));
        let ready = if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                Err("Local services did not become ready within 180 seconds".into())
            } else {
                match backend.status(server_url) {
                    BackendStatus::Ready => Ok(true),
                    BackendStatus::Starting(_) => Ok(false),
                    BackendStatus::Failed(error) => Err(error),
                }
            }
        } else {
            self.deadline = Some(Instant::now() + Duration::from_secs(180));
            backend
                .prepare(server_url)
                .map(|status| matches!(status, BackendStart::Ready))
        };
        match ready {
            Ok(false) => {}
            Err(error) => {
                self.pending = None;
                self.error = Some(error);
            }
            Ok(true) => {
                let selection = match self.pending.take().unwrap() {
                    Request::Read => None,
                    Request::Select(selection) => Some(selection),
                    Request::Import(draft) => {
                        self.catalog.import(draft, Some(server_url.to_owned()), ctx);
                        return;
                    }
                };
                let server_url = server_url.to_owned();
                let ctx = ctx.clone();
                let (send, receive) = mpsc::channel();
                self.response = Some(receive);
                let spawn = std::thread::Builder::new()
                    .name("tts-voice-selection".into())
                    .spawn(move || {
                        let result =
                            super::transport::request(&server_url, "/tts/voice", |client, url| {
                                match selection {
                                    Some(selection) => client.post(url).json(&selection),
                                    None => client.get(url),
                                }
                            });
                        let _ = send.send(result);
                        ctx.request_repaint();
                    });
                if let Err(error) = spawn {
                    self.response = None;
                    self.error = Some(error.to_string());
                }
            }
        }
    }
}
