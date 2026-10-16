//! Observe existing microphone results; the review UI never owns an inference session.
use crate::{
    CaptureSource, XRTranslateApp, ask,
    plugins::PluginId,
    session_coordinator::{TranslationEvent, TranslationSegment},
    voice_interaction::{VoiceAction, VoiceController, VoiceEffect},
};
use crossbeam_channel::Receiver;
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

#[derive(Default)]
pub(crate) struct VoiceHost {
    controller: VoiceController,
    pub inbox: VoiceInbox,
    active: bool,
    pub microphone_sync_active: Arc<AtomicBool>,
    ask: Option<(u64, ask::Reply)>,
    publication: Option<(u64, Receiver<Result<(), String>>)>,
}
impl XRTranslateApp {
    pub(crate) fn refresh_microphone_sync(&self) {
        let enabled = !self.first_run
            && self.microphone_sync_enabled
            && self.plugin_enabled(PluginId::OSC)
            && self.osc_plugin.applied().enabled;
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
            self.vr_overlay_plugin.manager().dock_microphone();
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
                    match self.osc_plugin.prepare_voice_text(&text) {
                        Ok(text) if self.plugin_enabled(PluginId::OSC) => {
                            self.voice_interaction.publication =
                                Some((operation, self.osc_plugin.publish_voice(operation, &text)));
                        }
                        result => {
                            let error = result.err().unwrap_or_else(|| {
                                "Enable VRChat OSC to send this message.".into()
                            });
                            effects.extend(
                                self.voice_interaction
                                    .controller
                                    .publish_done(operation, Err(error)),
                            );
                        }
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
        let settings = self.vr_overlay_plugin.draft();
        let requested = !self.first_run
            && self.plugin_enabled(PluginId::VR_OVERLAY)
            && settings.enabled
            && settings.interaction_enabled;
        let review = requested && settings.review_before_send;
        let runtime = self.vr_overlay_plugin.manager().status();
        let active = requested && runtime.steamvr_connected && runtime.interaction_available;
        // Only entering/leaving spatial control or a real proximity event changes
        // capture. Review, Send and Ask completion never call an audio control.
        if active != self.voice_interaction.active {
            // An idle surface may still have an old off event. It must not
            // become a game-mute gesture when spatial control first starts.
            self.vr_overlay_plugin.manager().take_voice_actions();
            self.set_microphone_input(false, Some(ctx.clone()), false, Self::start_task_capture);
            if self.voice_interaction.active {
                self.sync_game_microphone(false);
            }
        }
        self.voice_interaction.active = active;
        self.osc_plugin.set_microphone_review(review);
        self.voice_interaction
            .controller
            .set_edit_available(self.service_config.supports_chat());
        let effects = self
            .voice_interaction
            .controller
            .set_enabled(active, review);
        self.voice_effects(effects, ctx);
        for action in self.vr_overlay_plugin.manager().take_voice_actions() {
            if !active {
                continue;
            }
            if let VoiceAction::Microphone(near) = action {
                self.set_microphone_input(near, Some(ctx.clone()), true, Self::start_task_capture);
                self.sync_game_microphone(near);
            } else {
                let effects = self.voice_interaction.controller.apply(action);
                self.voice_effects(effects, ctx);
            }
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
                .ask_done(operation, result);
            self.voice_effects(effects, ctx);
        }
        if let Some((operation, result)) =
            self.voice_interaction
                .publication
                .as_ref()
                .and_then(|(id, rx)| match rx.try_recv() {
                    Ok(result) => Some((*id, result)),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        Some((*id, Err("OSC sender disconnected.".into())))
                    }
                    Err(_) => None,
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
        if snapshot.review_enabled && !snapshot.text.is_empty() {
            match self.osc_plugin.prepare_voice_text(&snapshot.text) {
                Ok(text) => snapshot.text = text,
                Err(error) if snapshot.can_send => {
                    snapshot.can_send = false;
                    snapshot.status = error;
                }
                _ => {}
            }
        }
        if !self.plugin_enabled(PluginId::OSC) {
            snapshot.can_send = false;
            if snapshot.review_enabled {
                snapshot.status = "Enable VRChat OSC to send the review text".into();
            }
        }
        self.vr_overlay_plugin
            .manager()
            .set_voice_snapshot(snapshot);
        if requested {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }
}
