//! Observe existing microphone results; the review UI never owns an inference session.
use crate::{
    CaptureSource, XRTranslateApp, ask,
    session_coordinator::{TranslationEvent, TranslationSegment},
    voice_interaction::{VoiceController, VoiceEffect},
};
use eframe::egui;
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, PartialEq)]
struct Sentence {
    stream: u64,
    turn: String,
    index: u32,
}
impl Sentence {
    fn of(segment: &TranslationSegment) -> Self {
        Self {
            stream: segment.stream_id,
            turn: segment.turn_id.clone(),
            index: segment.segment_index,
        }
    }
}
struct SeenSentence {
    id: Sentence,
    instruction: bool,
    source_seen: bool,
}
struct Phrase {
    source: String,
    translated: String,
    target_language: String,
    instruction: bool,
}
#[derive(Default)]
struct Inbox {
    review: bool,
    editing: bool,
    seen: Vec<SeenSentence>,
    latest: Option<Sentence>,
    last_result: Option<(Sentence, String)>,
    phrases: VecDeque<Phrase>,
}
#[derive(Clone, Default)]
pub(crate) struct VoiceInbox(Arc<Mutex<Inbox>>);
impl VoiceInbox {
    fn set_mode(&self, review: bool, editing: bool) {
        let mut inbox = self.0.lock();
        if inbox.review != review || inbox.editing != editing {
            inbox.phrases.clear();
        }
        inbox.review = review;
        inbox.editing = editing;
    }

    /// Called only for public microphone results, after ordinary normalization.
    pub(crate) fn observe(&self, events: &[TranslationEvent]) {
        let mut inbox = self.0.lock();
        for event in events {
            let segments = match event {
                TranslationEvent::Segment(segment) => std::slice::from_ref(segment),
                TranslationEvent::ReplaceSegments(segments) => segments.as_slice(),
                _ => continue,
            };
            for segment in segments {
                if segment.audio_source != CaptureSource::Microphone
                    || segment.source.trim().is_empty()
                {
                    continue;
                }
                let id = Sentence::of(segment);
                let index = if let Some(index) = inbox.seen.iter().position(|seen| seen.id == id) {
                    index
                } else {
                    if inbox.seen.len() >= 128 {
                        inbox.seen.remove(0);
                    }
                    let instruction = inbox.review && inbox.editing;
                    inbox.seen.push(SeenSentence {
                        id: id.clone(),
                        instruction,
                        source_seen: false,
                    });
                    inbox.latest = Some(id.clone());
                    inbox.seen.len() - 1
                };
                // Fix a sentence's role at its first source fragment, before
                // an Edit gesture can change how its final result is used.
                if segment.revisable {
                    continue;
                }
                let instruction = inbox.seen[index].instruction;
                let source_seen = inbox.seen[index].source_seen;
                if segment.translated.is_none() {
                    inbox.seen[index].source_seen = true;
                }
                if !inbox.review {
                    continue;
                }
                if inbox.editing {
                    // Source and translation of the same sentence produce one Ask.
                    if !instruction || source_seen {
                        continue;
                    }
                    inbox.seen[index].source_seen = true;
                    inbox.phrases.push_back(Phrase {
                        source: segment.source.clone(),
                        translated: String::new(),
                        target_language: String::new(),
                        instruction: true,
                    });
                } else if !instruction && inbox.latest.as_ref() == Some(&id) {
                    let Some(translated) = segment
                        .translated
                        .as_ref()
                        .filter(|text| !text.trim().is_empty())
                    else {
                        continue;
                    };
                    if inbox
                        .last_result
                        .as_ref()
                        .is_some_and(|(previous, text)| previous == &id && text == translated)
                    {
                        continue;
                    }
                    inbox.last_result = Some((id, translated.clone()));
                    // Only the latest ordinary sentence needs a presentation slot.
                    inbox.phrases.clear();
                    inbox.phrases.push_back(Phrase {
                        source: segment.source.clone(),
                        translated: translated.clone(),
                        target_language: segment.target_lang.clone(),
                        instruction: false,
                    });
                }
            }
        }
    }
    fn take(&self) -> VecDeque<Phrase> {
        std::mem::take(&mut self.0.lock().phrases)
    }
}

struct Preparation {
    text: String,
    tool: String,
    request: crate::plugin_host::Call,
    result: Option<Result<String, String>>,
}
#[derive(Default)]
pub(crate) struct VoiceHost {
    controller: VoiceController,
    actions: VecDeque<crate::voice_interaction::VoiceAction>,
    pub inbox: VoiceInbox,
    pub microphone_sync_active: Arc<AtomicBool>,
    ask: Option<(u64, ask::Reply)>,
    publication: Option<(u64, crate::plugin_host::Call)>,
    preparation: Option<Preparation>,
}
impl VoiceHost {
    pub fn action(&mut self, action: crate::voice_interaction::VoiceAction) {
        if self.actions.len() < 16 {
            self.actions.push_back(action);
        }
    }
}
impl XRTranslateApp {
    fn voice_output(&self) -> Option<String> {
        self.plugin_host
            .states()
            .find_map(|state| state["voice_output"].as_str().map(str::to_owned))
    }
    pub(crate) fn refresh_microphone_sync(&self) {
        let receiver = self
            .plugin_host
            .states()
            .find_map(|state| {
                serde_json::from_value::<xrtranslate_client_core::osc::ReceiveOptions>(
                    state["osc_receiver"].clone(),
                )
                .ok()
            })
            .unwrap_or_default();
        self.osc_transport.handle().update_settings(&receiver);
        let enabled = !self.first_run && self.microphone_sync_enabled && receiver.enabled;
        self.voice_interaction
            .microphone_sync_active
            .store(enabled, Ordering::Release);
        self.osc_transport.handle().set_microphone_sync(enabled);
    }

    /// Explicit microphone intent, not capture startup, failure or model cleanup.
    pub(crate) fn sync_game_microphone(&self, enabled: bool) {
        self.refresh_microphone_sync();
        if self
            .voice_interaction
            .microphone_sync_active
            .load(Ordering::Acquire)
        {
            self.osc_transport.handle().request_microphone(enabled);
        }
    }

    pub(crate) fn poll_microphone_sync(&mut self, ctx: &egui::Context) {
        self.refresh_microphone_sync();
        if self.osc_transport.handle().take_microphone_mute() {
            self.set_microphone_input(false, Some(ctx.clone()), false, Self::start_task_capture);
        }
        if self
            .voice_interaction
            .microphone_sync_active
            .load(Ordering::Acquire)
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn voice_effects(&mut self, effects: Vec<VoiceEffect>, ctx: &egui::Context) {
        let mut effects = VecDeque::from(effects);
        while let Some(effect) = effects.pop_front() {
            match effect {
                VoiceEffect::Ask { operation, prompt } => {
                    self.voice_interaction.ask = Some((operation, self.ask.request(prompt, ctx)));
                }
                VoiceEffect::Publish { operation, text } => {
                    if let Some(tool) = self.voice_output() {
                        self.voice_interaction.publication = Some((
                            operation,
                            crate::plugin_host::Call::new(
                                tool,
                                serde_json::json!({"action":"publish","operation":operation,"text":text}),
                            ),
                        ));
                    } else {
                        effects.extend(self.voice_interaction.controller.publish_done(
                            operation,
                            Err("Enable a text output to send this message.".into()),
                        ));
                    }
                }
            }
        }
        let snapshot = self.voice_interaction.controller.snapshot();
        self.voice_interaction
            .inbox
            .set_mode(snapshot.review_enabled, snapshot.instruction);
    }

    pub(crate) fn poll_voice_interaction(&mut self, ctx: &egui::Context) {
        let mode = self
            .plugin_host
            .states()
            .filter_map(|state| {
                serde_json::from_value::<xrtranslate_client_core::companion::VoiceMode>(
                    state["voice"].clone(),
                )
                .ok()
            })
            .fold(
                xrtranslate_client_core::companion::VoiceMode::default(),
                |mut all, mode| {
                    all.requested |= mode.requested;
                    all.active |= mode.active;
                    all.review |= mode.review;
                    all
                },
            );
        let requested = !self.first_run && mode.requested;
        let active = requested && mode.active;
        let review = requested && mode.review;
        self.plugin_host
            .set_context("voice.review", serde_json::json!(review));
        self.voice_interaction
            .controller
            .set_edit_available(self.service_config.supports_ask());
        let effects = self
            .voice_interaction
            .controller
            .set_enabled(active, review);
        self.voice_effects(effects, ctx);
        for action in std::mem::take(&mut self.voice_interaction.actions) {
            if !active {
                continue;
            }
            let effects = self.voice_interaction.controller.apply(action);
            self.voice_effects(effects, ctx);
        }

        for phrase in self.voice_interaction.inbox.take() {
            if phrase.instruction == self.voice_interaction.controller.is_editing() {
                let effects = self.voice_interaction.controller.observe_phrase(
                    &phrase.source,
                    &phrase.translated,
                    &phrase.target_language,
                );
                self.voice_effects(effects, ctx);
            }
        }
        if let Some((operation, result)) =
            self.voice_interaction
                .ask
                .as_ref()
                .and_then(|(id, rx)| match rx.try_recv() {
                    Ok(result) => Some((*id, result)),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        Some((*id, Err("Editing ended without an answer.".into())))
                    }
                    Err(_) => None,
                })
        {
            self.voice_interaction.ask = None;
            let effects = self
                .voice_interaction
                .controller
                .ask_done(operation, result.map_err(|error| error.to_string()));
            self.voice_effects(effects, ctx);
        }
        if let Some((operation, result)) =
            self.voice_interaction
                .publication
                .as_mut()
                .and_then(|(id, call)| {
                    call.poll(&mut self.plugin_host)
                        .map(|result| (*id, result.map(|_| ())))
                })
        {
            self.voice_interaction.publication = None;
            let effects = self
                .voice_interaction
                .controller
                .publish_done(operation, result);
            self.voice_effects(effects, ctx);
        }
        let mut snapshot = self.voice_interaction.controller.snapshot();
        snapshot.recording = active && self.input_capturing(CaptureSource::Microphone);
        let tool = self.voice_output();
        if snapshot.review_enabled
            && !snapshot.text.is_empty()
            && let Some(tool) = &tool
        {
            if self
                .voice_interaction
                .preparation
                .as_ref()
                .is_none_or(|p| p.text != snapshot.text || &p.tool != tool)
            {
                if let Some(previous) = self.voice_interaction.preparation.take() {
                    previous.request.cancel(&mut self.plugin_host);
                }
                self.voice_interaction.preparation = Some(Preparation {
                    request: crate::plugin_host::Call::new(
                        tool.clone(),
                        serde_json::json!({"action":"prepare","text":snapshot.text}),
                    ),
                    text: snapshot.text.clone(),
                    tool: tool.clone(),
                    result: None,
                });
            }
            let preparation = self.voice_interaction.preparation.as_mut().unwrap();
            if preparation.result.is_none() {
                preparation.result =
                    preparation
                        .request
                        .poll(&mut self.plugin_host)
                        .map(|result| {
                            result.and_then(|value| {
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .ok_or("Output returned no text".into())
                            })
                        });
            }
            match &preparation.result {
                Some(Ok(text)) => snapshot.text = text.clone(),
                Some(Err(error)) => {
                    snapshot.can_send = false;
                    snapshot.status = error.clone();
                }
                None => snapshot.can_send = false,
            }
        } else if let Some(previous) = self.voice_interaction.preparation.take() {
            previous.request.cancel(&mut self.plugin_host);
        }
        if tool.is_none() {
            snapshot.can_send = false;
            if snapshot.review_enabled {
                snapshot.status = "Enable a text output".into();
            }
        }
        self.plugin_host
            .set_context("voice.state", serde_json::json!(snapshot));
        if requested {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }
}
