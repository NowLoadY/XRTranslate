//! Stateless inference requests share the configured backend and model scheduler.
use crate::backend::{BackendManager, BackendStart, BackendStatus};
use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use xrtranslate_protocol::{AskError, AskRequest, AskResponse};

pub(crate) type Reply = Receiver<Result<String, AskError>>;

struct Request {
    input: AskRequest,
    reply: Sender<Result<String, AskError>>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
        self.request_with_cancel(AskRequest::Prompt { prompt }, None, ctx)
    }

    pub fn request_with_cancel(
        &mut self,
        input: AskRequest,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        ctx: &egui::Context,
    ) -> Reply {
        let (reply, receiver) = bounded(1);
        self.queue.push_back(Request {
            input,
            reply,
            cancel,
        });
        ctx.request_repaint();
        receiver
    }

    pub fn poll(&mut self, backend: &mut BackendManager, server: &str, ctx: &egui::Context) {
        self.queue.retain(|request| {
            !request
                .cancel
                .as_ref()
                .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire))
        });
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
                    let _ = request.reply.send(Err(error.clone().into()));
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
                    runtime.block_on(async {
                        tokio::select! {
                            result=send(&server,request.input)=>result,
                            _=async {
                                if let Some(cancel)=request.cancel {
                                    while !cancel.load(std::sync::atomic::Ordering::Acquire) {tokio::time::sleep(Duration::from_millis(50)).await;}
                                } else {std::future::pending::<()>().await;}
                            }=>Err("Request cancelled".into()),
                        }
                    })
                })();
                let _ = request.reply.send(result);
                let _ = done.send(());
                ctx.request_repaint();
            });
        match worker {
            Ok(_) => self.running = Some(running),
            Err(error) => {
                let _ = fallback.send(Err(error.to_string().into()));
            }
        }
    }
}

async fn send(server: &str, input: AskRequest) -> Result<String, AskError> {
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
        .json(&input)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(serde_json::from_str::<AskError>(&body)
            .unwrap_or_else(|_| format!("Ask failed ({status}): {body}").into()));
    }
    let response: AskResponse = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if response.text.trim().is_empty() {
        return Err("No answer was returned".into());
    }
    Ok(response.text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_accepts_one_context_source_and_no_remote_session() {
        let legacy: AskRequest = serde_json::from_value(json!({"prompt":"hello"})).unwrap();
        assert_eq!(legacy.into_messages()[0].content, "hello");
        let wire = json!({"messages":[
            {"role":"system","content":"rules"},
            {"role":"user","content":"question"},
            {"role":"assistant","content":"answer"},
            {"role":"user","content":"next"}
        ]});
        let request: AskRequest = serde_json::from_value(wire.clone()).unwrap();
        assert!(request.response_schema().is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
        let mut structured = wire.clone();
        structured["response_schema"] = json!({"type":"object"});
        let request: AskRequest = serde_json::from_value(structured.clone()).unwrap();
        assert!(request.response_schema().is_some());
        assert_eq!(serde_json::to_value(request).unwrap(), structured);
        for extra in ["prompt", "conversation", "previous_response_id"] {
            let mut invalid = wire.clone();
            invalid[extra] = json!("must not be combined with messages");
            assert!(serde_json::from_value::<AskRequest>(invalid).is_err());
        }
    }
}
