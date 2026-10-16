//! One replaceable review slot; capture and pipeline lifecycles belong to the host.
mod prompt;

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceAction {
    /// Routed directly by the host, independently of review actions.
    Microphone(bool),
    Send,
    Revise,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct VoiceSnapshot {
    pub enabled: bool,
    pub review_enabled: bool,
    /// Filled by the host from actual microphone capture state.
    pub recording: bool,
    pub instruction: bool,
    pub can_send: bool,
    pub can_revise: bool,
    pub busy: bool,
    pub text: String,
    pub status: String,
    /// The edit button exits editing and retains already applied changes.
    pub revise_cancels: bool,
}

#[derive(Debug)]
pub(crate) enum VoiceEffect {
    Ask { operation: u64, prompt: String },
    Publish { operation: u64, text: String },
}

#[derive(Default)]
struct Slot {
    source: String,
    text: String,
    target_language: String,
    version: u64,
}

#[derive(Default)]
pub(crate) struct VoiceController {
    enabled: bool,
    review_enabled: bool,
    edit_available: bool,
    editing: bool,
    slot: Slot,
    next_operation: u64,
    ask_operation: Option<u64>,
    // Each spoken instruction becomes one Ask, based on the previous result.
    instructions: VecDeque<String>,
    publication: Option<(u64, u64)>,
    sent_version: Option<u64>,
    status: String,
}

impl VoiceController {
    pub fn set_edit_available(&mut self, available: bool) {
        self.edit_available = available;
        if !available && self.editing {
            self.exit_editing();
            self.status = "Selected model cannot edit; current text kept".into();
        }
    }

    pub fn set_enabled(&mut self, interaction: bool, review: bool) -> Vec<VoiceEffect> {
        if self.enabled == interaction && self.review_enabled == review {
            return Vec::new();
        }
        self.enabled = interaction;
        self.review_enabled = review;
        if !interaction || !review {
            self.exit_editing();
        }
        if !review {
            self.slot = Slot::default();
            self.sent_version = None;
        }
        self.ready();
        Vec::new()
    }

    pub fn apply(&mut self, action: VoiceAction) -> Vec<VoiceEffect> {
        if !self.enabled || !self.review_enabled {
            return Vec::new();
        }
        match action {
            VoiceAction::Microphone(_) => Vec::new(),
            VoiceAction::Revise if self.editing => {
                self.exit_editing();
                self.status = "Editing finished; changes kept".into();
                Vec::new()
            }
            VoiceAction::Revise if self.snapshot().can_revise => {
                self.editing = true;
                self.status = "Editing: speak a change; tap Edit again to finish".into();
                Vec::new()
            }
            VoiceAction::Send if self.snapshot().can_send => {
                self.exit_editing();
                let operation = self.operation();
                self.publication = Some((operation, self.slot.version));
                self.status = "Sending".into();
                vec![VoiceEffect::Publish {
                    operation,
                    text: self.slot.text.clone(),
                }]
            }
            _ => Vec::new(),
        }
    }

    /// The host supplies one stable utterance and owns utterance deduplication.
    /// While editing, its recognized source becomes an Ask instruction.
    pub fn observe_phrase(
        &mut self,
        source: &str,
        translated: &str,
        target_language: &str,
    ) -> Vec<VoiceEffect> {
        if !self.review_enabled {
            return Vec::new();
        }
        if self.editing {
            let instruction = source.trim();
            if !instruction.is_empty() {
                self.instructions.push_back(instruction.to_owned());
            }
            return self.next_instruction();
        }
        let text = if translated.trim().is_empty() {
            source.trim()
        } else {
            translated.trim()
        };
        if text.is_empty() {
            return Vec::new();
        }
        let version = self.operation();
        self.slot = Slot {
            source: source.trim().to_owned(),
            text: text.to_owned(),
            target_language: target_language.trim().to_owned(),
            version,
        };
        self.ready();
        Vec::new()
    }

    pub fn ask_done(&mut self, operation: u64, result: Result<String, String>) -> Vec<VoiceEffect> {
        if !self.editing || self.ask_operation != Some(operation) {
            return Vec::new();
        }
        self.ask_operation = None;
        match result.and_then(prompt::accept) {
            Ok(text) => {
                if self.slot.text != text {
                    self.slot.text = text;
                    self.slot.version = self.operation();
                    self.status = "Change applied; speak another change or finish editing".into();
                } else {
                    self.status = "Text unchanged; speak another change or finish editing".into();
                }
            }
            Err(error) => self.status = format!("Edit failed; current text kept: {error}"),
        }
        self.next_instruction()
    }

    pub fn publish_done(&mut self, operation: u64, result: Result<(), String>) -> Vec<VoiceEffect> {
        let Some((current, version)) = self.publication else {
            return Vec::new();
        };
        if current != operation {
            return Vec::new();
        }
        self.publication = None;
        match result {
            Ok(()) => {
                self.sent_version = Some(version);
                if self.slot.version == version {
                    self.status = "Message submitted; the next phrase replaces it".into();
                }
            }
            Err(error) => self.status = format!("Not sent; review text kept: {error}"),
        }
        Vec::new()
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn snapshot(&self) -> VoiceSnapshot {
        let available = self.enabled && self.review_enabled && !self.slot.text.is_empty();
        VoiceSnapshot {
            enabled: self.enabled,
            review_enabled: self.review_enabled,
            recording: false,
            instruction: self.editing,
            can_send: available
                && self.publication.is_none()
                && self.sent_version != Some(self.slot.version),
            can_revise: available && self.edit_available && self.publication.is_none(),
            busy: self.ask_operation.is_some() || self.publication.is_some(),
            text: if self.review_enabled {
                self.slot.text.clone()
            } else {
                String::new()
            },
            status: if !self.enabled {
                "Disabled".into()
            } else if !self.review_enabled {
                "Microphone ready".into()
            } else {
                self.status.clone()
            },
            revise_cancels: self.editing,
        }
    }

    fn operation(&mut self) -> u64 {
        self.next_operation += 1;
        self.next_operation
    }

    fn exit_editing(&mut self) {
        self.editing = false;
        self.ask_operation = None;
        self.instructions.clear();
    }

    fn next_instruction(&mut self) -> Vec<VoiceEffect> {
        if !self.editing || self.ask_operation.is_some() {
            return Vec::new();
        }
        let Some(instruction) = self.instructions.pop_front() else {
            return Vec::new();
        };
        let operation = self.operation();
        self.ask_operation = Some(operation);
        self.status = "Applying spoken change; tap Edit to finish".into();
        vec![VoiceEffect::Ask {
            operation,
            prompt: prompt::build(
                &self.slot.source,
                &self.slot.text,
                &self.slot.target_language,
                &instruction,
            ),
        }]
    }

    fn ready(&mut self) {
        self.status = if self.slot.text.is_empty() {
            "Speak to fill the review slot"
        } else {
            "Latest phrase: edit or send"
        }
        .into();
    }
}
