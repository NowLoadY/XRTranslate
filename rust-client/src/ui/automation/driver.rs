use std::sync::Mutex;
use crossbeam_channel::{Receiver, Sender, bounded};
use serde::{Deserialize, Serialize};

use super::registry::{ElementDescriptor, ElementValue, FrameSnapshot};
use crate::ui::Page;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum DirectorCommand {
    Page(String),
    GetPage,
    List { filter: Option<String> },
    Inspect(String),
    Click(String),
    Set { target: String, value: ElementValue },
    Get(String),
    Status,
    Wait(u64),
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

    /// Called at the start of each egui frame to process incoming director commands.
    pub fn begin_frame(&self, current_page: &str) {
        let mut state = self.frame_state.lock().unwrap();
        state.active_page_name = current_page.to_string();
        state.elements.clear();
        state.action_performed_this_frame = false;

        // Process any pending commands from external clients
        while let Ok(envelope) = self.command_rx.try_recv() {
            self.execute_command(&mut state, envelope);
        }
    }

    fn execute_command(&self, state: &mut AutomationFrameState, envelope: CommandEnvelope) {
        let CommandEnvelope { command, responder } = envelope;
        match command {
            DirectorCommand::GetPage => {
                let _ = responder.send(DirectorResponse::ok(
                    "Current page",
                    Some(serde_json::json!({
                        "page": state.active_page_name,
                    })),
                ));
            }
            DirectorCommand::Page(page_name) => {
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
                    "promptstudio" | "prompt_studio" | "prompt-studio" | "prompt" => {
                        Some(Page::PromptStudio)
                    }
                    "osc" | "plugin:osc" => {
                        Some(Page::Plugin(crate::plugins::PluginId::OSC))
                    }
                    "meeting" | "plugin:meeting" => {
                        Some(Page::Plugin(crate::plugins::PluginId::MEETING))
                    }
                    "vroverlay" | "vr_overlay" | "plugin:vr_overlay" => {
                        Some(Page::Plugin(crate::plugins::PluginId::VR_OVERLAY))
                    }
                    "videoplayer" | "video_player" | "player" | "plugin:video_player" => {
                        Some(Page::Plugin(crate::plugins::PluginId::VIDEO_PLAYER))
                    }
                    _ => None,
                };
                if let Some(page) = page {
                    state.pending_page = Some(page);
                    let _ = responder.send(DirectorResponse::ok(
                        format!("Navigating to page: {page_name}"),
                        None,
                    ));
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Unknown page '{page_name}'. Valid pages: Translation, Settings, AudioStudio, PromptStudio, osc, meeting, vr_overlay, video_player, onboarding:<step>"
                    )));
                }
            }
            DirectorCommand::List { filter } => {
                let elements: Vec<_> = if let Some(f) = filter {
                    let f_lower = f.to_lowercase();
                    state
                        .last_snapshot
                        .elements
                        .iter()
                        .filter(|e| {
                            e.label.to_lowercase().contains(&f_lower)
                                || format!("{:?}", e.kind).to_lowercase().contains(&f_lower)
                        })
                        .cloned()
                        .collect()
                } else {
                    state.last_snapshot.elements.clone()
                };
                let _ = responder.send(DirectorResponse::ok(
                    format!("Found {} elements on page {}", elements.len(), state.last_snapshot.page),
                    Some(serde_json::to_value(&elements).unwrap_or_default()),
                ));
            }
            DirectorCommand::Inspect(target) => {
                if let Some(elem) = state.last_snapshot.find_element(&target) {
                    let _ = responder.send(DirectorResponse::ok(
                        format!("Element inspected: {}", elem.label),
                        Some(serde_json::to_value(elem).unwrap_or_default()),
                    ));
                } else {
                    let _ = responder.send(DirectorResponse::err(format!(
                        "Element '{target}' not found in current frame"
                    )));
                }
            }
            DirectorCommand::Click(target) => {
                state.pending_click = Some(target.clone());
                let _ = responder.send(DirectorResponse::ok(
                    format!("Scheduled click on '{target}'"),
                    None,
                ));
            }
            DirectorCommand::Set { target, value } => {
                state.pending_set = Some((target.clone(), value.clone()));
                let _ = responder.send(DirectorResponse::ok(
                    format!("Queued set on '{target}' to {value:?}"),
                    None,
                ));
            }
            DirectorCommand::Get(target) => {
                if let Some(elem) = state.last_snapshot.find_element(&target) {
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
                        "Element '{target}' not found in current frame"
                    )));
                }
            }
            DirectorCommand::Status => {
                let _ = responder.send(DirectorResponse::ok(
                    "Application ready",
                    Some(serde_json::json!({
                        "page": state.active_page_name,
                        "elements_count": state.last_snapshot.elements.len(),
                    })),
                ));
            }
            DirectorCommand::Wait(ms) => {
                let _ = responder.send(DirectorResponse::ok(format!("Waited {ms} ms"), None));
            }
        }
    }

    /// Called at the end of each egui frame to finalize snapshot.
    pub fn finish_frame(&self) {
        let mut state = self.frame_state.lock().unwrap();
        state.last_snapshot = FrameSnapshot {
            page: state.active_page_name.clone(),
            elements: state.elements.clone(),
        };
    }
}
