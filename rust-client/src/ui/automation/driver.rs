use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use super::registry::{ElementDescriptor, ElementValue, FrameSnapshot};
use crate::ui::Page;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum DirectorCommand {
    Tools,
    Call(crate::director::ToolCall),
    Page(String),
    GetPage,
    List {
        filter: Option<String>,
    },
    Inspect(String),
    Click(String),
    Set {
        target: String,
        value: ElementValue,
    },
    Get(String),
    Status,
    Wait(u64),
    Text(String),
    AudioFile(Option<std::path::PathBuf>),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectorResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl DirectorResponse {
    pub fn ok(message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            message: error.into(),
            data: None,
        }
    }
}

pub struct CommandEnvelope {
    pub command: DirectorCommand,
    pub responder: Sender<DirectorResponse>,
    pub expires: std::time::Instant,
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
pub struct AutomationFrameState {
    pub pages: Vec<crate::ui::NavigationPage>,
    pub pending_widget_reply: Option<Sender<DirectorResponse>>,
    pub active_page_name: String,
    pub elements: Vec<ElementDescriptor>,
    pub pending_click: Option<String>,
    pub pending_set: Option<(String, ElementValue)>,
    pub pending_page: Option<Page>,
    pub pending_onboarding_step: Option<usize>,
    pub pending_queries: Vec<CommandEnvelope>,
    pub pending_input: Vec<egui::Event>,
    pub last_snapshot: FrameSnapshot,
    pub action_performed_this_frame: bool,
}

pub struct AutomationDriver {
    context: std::sync::OnceLock<egui::Context>,
    pub(crate) frame_state: Mutex<AutomationFrameState>,
    command_tx: Sender<CommandEnvelope>,
    command_rx: Receiver<CommandEnvelope>,
    host_requests: Mutex<Vec<CommandEnvelope>>,
}

impl Default for AutomationDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationDriver {
    #[must_use]
    pub fn new() -> Self {
        let (command_tx, command_rx) = bounded(32);
        Self {
            context: Default::default(),
            frame_state: Mutex::new(AutomationFrameState::default()),
            command_tx,
            command_rx,
            host_requests: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn channel(&self) -> Sender<CommandEnvelope> {
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

    pub(crate) fn take_host_requests(&self) -> Vec<CommandEnvelope> {
        std::mem::take(&mut *self.host_requests.lock().unwrap())
    }

    /// Called at the start of each egui frame to process incoming director action commands.
    pub fn begin_frame(&self, ctx: &egui::Context, current_page: &str) {
        self.context.get_or_init(|| ctx.clone());
        super::attention::begin_frame(ctx);
        let mut state = self.frame_state.lock().unwrap();
        state.active_page_name = current_page.to_string();
        state.elements.clear();
        state.action_performed_this_frame = false;

        // Process any pending commands from external clients
        while let Ok(envelope) = self.command_rx.try_recv() {
            if envelope.expires <= std::time::Instant::now()
                || envelope
                    .cancelled
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                let _ = envelope.responder.send(DirectorResponse::err(
                    "Request cancelled or expired before execution",
                ));
                continue;
            }
            match envelope.command {
                DirectorCommand::Tools | DirectorCommand::Call(_) => {
                    self.host_requests.lock().unwrap().push(envelope);
                }
                DirectorCommand::Page(_)
                | DirectorCommand::Click(_)
                | DirectorCommand::Set { .. }
                | DirectorCommand::Wait(_)
                | DirectorCommand::Text(_)
                | DirectorCommand::AudioFile(_)
                | DirectorCommand::Viewport { .. }
                | DirectorCommand::Pointer { .. }
                | DirectorCommand::Scroll { .. } => {
                    self.execute_action_command(ctx, &mut state, envelope);
                    // Let this action reach its widget before accepting another mutation.
                    super::request_frame(ctx);
                    break;
                }
                DirectorCommand::GetPage
                | DirectorCommand::List { .. }
                | DirectorCommand::Inspect(_)
                | DirectorCommand::Get(_)
                | DirectorCommand::Status => {
                    state.pending_queries.push(envelope);
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
                DirectorResponse::ok(
                    "UI action completed",
                    Some(serde_json::json!({"page":current_page})),
                )
            } else {
                DirectorResponse::err("Control was not available or enabled in this frame")
            };
            state.pending_click = None;
            state.pending_set = None;
            let _ = responder.send(response);
        }

        let queries = std::mem::take(&mut state.pending_queries);
        for envelope in queries {
            self.execute_query_command(&state, envelope);
        }
    }

    fn execute_action_command(
        &self,
        ctx: &egui::Context,
        state: &mut AutomationFrameState,
        envelope: CommandEnvelope,
    ) {
        let CommandEnvelope {
            command, responder, ..
        } = envelope;
        match command {
            DirectorCommand::AudioFile(path) => {
                let response = match super::audio::configure(path) {
                    Ok(()) => DirectorResponse::ok("Audio input configured", None),
                    Err(error) => DirectorResponse::err(error),
                };
                let _ = responder.send(response);
            }
            DirectorCommand::Page(page_name) => {
                state.pending_click = None;
                state.pending_set = None;
                state.pending_input.clear();
                let target = page_name.trim().to_lowercase();
                if let Some(step_str) = target.strip_prefix("onboarding:") {
                    if let Ok(step) = step_str.parse::<usize>() {
                        state.pending_onboarding_step = Some(step);
                        state.pending_widget_reply = Some(responder);
                        return;
                    }
                }
                let page = state
                    .pages
                    .iter()
                    .find(|entry| {
                        entry.route.eq_ignore_ascii_case(&target)
                            || entry.title.eq_ignore_ascii_case(&target)
                            || entry.label.eq_ignore_ascii_case(&target)
                    })
                    .map(|entry| entry.page);
                if let Some(page) = page {
                    state.pending_page = Some(page);
                    state.pending_widget_reply = Some(responder);
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Page '{page_name}' is unavailable; use ui.pages to discover current routes"
                    )));
                }
            }
            DirectorCommand::Click(target) => {
                let element = match state.last_snapshot.resolve_element(&target) {
                    Ok(element) => element,
                    Err(error) => {
                        let _ = responder.send(DirectorResponse::err(error));
                        return;
                    }
                };
                state.pending_click = Some(element.id_hex.clone());
                state.pending_widget_reply = Some(responder);
            }
            DirectorCommand::Set { target, value } => {
                let element = match state.last_snapshot.resolve_element(&target) {
                    Ok(element) => element,
                    Err(error) => {
                        let _ = responder.send(DirectorResponse::err(error));
                        return;
                    }
                };
                state.pending_set = Some((element.id_hex.clone(), value));
                state.pending_widget_reply = Some(responder);
            }
            DirectorCommand::Wait(ms) => {
                let _ = responder.send(DirectorResponse::ok(format!("Waited {ms}ms"), None));
            }
            DirectorCommand::Viewport {
                width,
                height,
                scale,
            } => {
                if !(640.0..=4096.0).contains(&width)
                    || !(480.0..=2160.0).contains(&height)
                    || !(0.75..=3.0).contains(&scale)
                {
                    let _ = responder.send(DirectorResponse::err("Invalid viewport size or scale"));
                    return;
                }
                // Window commands use this frame's scale; the new scale takes effect next frame.
                let current_scale = ctx.pixels_per_point();
                ctx.set_pixels_per_point(scale);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    width / current_scale,
                    height / current_scale,
                )));
                let _ = responder.send(DirectorResponse::ok("Viewport updated", None));
            }
            DirectorCommand::Pointer { x, y, pressed } => {
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
                let _ = responder.send(DirectorResponse::ok("Pointer queued", None));
            }
            DirectorCommand::Text(text) => {
                state.pending_input.push(egui::Event::Text(text));
                super::request_frame(ctx);
                let _ = responder.send(DirectorResponse::ok("Text input queued", None));
            }
            DirectorCommand::Scroll { x, y, delta } => {
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
                let _ = responder.send(DirectorResponse::ok("Scroll queued", None));
            }
            _ => {}
        }
    }

    fn execute_query_command(&self, state: &AutomationFrameState, envelope: CommandEnvelope) {
        let CommandEnvelope {
            command, responder, ..
        } = envelope;
        let snapshot = &state.last_snapshot;
        match command {
            DirectorCommand::GetPage => {
                let _ = responder.send(DirectorResponse::ok(
                    "Current page",
                    Some(serde_json::json!({
                        "page": snapshot.page,
                    })),
                ));
            }
            DirectorCommand::List { filter } => {
                let elements: Vec<_> = if let Some(f) = filter {
                    let f_lower = f.to_lowercase();
                    snapshot
                        .elements
                        .iter()
                        .filter(|e| {
                            e.label.to_lowercase().contains(&f_lower)
                                || format!("{:?}", e.kind).to_lowercase().contains(&f_lower)
                        })
                        .cloned()
                        .collect()
                } else {
                    snapshot.elements.clone()
                };
                let _ = responder.send(DirectorResponse::ok(
                    format!(
                        "Found {} elements on page {}",
                        elements.len(),
                        snapshot.page
                    ),
                    Some(serde_json::to_value(&elements).unwrap_or_default()),
                ));
            }
            DirectorCommand::Inspect(target) => {
                if let Some(elem) = snapshot.find_element(&target) {
                    let _ = responder.send(DirectorResponse::ok(
                        format!("Element inspected: {}", elem.label),
                        Some(serde_json::to_value(elem).unwrap_or_default()),
                    ));
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Element '{target}' not found on page '{}'",
                        snapshot.page
                    )));
                }
            }
            DirectorCommand::Get(target) => {
                if let Some(elem) = snapshot.find_element(&target) {
                    let _ = responder.send(DirectorResponse::ok(
                        format!("Value for '{}'", elem.label),
                        Some(serde_json::json!({
                            "label": elem.label,
                            "kind": elem.kind,
                            "value": elem.value,
                            "enabled": elem.enabled,
                        })),
                    ));
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Element '{target}' not found on page '{}'",
                        snapshot.page
                    )));
                }
            }
            DirectorCommand::Status => {
                let _ = responder.send(DirectorResponse::ok(
                    "UI Director is active",
                    Some(serde_json::json!({
                        "active_page": snapshot.page,
                        "element_count": snapshot.elements.len(),
                    })),
                ));
            }
            _ => {}
        }
    }
}
