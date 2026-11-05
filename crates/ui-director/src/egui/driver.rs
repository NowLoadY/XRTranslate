use crate::Response;
use crate::feedback::Effect;
use ::egui;
use crossbeam_channel::{Receiver, Sender, bounded};
use std::sync::Mutex;

use super::registry::{ElementDescriptor, ElementValue, FrameSnapshot};

#[derive(Debug, Clone)]
pub enum Command {
    Call(crate::ToolCall),
    Page(String),
    Click(String),
    Set {
        target: String,
        value: ElementValue,
    },
    Text(String),
    Viewport {
        width: f32,
        height: f32,
        scale: f32,
    },
    Pointer {
        x: f32,
        y: f32,
        pressed: Option<bool>,
    },
    Scroll {
        x: f32,
        y: f32,
        delta: f32,
    },
}

pub struct Request {
    pub command: Command,
    pub responder: Sender<Response>,
    pub expires: std::time::Instant,
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
pub struct FrameState {
    pub pages: Vec<super::registry::Page>,
    pub pending_widget_reply: Option<Sender<Response>>,
    pub active_page_name: String,
    pub elements: Vec<ElementDescriptor>,
    pub pending_click: Option<String>,
    pub pending_set: Option<(String, ElementValue)>,
    pub pending_page: Option<String>,
    pub pending_input: Vec<egui::Event>,
    pub last_snapshot: FrameSnapshot,
    pub action_performed_this_frame: bool,
    pub confirm_change: Option<(String, ElementValue, f64)>,
}

pub struct Driver {
    context: std::sync::OnceLock<egui::Context>,
    pub(crate) frame_state: Mutex<FrameState>,
    command_tx: Sender<Request>,
    command_rx: Receiver<Request>,
    host_requests: Mutex<Vec<Request>>,
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl Driver {
    #[must_use]
    pub fn new() -> Self {
        let (command_tx, command_rx) = bounded(32);
        Self {
            context: Default::default(),
            frame_state: Mutex::new(FrameState::default()),
            command_tx,
            command_rx,
            host_requests: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn channel(&self) -> Sender<Request> {
        self.command_tx.clone()
    }

    pub(crate) fn request_repaint(&self) {
        if let Some(context) = self.context.get() {
            super::request_frame(context);
        }
    }

    pub(crate) fn owns_context(&self, context: &egui::Context) -> bool {
        self.context.get() == Some(context)
    }

    pub(super) fn context(&self) -> Option<&egui::Context> {
        self.context.get()
    }

    pub fn take_host_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.host_requests.lock().unwrap())
    }

    /// Called at the start of each egui frame to process incoming director action commands.
    pub fn begin_frame(&self, ctx: &egui::Context, current_page: &str) {
        self.context.get_or_init(|| ctx.clone());
        super::feedback::begin_frame(ctx);
        let mut state = self.frame_state.lock().unwrap();
        state.active_page_name = current_page.to_string();
        state.elements.clear();
        state.action_performed_this_frame = false;
        if state
            .confirm_change
            .as_ref()
            .is_some_and(|(_, _, expires)| *expires <= ctx.input(|i| i.time))
        {
            state.confirm_change = None;
        }

        // Process any pending commands from external clients
        while let Ok(envelope) = self.command_rx.try_recv() {
            if envelope.expires <= std::time::Instant::now()
                || envelope
                    .cancelled
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                let _ = envelope.responder.send(Response::err(
                    "Request cancelled or expired before execution",
                ));
                continue;
            }
            match envelope.command {
                Command::Call(_) => {
                    self.host_requests.lock().unwrap().push(envelope);
                }
                Command::Page(_)
                | Command::Click(_)
                | Command::Set { .. }
                | Command::Text(_)
                | Command::Viewport { .. }
                | Command::Pointer { .. }
                | Command::Scroll { .. } => {
                    self.execute_action_command(ctx, &mut state, envelope);
                    // Let this action reach its widget before accepting another mutation.
                    super::request_frame(ctx);
                    break;
                }
            }
        }
    }

    /// Called at the end of each egui frame to save snapshot and fulfill pending query commands.
    pub fn finish_frame(&self, current_page: &str) {
        let mut state = self.frame_state.lock().unwrap();
        state.active_page_name = current_page.to_owned();
        state.last_snapshot = FrameSnapshot {
            page: state.active_page_name.clone(),
            elements: state.elements.clone(),
        };
        if let Some(responder) = state.pending_widget_reply.take() {
            let response = if state.action_performed_this_frame {
                Response::ok(
                    "UI action completed",
                    Some(serde_json::json!({"page":current_page})),
                )
            } else {
                Response::err("Control was not available or enabled in this frame")
            };
            state.pending_click = None;
            state.pending_set = None;
            let _ = responder.send(response);
        }
    }

    fn execute_action_command(
        &self,
        ctx: &egui::Context,
        state: &mut FrameState,
        envelope: Request,
    ) {
        let Request {
            command, responder, ..
        } = envelope;
        match command {
            Command::Page(page_name) => {
                state.pending_click = None;
                state.pending_set = None;
                state.pending_input.clear();
                let target = page_name.trim().to_lowercase();
                let page = state
                    .pages
                    .iter()
                    .find(|entry| {
                        entry.route.eq_ignore_ascii_case(&target)
                            || entry.title.eq_ignore_ascii_case(&target)
                            || entry.label.eq_ignore_ascii_case(&target)
                    })
                    .map(|entry| entry.route.clone());
                if let Some(page) = page {
                    state.pending_page = Some(page);
                    state.pending_widget_reply = Some(responder);
                } else {
                    let _ = responder.send(Response::err(format!(
                        "Page '{page_name}' is unavailable; use ui.pages to discover current routes"
                    )));
                }
            }
            Command::Click(target) => {
                let element = match state.last_snapshot.resolve_element(&target) {
                    Ok(element) => element,
                    Err(error) => {
                        let _ = responder.send(Response::err(error));
                        return;
                    }
                };
                super::feedback::show_in(ctx, [element.feedback(Effect::Pending)]);
                state.pending_click = Some(element.id_hex.clone());
                state.pending_widget_reply = Some(responder);
            }
            Command::Set { target, value } => {
                let element = match state.last_snapshot.resolve_element(&target) {
                    Ok(element) => element,
                    Err(error) => {
                        let _ = responder.send(Response::err(error));
                        return;
                    }
                };
                super::feedback::show_in(ctx, [element.feedback(Effect::Pending)]);
                state.pending_set = Some((element.id_hex.clone(), value));
                state.pending_widget_reply = Some(responder);
            }
            Command::Viewport {
                width,
                height,
                scale,
            } => {
                if !(640.0..=4096.0).contains(&width)
                    || !(480.0..=2160.0).contains(&height)
                    || !(0.75..=3.0).contains(&scale)
                {
                    let _ = responder.send(Response::err("Invalid viewport size or scale"));
                    return;
                }
                // Window commands use this frame's scale; the new scale takes effect next frame.
                let current_scale = ctx.pixels_per_point();
                ctx.set_pixels_per_point(scale);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    width / current_scale,
                    height / current_scale,
                )));
                let _ = responder.send(Response::ok("Viewport updated", None));
            }
            Command::Pointer { x, y, pressed } => {
                let position = egui::pos2(x, y);
                state
                    .pending_input
                    .push(egui::Event::PointerMoved(position));
                if let Some(pressed) = pressed {
                    state.pending_input.push(egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                super::request_frame(ctx);
                let _ = responder.send(Response::ok("Pointer queued", None));
            }
            Command::Text(text) => {
                state.pending_input.push(egui::Event::Text(text));
                super::request_frame(ctx);
                let _ = responder.send(Response::ok("Text input queued", None));
            }
            Command::Scroll { x, y, delta } => {
                state.pending_input.extend([
                    egui::Event::PointerMoved(egui::pos2(x, y)),
                    egui::Event::MouseWheel {
                        source: egui::MouseWheelSource::Wheel,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, delta),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                super::request_frame(ctx);
                let _ = responder.send(Response::ok("Scroll queued", None));
            }
            _ => {}
        }
    }
}
