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
    Preview(String),
}

enum Response {
    Status(VoiceStatus),
    Preview(String, Vec<u8>, u32),
}

#[derive(Default)]
pub struct TtsCenterController {
    pub status: VoiceStatus,
    pub error: Option<String>,
    pub loaded: bool,
    pub previewing: Option<String>,
    pub requested_voice: Option<String>,
    pub catalog: CatalogController,
    pub draft: Option<VoiceDraft>,
    pub generated_preview: Option<(String, Vec<u8>, u32)>,
    pending: Option<Request>,
    deadline: Option<Instant>,
    next_poll: Option<Instant>,
    response: Option<mpsc::Receiver<Result<Response, String>>>,
}

impl TtsCenterController {
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.response.is_some()
    }

    pub fn refresh(&mut self) {
        if !self.busy() {
            self.status = VoiceStatus::default();
        }
        self.enqueue(Request::Read);
    }

    pub fn select(&mut self, voice_id: Option<String>) {
        self.enqueue(Request::Select(VoiceSelection {
            voice_id,
            preset: None,
        }));
    }

    pub fn select_preset(&mut self, preset: String) {
        self.enqueue(Request::Select(VoiceSelection {
            voice_id: None,
            preset: Some(preset),
        }));
    }

    pub fn preview_preset(&mut self, preset: String) {
        self.enqueue(Request::Preview(preset));
    }

    pub fn import(&mut self, draft: VoiceDraft, ctx: &egui::Context) {
        self.error = None;
        self.requested_voice = None;
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
        self.requested_voice = match &request {
            Request::Preview(preset) => Some(preset.clone()),
            Request::Select(selection) => selection
                .voice_id
                .clone()
                .or_else(|| selection.preset.clone()),
            _ => None,
        };
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
                Err(error) => {
                    self.requested_voice = None;
                    self.error = Some(error);
                }
            }
        }
        if let Some(response) = &self.response {
            match response.try_recv() {
                Ok(result) => {
                    self.response = None;
                    match result {
                        Ok(Response::Status(status)) => self.status = status,
                        Ok(Response::Preview(id, pcm, rate)) => {
                            self.generated_preview = Some((id, pcm, rate))
                        }
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
                let request = self.pending.take().unwrap();
                if let Request::Import(draft) = request {
                    self.catalog.import(draft, Some(server_url.to_owned()), ctx);
                    return;
                }
                let server_url = server_url.to_owned();
                let ctx = ctx.clone();
                let (send, receive) = mpsc::channel();
                self.response = Some(receive);
                let spawn = std::thread::Builder::new()
                    .name("tts-voice-selection".into())
                    .spawn(move || {
                        let result = if let Request::Preview(preset) = request {
                            super::preview::generate(&server_url, &preset).map(|(pcm, rate)| {
                                Response::Preview(format!("preset:{preset}"), pcm, rate)
                            })
                        } else {
                            super::transport::request(&server_url, "/tts/voice", |client, url| {
                                match request {
                                    Request::Select(selection) => client.post(url).json(&selection),
                                    _ => client.get(url),
                                }
                            })
                            .map(Response::Status)
                        };
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
