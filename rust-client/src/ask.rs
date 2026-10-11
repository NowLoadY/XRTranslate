//! Finite, context-free requests share the configured backend and model scheduler.
use crate::backend::{BackendManager, BackendStart, BackendStatus};
use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use xrtranslate_protocol::{AskRequest, AskResponse};

pub(crate) type Reply = Receiver<Result<String, String>>;

struct Request {
    prompt: String,
    reply: Sender<Result<String, String>>,
}

#[derive(Default)]
pub(crate) struct Ask {
    queue: VecDeque<Request>,
    running: Option<Receiver<()>>,
    preparing: Option<Instant>,
    next_poll: Option<Instant>,
}

impl Ask {
    pub fn request(&mut self, prompt: String, ctx: &egui::Context) -> Reply {
        let (reply, receiver) = bounded(1);
        self.queue.push_back(Request { prompt, reply });
        ctx.request_repaint();
        receiver
    }

    pub fn poll(&mut self, backend: &mut BackendManager, server: &str, ctx: &egui::Context) {
        if let Some(running) = &self.running {
            if matches!(
                running.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            ) {
                return;
            }
            self.running = None;
        }
        if self.queue.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.next_poll.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.next_poll = Some(Instant::now() + Duration::from_millis(250));
        let ready = if let Some(started) = self.preparing {
            match backend.status(server) {
                BackendStatus::Ready => Ok(true),
                BackendStatus::Starting(_) if started.elapsed() < Duration::from_secs(180) => {
                    Ok(false)
                }
                BackendStatus::Starting(_) => Err("Model startup timed out".into()),
                BackendStatus::Failed(error) => Err(error),
            }
        } else {
            backend.prepare(server).map(|state| match state {
                BackendStart::Ready => true,
                BackendStart::Starting(_) => {
                    self.preparing = Some(Instant::now());
                    false
                }
            })
        };
        match ready {
            Ok(false) => return,
            Err(error) => {
                self.preparing = None;
                for request in self.queue.drain(..) {
                    let _ = request.reply.send(Err(error.clone()));
                }
                return;
            }
            Ok(true) => self.preparing = None,
        }
        let request = self.queue.pop_front().unwrap();
        let fallback = request.reply.clone();
        let server = server.to_owned();
        let ctx = ctx.clone();
        let (done, running) = bounded(1);
        let worker = std::thread::Builder::new()
            .name("ask".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime.block_on(send(&server, request.prompt))
                })();
                let _ = request.reply.send(result);
                let _ = done.send(());
                ctx.request_repaint();
            });
        match worker {
            Ok(_) => self.running = Some(running),
            Err(error) => {
                let _ = fallback.send(Err(error.to_string()));
            }
        }
    }
}

async fn send(server: &str, prompt: String) -> Result<String, String> {
    let mut url = reqwest::Url::parse(server).map_err(|e| e.to_string())?;
    let scheme = if matches!(url.scheme(), "wss" | "https") {
        "https"
    } else {
        "http"
    };
    url.set_scheme(scheme).map_err(|_| "Invalid backend URL")?;
    url.set_path("/ask");
    url.set_query(None);
    let response = reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_secs(180))
        .json(&AskRequest { prompt })
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("Ask failed ({status}): {body}")));
    }
    let response: AskResponse = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if response.text.trim().is_empty() {
        return Err("No answer was returned".into());
    }
    Ok(response.text.trim().to_owned())
}
