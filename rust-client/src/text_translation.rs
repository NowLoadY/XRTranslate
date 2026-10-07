//! Finite text tasks use the same scoped result stream as audio translation.
use crate::{
    CaptureSource,
    backend::{BackendManager, BackendStart, BackendStatus},
    network::{self, ExternalAudioGate, SessionConfig, SessionEvent, SessionHandle},
    session_coordinator::{TranslationInput, TranslationSessionOwner, TranslationTask},
    translation_service::{ChannelScope, TaskEvent},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection};
use xrtranslate_protocol::PromptGraphSet;

pub(crate) fn select_languages_with_options(
    text: &str,
    source: &str,
    target: &str,
    capabilities: LanguageCapabilities,
    additional_target: Option<&str>,
    asr_only: bool,
) -> Result<LanguageSelection, String> {
    let capabilities = capabilities.for_text();
    if asr_only {
        return capabilities.select_with_options(source, target, None, true);
    }
    if let Some((source, target)) =
        xrtranslate_engine::auto_route_language_pair(text, source, target)
        && let Ok(selection) =
            capabilities.select_with_options(source, target, additional_target, false)
    {
        return Ok(selection);
    }
    capabilities.select_with_options(source, target, additional_target, false)
}

struct TextConnection {
    session: Option<SessionHandle>,
    tx: Sender<SessionEvent>,
    rx: Receiver<SessionEvent>,
    drained: bool,
}

impl Default for TextConnection {
    fn default() -> Self {
        let (tx, rx) = bounded(128);
        Self {
            session: None,
            tx,
            rx,
            drained: false,
        }
    }
}

impl TextConnection {
    fn close(&mut self) {
        if let Some(session) = self.session.take() {
            session.cancel();
        }
    }
}

struct TextTask {
    scope: Arc<ChannelScope>,
    pending: Option<String>,
    languages: LanguageSelection,
    connection: TextConnection,
    started: Instant,
    segments: BTreeMap<u32, String>,
    segment_count: u32,
    terminal: bool,
}

impl TextTask {
    fn same_conversation(&self, other: &Self) -> bool {
        self.scope.owner.same_conversation(&other.scope.owner)
            && self.languages == other.languages
            && self.scope.publish_to_host_outputs == other.scope.publish_to_host_outputs
    }

    fn send_pending(&mut self) -> Result<(), String> {
        if let Some(text) = &self.pending {
            let (source, destination) = self.languages.wire();
            self.connection
                .session
                .as_ref()
                .ok_or("Text session is unavailable")?
                .translate_text(text, Some(source), Some(destination))?;
            self.pending = None;
            self.connection.drained = false;
            self.started = Instant::now();
        }
        Ok(())
    }

    fn emit(&self, event: SessionEvent, target: &Sender<TaskEvent>) {
        let _ = target.send(TaskEvent {
            scope: self.scope.clone(),
            event,
        });
    }

    fn finish(&mut self, reason: String, target: &Sender<TaskEvent>) {
        self.emit(SessionEvent::Disconnected(reason), target);
        self.terminal = true;
        // The event pump still needs this scope to deliver queued results.
    }

    fn fail(&mut self, error: String, target: &Sender<TaskEvent>, errors: &mut Vec<String>) {
        self.connection.close();
        self.emit(SessionEvent::Error(error.clone()), target);
        self.finish(error.clone(), target);
        if self.scope.publish_to_host_outputs {
            errors.push(error);
        }
    }

    fn cancel(&self) {
        self.scope.active.store(false, Ordering::Release);
        if let Some(session) = &self.connection.session {
            session.cancel();
        }
    }
}

pub(crate) struct TextTranslation {
    tasks: Vec<TextTask>,
    completed: VecDeque<CompletedText>,
    backend_preparing: bool,
    next_poll: Instant,
}

/// Completed finite text output, independent of any clipboard or UI policy.
pub(crate) struct CompletedText {
    pub owner: TranslationSessionOwner,
    pub translated: String,
}

#[derive(Default)]
pub(crate) struct TextPoll {
    pub errors: Vec<String>,
    pub completed: VecDeque<CompletedText>,
}

impl Default for TextTranslation {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            completed: VecDeque::new(),
            backend_preparing: false,
            next_poll: Instant::now(),
        }
    }
}

impl TextTranslation {
    pub(crate) fn busy(&self) -> bool {
        self.tasks.iter().any(|task| !task.terminal)
    }

    #[cfg(test)]
    pub(crate) fn preparing(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| !task.terminal && task.pending.is_some())
    }

    pub(crate) fn preparing_host(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| !task.terminal && task.pending.is_some() && task.scope.owner.is_host())
    }

    #[cfg(test)]
    pub(crate) fn preparing_for(&self, plugin_id: &str) -> bool {
        self.tasks.iter().any(|task| {
            !task.terminal && task.pending.is_some() && task.scope.owner.is_plugin(plugin_id)
        })
    }

    pub(crate) fn owner_active(&self, plugin_id: &str) -> bool {
        self.tasks
            .iter()
            .any(|task| task.scope.owner.is_plugin(plugin_id) && task.scope.accepts_events())
    }

    pub(crate) fn scopes(&self) -> impl Iterator<Item = &Arc<ChannelScope>> {
        self.tasks.iter().map(|task| &task.scope)
    }

    fn retire_finished(&mut self) {
        for task in self.tasks.iter_mut().filter(|task| task.terminal) {
            if task.scope.finished.load(Ordering::Acquire) && !task.segments.is_empty() {
                let segments = std::mem::take(&mut task.segments);
                if task.scope.active.load(Ordering::Acquire)
                    && !task.scope.failed.load(Ordering::Acquire)
                {
                    let mut translated = String::new();
                    for segment in segments.into_values() {
                        crate::streaming::append_segment(&mut translated, &segment);
                    }
                    if !translated.trim().is_empty() {
                        if self.completed.len() == 32 {
                            self.completed.pop_front();
                        }
                        self.completed.push_back(CompletedText {
                            owner: task.scope.owner.clone(),
                            translated,
                        });
                    }
                }
            }
            while let Ok(event) = task.connection.rx.try_recv() {
                match event {
                    SessionEvent::StreamEnded { .. } => task.connection.drained = true,
                    SessionEvent::Error(_)
                    | SessionEvent::BackendError { .. }
                    | SessionEvent::Disconnected(_) => task.connection.close(),
                    _ => {}
                }
            }
            if !task.connection.drained && task.started.elapsed() > Duration::from_secs(180) {
                task.connection.close();
            }
        }
        self.tasks.retain(|task| {
            task.scope.accepts_events()
                || (task.scope.active.load(Ordering::Acquire) && task.connection.session.is_some())
        });
    }

    fn reuse_connections(&mut self) {
        for index in 0..self.tasks.len() {
            let task = &self.tasks[index];
            if task.terminal || task.connection.session.is_some() {
                continue;
            }
            let Some(previous) = self.tasks[..index]
                .iter()
                .position(|previous| previous.same_conversation(task))
            else {
                continue;
            };
            let (earlier, waiting) = self.tasks.split_at_mut(index);
            let previous = &mut earlier[previous];
            let task = &mut waiting[0];
            if previous.terminal
                && previous.scope.finished.load(Ordering::Acquire)
                && previous.connection.drained
                && previous.connection.session.is_some()
            {
                std::mem::swap(&mut previous.connection, &mut task.connection);
                task.scope.stream_id.store(
                    task.connection.session.as_ref().unwrap().stream_id(),
                    Ordering::Release,
                );
                let _ = task.connection.tx.send(SessionEvent::Connected);
                if let Err(error) = task.send_pending() {
                    let _ = task.connection.tx.send(SessionEvent::Error(error));
                }
            }
        }
        self.retire_finished();
        // Keep a small bounded cache of conversations, most recently used last.
        let mut idle = 0;
        for index in (0..self.tasks.len()).rev() {
            let task = &self.tasks[index];
            if task.terminal && !task.scope.accepts_events() {
                idle += 1;
                if idle > 8 {
                    self.tasks.remove(index);
                }
            }
        }
    }

    fn needs_connection(&self, index: usize) -> bool {
        let task = &self.tasks[index];
        !task.terminal
            && task.connection.session.is_none()
            && !self.tasks[..index]
                .iter()
                .any(|previous| previous.same_conversation(task))
    }

    /// Each accepted request captures its owner and language pair permanently.
    pub(crate) fn submit(&mut self, task: TranslationTask) -> Result<(), String> {
        let owner = task.owner();
        let TranslationInput::Text(text) = task.input else {
            return Err("A text translation task requires text input".into());
        };
        if text.trim().is_empty() {
            return Err("Text cannot be empty".into());
        }
        self.retire_finished();
        if self
            .tasks
            .iter()
            .filter(|task| !task.terminal || task.scope.accepts_events())
            .count()
            >= 32
        {
            return Err("Translation queue is full. Please try again.".into());
        }
        let publish_to_host_outputs = task
            .plugin
            .as_ref()
            .is_none_or(|binding| binding.publish_to_host_outputs());
        let scope = ChannelScope::text(owner, publish_to_host_outputs);
        scope
            .asr_only
            .store(task.languages.asr_only(), Ordering::Release);
        self.tasks.push(TextTask {
            scope,
            pending: Some(text),
            languages: task.languages,
            connection: TextConnection::default(),
            started: Instant::now(),
            segments: BTreeMap::new(),
            segment_count: 0,
            terminal: false,
        });
        self.reuse_connections();
        self.next_poll = Instant::now();
        Ok(())
    }

    pub(crate) fn update_prompts(&self, graphs: PromptGraphSet) {
        for task in &self.tasks {
            if let Some(session) = &task.connection.session {
                session.update_prompt_templates(graphs.clone());
            }
        }
    }

    pub(crate) fn cancel_owner(&mut self, owner: &TranslationSessionOwner) {
        self.completed.retain(|result| result.owner != *owner);
        self.tasks.retain(|task| {
            if task.scope.owner == *owner {
                task.cancel();
                false
            } else {
                true
            }
        });
        if self.tasks.is_empty() {
            self.backend_preparing = false;
        }
    }

    pub(crate) fn reset(&mut self) {
        self.completed.clear();
        for task in self.tasks.drain(..) {
            task.cancel();
        }
        self.backend_preparing = false;
    }

    pub(crate) fn poll(
        &mut self,
        backend: &mut BackendManager,
        server_url: &str,
        graphs: PromptGraphSet,
        ctx: Option<egui::Context>,
        target: &Sender<TaskEvent>,
    ) -> TextPoll {
        self.retire_finished();
        self.reuse_connections();
        let mut errors = Vec::new();
        let waiting: Vec<_> = (0..self.tasks.len())
            .filter(|&index| self.needs_connection(index))
            .collect();
        if !waiting.is_empty() && Instant::now() >= self.next_poll {
            self.next_poll = Instant::now() + Duration::from_millis(250);
            let state = if self.backend_preparing {
                match backend.status(server_url) {
                    BackendStatus::Ready => Ok(BackendStart::Ready),
                    BackendStatus::Starting(stage) => Ok(BackendStart::Starting(stage)),
                    BackendStatus::Failed(error) => Err(error),
                }
            } else {
                backend.prepare(server_url)
            };
            match state {
                Ok(BackendStart::Ready) => {
                    self.backend_preparing = false;
                    for index in waiting {
                        let task = &mut self.tasks[index];
                        let session = network::start_text_session(
                            task.connection.tx.clone(),
                            SessionConfig {
                                server_url: server_url.to_owned(),
                                languages: task.languages,
                                external_audio_gate: ExternalAudioGate::default(),
                                publish_to_host_outputs: task.scope.publish_to_host_outputs,
                                tts: None,
                                egui_ctx: ctx.clone(),
                                vad_threshold: 0.0,
                                vad_silence_ms: 0,
                                continuous_recognition: false,
                                audio_source: CaptureSource::Microphone,
                                finish_when_audio_ends: false,
                                prompt_graphs: Some(graphs.clone()),
                            },
                        );
                        task.scope
                            .stream_id
                            .store(session.stream_id(), Ordering::Release);
                        task.connection.session = Some(session);
                    }
                }
                Ok(BackendStart::Starting(_)) => self.backend_preparing = true,
                Err(error) => {
                    self.backend_preparing = false;
                    for index in waiting {
                        let task = &mut self.tasks[index];
                        task.fail(error.clone(), target, &mut errors);
                    }
                }
            }
        }
        for task in self.tasks.iter_mut().filter(|task| !task.terminal) {
            while let Ok(event) = task.connection.rx.try_recv() {
                match &event {
                    SessionEvent::Connected => {
                        task.emit(event, target);
                        if let Err(error) = task.send_pending() {
                            task.fail(error, target, &mut errors);
                        }
                    }
                    SessionEvent::Error(error) => {
                        task.fail(error.clone(), target, &mut errors);
                    }
                    SessionEvent::BackendError { message, .. } => {
                        let error = message.clone();
                        task.connection.close();
                        task.emit(event, target);
                        task.finish(error.clone(), target);
                        if task.scope.publish_to_host_outputs {
                            errors.push(error);
                        }
                    }
                    SessionEvent::Disconnected(reason) => {
                        task.fail(reason.clone(), target, &mut errors);
                    }
                    SessionEvent::Translation {
                        revisable,
                        segment_index,
                        segment_count,
                        translated,
                        ..
                    } => {
                        task.segment_count = task.segment_count.max((*segment_count).max(1));
                        if !revisable && *segment_index > 0 && *segment_index <= task.segment_count
                        {
                            task.segments.insert(*segment_index, translated.clone());
                        }
                        task.emit(event, target);
                        if task.segments.len() == task.segment_count as usize {
                            task.finish("Finished".into(), target);
                        }
                    }
                    _ => task.emit(event, target),
                }
                if task.terminal {
                    break;
                }
            }
            if !task.terminal && task.started.elapsed() > Duration::from_secs(180) {
                let error = if task.pending.is_some() {
                    "Text translation startup timed out"
                } else {
                    "Text translation timed out"
                };
                task.fail(error.into(), target, &mut errors);
            }
        }
        if self
            .tasks
            .iter()
            .any(|task| !task.terminal || task.scope.accepts_events())
            && let Some(ctx) = ctx
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        TextPoll {
            errors,
            completed: std::mem::take(&mut self.completed),
        }
    }
}

impl Drop for TextTranslation {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str, languages: LanguageSelection) -> TranslationTask {
        TranslationTask {
            languages,
            plugin: None,
            input: TranslationInput::Text(text.into()),
            profiles: Vec::new(),
        }
    }

    #[test]
    fn reset_detaches_late_events_before_a_new_request() {
        let mut text = TextTranslation::default();
        let languages = LanguageSelection::parse("en", "zh").unwrap();
        text.submit(request("old", languages)).unwrap();
        let old = text.tasks[0].connection.tx.clone();
        let old_scope = text.tasks[0].scope.clone();
        text.reset();
        assert!(old.send(SessionEvent::Connected).is_err());
        assert!(!old_scope.accepts_events());
        assert!(!text.busy());
        text.submit(request("new", languages)).unwrap();
        assert_eq!(text.tasks[0].pending.as_deref(), Some("new"));
        assert!(!Arc::ptr_eq(&text.tasks[0].scope, &old_scope));
    }

    #[test]
    fn multipart_text_result_stays_busy_until_all_one_based_segments_arrive() {
        let mut text = TextTranslation::default();
        text.submit(request(
            "words",
            LanguageSelection::parse("en", "zh").unwrap(),
        ))
        .unwrap();
        text.next_poll = Instant::now() + Duration::from_secs(60);
        let mut backend = BackendManager::load();
        let (target, events) = bounded(16);
        let make_segment = |segment_index| SessionEvent::Translation {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            continuous: false,
            publish_to_host_outputs: false,
            source: "words".into(),
            translated: format!("文字{segment_index}"),
            additional_translations: Vec::new(),
            asr_only: false,
            turn_id: "text-1".into(),
            segment_index,
            segment_count: 2,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 0.0,
            timing: Default::default(),
            boundary: Default::default(),
            term_matches: Vec::new(),
            prompt_trace: None,
            revisable: false,
            overlap_ratio: 0.0,
            authoritative_snapshot: false,
            revision: 0,
        };
        for index in [2, 2, 1] {
            text.tasks[0]
                .connection
                .tx
                .send(make_segment(index))
                .unwrap();
            let output = text.poll(
                &mut backend,
                "",
                PromptGraphSet {
                    graph: Default::default(),
                },
                Some(egui::Context::default()),
                &target,
            );
            assert!(output.completed.is_empty());
            assert_eq!(text.busy(), index != 1);
        }
        assert!(text.tasks[0].scope.accepts_events());
        let delivered = events.try_iter().collect::<Vec<_>>();
        assert_eq!(delivered.len(), 4);
        assert!(
            matches!(&delivered.last().unwrap().event, SessionEvent::Disconnected(reason) if reason == "Finished")
        );
        // The UI must wait for acceptance by the same pump that updates history.
        for event in delivered {
            let _ = event.scope.publish(&event.event, &[]);
        }
        text.retire_finished();
        let completed = text.completed.pop_front().unwrap();
        assert_eq!(completed.translated, "文字1文字2");
        assert!(completed.owner.is_host());
        text.retire_finished();
        assert!(text.completed.is_empty());
    }

    #[test]
    fn failed_and_cancelled_text_does_not_produce_completed_output() {
        for reason in ["Cancelled", "connection failed"] {
            let mut text = TextTranslation::default();
            text.submit(request(
                "input",
                LanguageSelection::parse("en", "zh").unwrap(),
            ))
            .unwrap();
            let task = &mut text.tasks[0];
            task.terminal = true;
            task.segments.insert(1, "partial result".into());
            if reason == "Cancelled" {
                task.scope.cancel(&[]);
            } else {
                let _ = task
                    .scope
                    .publish(&SessionEvent::Disconnected(reason.into()), &[]);
            }
            text.retire_finished();
            assert!(text.completed.is_empty());
        }
    }

    #[test]
    fn reset_discards_undelivered_completed_output() {
        let mut text = TextTranslation::default();
        text.completed.push_back(CompletedText {
            owner: TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
            translated: "old result".into(),
        });
        text.reset();
        assert!(text.completed.is_empty());
    }

    #[test]
    fn preparation_keeps_independent_requests_and_their_language_pairs() {
        use crate::session_coordinator::{
            PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy,
        };

        let mut controller = TextTranslation::default();
        let first = LanguageSelection::parse("en", "zh").unwrap();
        let second = LanguageSelection::parse("zh", "en").unwrap();
        let mut plugin_request = request("first", first);
        plugin_request.plugin = Some(PluginSessionBinding::text(
            PluginSessionOwner::new("example", "first", "", "", ""),
            SessionOutputPolicy::PluginOnly,
        ));
        controller.submit(plugin_request).unwrap();
        controller.submit(request("second", second)).unwrap();
        assert!(controller.owner_active("example"));
        assert!(controller.preparing_for("example"));
        assert!(!controller.preparing_for("other"));
        assert!(!controller.tasks[0].scope.publish_to_host_outputs);
        assert!(controller.tasks[1].scope.publish_to_host_outputs);
        assert_eq!(controller.tasks[0].pending.as_deref(), Some("first"));
        assert_eq!(controller.tasks[0].languages, first);
        assert_eq!(controller.tasks[1].pending.as_deref(), Some("second"));
        assert_eq!(controller.tasks[1].languages, second);
        for _ in 2..32 {
            controller.submit(request("queued", first)).unwrap();
        }
        assert!(controller.submit(request("overflow", first)).is_err());
        let canceled_scope = controller.tasks[0].scope.clone();
        controller.cancel_owner(&canceled_scope.owner);
        assert!(!canceled_scope.accepts_events());
        assert!(!controller.owner_active("example"));
        assert!(!controller.preparing_for("example"));
        assert_eq!(controller.tasks[0].pending.as_deref(), Some("second"));
        assert_eq!(controller.tasks.len(), 31);
        controller.reset();
        assert!(!controller.preparing());
    }

    #[test]
    fn requests_capture_additional_target_and_asr_mode_in_conversation_scope() {
        let mut controller = TextTranslation::default();
        let base = LanguageSelection::parse("en", "zh").unwrap();
        let extra = LanguageSelection::parse_with_options("en", "zh", Some("ja"), false).unwrap();
        let asr = LanguageSelection::parse_with_options("en", "zh", Some("ja"), true).unwrap();
        for languages in [base, extra, asr] {
            controller.submit(request("input", languages)).unwrap();
        }
        assert!(!controller.tasks[0].same_conversation(&controller.tasks[1]));
        assert!(!controller.tasks[1].same_conversation(&controller.tasks[2]));
        assert_eq!(
            controller.tasks[1]
                .languages
                .additional_target()
                .unwrap()
                .code(),
            "ja"
        );
        assert!(controller.tasks[2].scope.asr_only.load(Ordering::Acquire));
    }

    #[test]
    fn text_detection_preserves_additional_target_and_asr_does_not_translate() {
        let caps = LanguageCapabilities::default();
        let selected = select_languages_with_options(
            "How are you doing today?",
            "auto",
            "zh,en",
            caps,
            Some("ja"),
            false,
        )
        .unwrap();
        assert_eq!(selected.wire(), ("en".into(), "zh".into()));
        assert_eq!(selected.additional_target().unwrap().code(), "ja");
        let asr = select_languages_with_options(
            "How are you doing today?",
            "zh",
            "en",
            caps,
            Some("ja"),
            true,
        )
        .unwrap();
        assert!(asr.asr_only());
        assert_eq!(asr.wire(), ("zh".into(), "zh".into()));
        assert!(asr.additional_target().is_none());
    }
}
