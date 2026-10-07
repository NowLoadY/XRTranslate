use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use super::registry::{ElementDescriptor, ElementValue, FrameSnapshot};
use crate::ui::Page;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum DirectorCommand {
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
}

#[derive(Default)]
pub struct AutomationFrameState {
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
    pub(crate) frame_state: Mutex<AutomationFrameState>,
    command_tx: Sender<CommandEnvelope>,
    command_rx: Receiver<CommandEnvelope>,
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
            frame_state: Mutex::new(AutomationFrameState::default()),
            command_tx,
            command_rx,
        }
    }

    #[must_use]
    pub fn channel(&self) -> Sender<CommandEnvelope> {
        self.command_tx.clone()
    }

    /// Called at the start of each egui frame to process incoming director action commands.
    pub fn begin_frame(&self, ctx: &egui::Context, current_page: &str) {
        let mut state = self.frame_state.lock().unwrap();
        state.active_page_name = current_page.to_string();
        state.elements.clear();
        state.action_performed_this_frame = false;

        // Process any pending commands from external clients
        while let Ok(envelope) = self.command_rx.try_recv() {
            match envelope.command {
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
                    ctx.request_repaint();
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
        let CommandEnvelope { command, responder } = envelope;
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
                        let _ = responder.send(DirectorResponse::ok(
                            format!("Navigating to Onboarding Step {step}"),
                            None,
                        ));
                        return;
                    }
                }
                let page = match target.as_str() {
                    "translation" => Some(Page::Translation),
                    "settings" => Some(Page::Settings),
                    "audiostudio" | "audio_studio" | "audio-studio" | "audio" => {
                        Some(Page::AudioStudio)
                    }
                    "ttscenter" | "tts_center" | "tts-center" | "tts" => Some(Page::TtsCenter),
                    "promptstudio" | "prompt_studio" | "prompt-studio" | "prompt" => {
                        Some(Page::PromptStudio)
                    }
                    "corpusstudio" | "corpus" | "corpus_studio" | "corpus-studio"
                    | "vocabulary" => Some(Page::CorpusStudio),
                    _ => {
                        let plugin = target.strip_prefix("plugin:").unwrap_or(&target);
                        let plugin = match plugin {
                            "vroverlay" => "vr_overlay",
                            "media" | "videoplayer" | "player" => "video_player",
                            plugin => plugin,
                        };
                        crate::plugins::PluginId::parse(plugin)
                            .filter(|id| id.is_supported())
                            .map(Page::Plugin)
                    }
                };
                if let Some(page) = page {
                    state.pending_page = Some(page);
                    let _ = responder.send(DirectorResponse::ok(
                        format!("Navigating to page: {page_name}"),
                        None,
                    ));
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Unknown page '{page_name}'. Valid pages: Translation, Settings, AudioStudio, PromptStudio, CorpusStudio, TtsCenter, plugin:<id>, onboarding:<step>"
                    )));
                }
            }
            DirectorCommand::Click(target) => {
                state.pending_click = Some(
                    state
                        .last_snapshot
                        .find_element(&target)
                        .map_or_else(|| target.clone(), |element| element.id_hex.clone()),
                );
                let _ = responder.send(DirectorResponse::ok(
                    format!("Scheduled click on '{target}'"),
                    None,
                ));
            }
            DirectorCommand::Set { target, value } => {
                let resolved = state
                    .last_snapshot
                    .find_element(&target)
                    .map_or_else(|| target.clone(), |element| element.id_hex.clone());
                state.pending_set = Some((resolved, value.clone()));
                let _ = responder.send(DirectorResponse::ok(
                    format!("Queued set on '{target}' to {value:?}"),
                    None,
                ));
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
                ctx.request_repaint();
                let _ = responder.send(DirectorResponse::ok("Pointer queued", None));
            }
            DirectorCommand::Text(text) => {
                state.pending_input.push(egui::Event::Text(text));
                ctx.request_repaint();
                let _ = responder.send(DirectorResponse::ok("Text input queued", None));
            }
            DirectorCommand::Scroll { x, y, delta } => {
                state.pending_input.extend([
                    egui::Event::PointerMoved(egui::pos2(x, y)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, delta),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                ctx.request_repaint();
                let _ = responder.send(DirectorResponse::ok("Scroll queued", None));
            }
            _ => {}
        }
    }

    fn execute_query_command(&self, state: &AutomationFrameState, envelope: CommandEnvelope) {
        let CommandEnvelope { command, responder } = envelope;
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
