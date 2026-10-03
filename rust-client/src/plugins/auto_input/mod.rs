//! Opt-in delivery of completed host translations to the focused text field.
//!
//! The worker starts on first enablement, sleeps without polling while disabled,
//! and releases the native focus capability then. Disabling or clearing drops
//! pending text; shutdown wakes and joins the worker. Results get one delivery
//! attempt and are never held for a future focus. Subscriber handles never
//! own worker lifetime and never call native input APIs.

use crate::{
    focused_input,
    session_coordinator::{HostOutputEvent, HostOutputSubscriber},
};
use parking_lot::{Condvar, Mutex};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_PENDING: usize = 64;
const MAX_PENDING_BYTES: usize = 256 * 1024;
const MAX_REMEMBERED: usize = 4096;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TranslationId {
    stream: u64,
    turn: String,
    segment: u32,
}

#[derive(Clone)]
struct PendingTranslation {
    id: TranslationId,
    text: Arc<str>,
    produced_at: u64,
}

#[derive(Default)]
struct State {
    enabled: bool,
    shutdown: bool,
    generation: u64,
    pending: VecDeque<PendingTranslation>,
    pending_bytes: usize,
    remembered: HashSet<TranslationId>,
    remembered_order: VecDeque<TranslationId>,
    error: Option<String>,
}

impl State {
    fn clear_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        while let Some(item) = self.pending.pop_front() {
            self.remember(item.id);
        }
        self.pending_bytes = 0;
    }

    fn remember(&mut self, id: TranslationId) {
        if !self.remembered.insert(id.clone()) {
            return;
        }
        if self.remembered_order.len() == MAX_REMEMBERED
            && let Some(oldest) = self.remembered_order.pop_front()
        {
            self.remembered.remove(&oldest);
        }
        self.remembered_order.push_back(id);
    }

    fn cancel_stream(&mut self, stream: u64) {
        let mut discarded = Vec::new();
        self.pending.retain(|item| {
            if item.id.stream == stream {
                discarded.push((item.id.clone(), item.text.len()));
                false
            } else {
                true
            }
        });
        for (id, bytes) in discarded {
            self.pending_bytes -= bytes;
            self.remember(id);
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.clear_pending();
            self.enabled = enabled;
            self.error = None;
        }
    }

    fn observe(&mut self, stream: u64, turn: &str, segment: u32, text: &str) {
        let produced_at = focused_input::timestamp();
        if self.shutdown || text.is_empty() {
            return;
        }
        let id = TranslationId {
            stream,
            turn: turn.to_owned(),
            segment,
        };
        if self.remembered.contains(&id) || self.pending.iter().any(|item| item.id == id) {
            return;
        }
        // Remember ignored and discarded results too: re-enabling must not
        // replay an older completed segment when a replacement batch arrives.
        if !self.enabled {
            self.remember(id);
            return;
        }
        if self.pending.len() == MAX_PENDING
            || text.len() > MAX_PENDING_BYTES.saturating_sub(self.pending_bytes)
        {
            self.remember(id);
            self.error =
                Some("Automatic input queue is full; a translation was skipped.".to_owned());
            return;
        }
        let text: Arc<str> = Arc::from(text);
        self.pending_bytes += text.len();
        self.pending.push_back(PendingTranslation {
            id,
            text,
            produced_at,
        });
    }

    fn is_current(&self, generation: u64, id: &TranslationId) -> bool {
        self.enabled
            && !self.shutdown
            && self.generation == generation
            && self.pending.front().is_some_and(|item| &item.id == id)
    }

    /// Every attempt consumes the result, including absent or changed focus.
    /// Refocusing must never replay an earlier translation.
    fn finish_attempt(
        &mut self,
        generation: u64,
        id: &TranslationId,
        result: Result<bool, String>,
    ) -> bool {
        if !self.is_current(generation, id) {
            return false;
        }
        if let Some(item) = self.pending.pop_front() {
            self.pending_bytes -= item.text.len();
            self.remember(item.id);
        }
        if let Err(error) = result {
            self.error = Some(error);
        }
        true
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Clone)]
pub(crate) struct AutoInputPublisher(Arc<Shared>);

impl HostOutputSubscriber for AutoInputPublisher {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        match event {
            HostOutputEvent::CommittedTranslation {
                stream_id,
                turn_id,
                segment_index,
                translated,
            } => {
                self.0
                    .state
                    .lock()
                    .observe(stream_id, turn_id, segment_index, translated);
                self.0.wake.notify_one();
            }
            HostOutputEvent::Clear => {
                self.0.state.lock().clear_pending();
                self.0.wake.notify_one();
            }
            HostOutputEvent::StreamCancelled(stream) => {
                self.0.state.lock().cancel_stream(stream);
                self.0.wake.notify_one();
            }
            // Captions may be source-only, previewed, or revised. Stream end
            // leaves only the brief handoff to the worker, never a focus backlog.
            HostOutputEvent::Caption { .. } | HostOutputEvent::StreamEnded(_) => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct AutoInputPlugin {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl AutoInputPlugin {
    pub(crate) fn publisher(&self) -> AutoInputPublisher {
        AutoInputPublisher(self.shared.clone())
    }

    pub(crate) fn enabled(&self) -> bool {
        self.shared.state.lock().enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.worker.is_none() {
            let shared = self.shared.clone();
            match thread::Builder::new()
                .name("translation-auto-input".to_owned())
                .spawn(move || run_worker(shared))
            {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.shared.state.lock().error =
                        Some(format!("Could not start automatic input: {error}"));
                    return;
                }
            }
        }
        self.shared.state.lock().set_enabled(enabled);
        self.shared.wake.notify_one();
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.shared.state.lock().error.take()
    }

    pub(crate) fn clear(&self) {
        self.shared.state.lock().clear_pending();
        self.shared.wake.notify_one();
    }
}

impl Drop for AutoInputPlugin {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock();
            state.shutdown = true;
            state.enabled = false;
            state.clear_pending();
        }
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker(shared: Arc<Shared>) {
    let mut platform = None;
    loop {
        let mut state = shared.state.lock();
        while !state.enabled && !state.shutdown {
            if platform.is_some() {
                // Native teardown must not hold the subscriber's queue lock.
                drop(state);
                platform = None;
                state = shared.state.lock();
                continue;
            }
            shared.wake.wait(&mut state);
        }
        if state.shutdown {
            return;
        }
        let generation = state.generation;
        if platform.is_none() {
            drop(state);
            // Observe focus from enablement, before accepting the first result.
            let initialized = focused_input::Platform::new();
            let mut state = shared.state.lock();
            if !state.enabled || state.shutdown || state.generation != generation {
                drop(state);
                drop(initialized);
                continue;
            }
            match initialized {
                Ok(native) => platform = Some(native),
                Err(error) => {
                    state.set_enabled(false);
                    state.error = Some(error);
                }
            }
            continue;
        }
        let native = platform.as_ref().expect("platform initialized above");
        let Some(item) = state.pending.front().cloned() else {
            // Service native focus notifications without polling editable UI.
            shared.wake.wait_for(&mut state, EVENT_POLL_INTERVAL);
            drop(state);
            native.poll_events();
            continue;
        };
        drop(state);
        let result =
            native
                .focused_target_since(item.produced_at)
                .and_then(|target| match target {
                    Some(target) => native.insert_if(&target, &item.text, || {
                        let state = shared.state.lock();
                        state.is_current(generation, &item.id).then_some(state)
                    }),
                    None => Ok(false),
                });
        shared
            .state
            .lock()
            .finish_attempt(generation, &item.id, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client_settings::CaptureSource, session_coordinator::CaptionUpdate};

    fn enabled_state() -> State {
        State {
            enabled: true,
            ..State::default()
        }
    }

    #[test]
    fn default_is_disabled_and_does_not_create_worker() {
        let plugin = AutoInputPlugin::default();
        assert!(!plugin.enabled());
        assert!(plugin.worker.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn idle_worker_supports_enable_disable_reenable_and_join() {
        let mut plugin = AutoInputPlugin::default();
        plugin.set_enabled(true);
        assert!(plugin.enabled());
        assert!(plugin.worker.is_some());
        plugin.set_enabled(false);
        assert!(!plugin.enabled());
        plugin.set_enabled(true);
        assert!(plugin.enabled());
        assert!(plugin.take_error().is_none());
        drop(plugin);
    }

    #[test]
    fn translations_are_transferred_verbatim_without_added_separators() {
        let mut state = enabled_state();
        let translations = ["  First\t sentence. \r\n", "Second sentence.🙂", " \n"];
        for (index, text) in translations.iter().enumerate() {
            state.observe(1, "turn", index as u32, text);
        }
        assert_eq!(state.pending.len(), translations.len());
        for (item, expected) in state.pending.iter().zip(translations) {
            assert_eq!(&*item.text, expected);
        }
        let text: String = state.pending.iter().map(|item| &*item.text).collect();
        assert_eq!(text, translations.concat());
        assert_eq!(state.pending_bytes, text.len());
    }

    #[test]
    fn consumes_each_stable_identity_once_even_if_republished_with_new_text() {
        let mut state = enabled_state();
        state.observe(1, "turn", 0, "first");
        let item = state.pending.front().unwrap().clone();
        assert!(state.finish_attempt(state.generation, &item.id, Ok(true)));
        state.observe(1, "turn", 0, "revised");
        assert!(state.pending.is_empty());
        state.observe(1, "turn", 1, "first");
        state.observe(2, "turn", 0, "first");
        assert_eq!(state.pending.len(), 2);
    }

    #[test]
    fn missing_or_changed_focus_discards_result_and_never_replays_on_refocus() {
        let mut state = enabled_state();
        state.observe(1, "turn", 0, "translation");
        let item = state.pending.front().unwrap().clone();
        assert!(state.finish_attempt(state.generation, &item.id, Ok(false)));
        assert!(state.pending.is_empty());
        assert_eq!(state.pending_bytes, 0);
        assert!(state.error.is_none());
        state.observe(1, "turn", 0, "translation");
        assert!(state.pending.is_empty());
        state.observe(1, "after-refocus", 0, "new translation");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(&*state.pending[0].text, "new translation");
    }

    #[test]
    fn uncertain_delivery_is_never_retried() {
        let mut state = enabled_state();
        state.observe(1, "turn", 0, "translation");
        let item = state.pending.front().unwrap().clone();
        assert!(state.finish_attempt(state.generation, &item.id, Err("partial delivery".into())));
        assert!(state.pending.is_empty());
        assert_eq!(state.pending_bytes, 0);
        assert_eq!(state.error.as_deref(), Some("partial delivery"));
        state.observe(1, "turn", 0, "translation");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn disable_and_reenable_discard_pending_and_never_replay_observed_results() {
        let mut state = enabled_state();
        state.observe(1, "before", 0, "pending");
        let generation = state.generation;
        let item = state.pending.front().unwrap().clone();
        state.set_enabled(false);
        state.observe(1, "disabled", 0, "ignored");
        state.set_enabled(true);
        assert!(!state.is_current(generation, &item.id));
        state.observe(1, "before", 0, "pending");
        state.observe(1, "disabled", 0, "ignored");
        state.observe(1, "new", 0, "new text");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(&*state.pending[0].text, "new text");
    }

    #[test]
    fn subscriber_ignores_caption_previews_and_hands_off_completed_results_once() {
        let plugin = AutoInputPlugin::default();
        plugin.shared.state.lock().set_enabled(true);
        let publisher = plugin.publisher();
        for update in [
            CaptionUpdate::Replace,
            CaptionUpdate::Append,
            CaptionUpdate::RollOver,
        ] {
            publisher.on_host_output(HostOutputEvent::Caption {
                stream_id: 1,
                audio_source: CaptureSource::Microphone,
                is_typing: false,
                source: "source",
                translated: "preview",
                speaker: "",
                update,
            });
        }
        assert!(plugin.shared.state.lock().pending.is_empty());
        publisher.on_host_output(HostOutputEvent::CommittedTranslation {
            stream_id: 1,
            turn_id: "turn",
            segment_index: 0,
            translated: "completed",
        });
        publisher.on_host_output(HostOutputEvent::StreamEnded(1));
        assert_eq!(plugin.shared.state.lock().pending.len(), 1);
        publisher.on_host_output(HostOutputEvent::Clear);
        let state = plugin.shared.state.lock();
        assert!(state.pending.is_empty());
        assert_eq!(state.pending_bytes, 0);
    }

    #[test]
    fn cancelling_one_stream_invalidates_its_work_and_preserves_other_streams() {
        let plugin = AutoInputPlugin::default();
        let publisher = plugin.publisher();
        let (generation, cancelled) = {
            let mut state = plugin.shared.state.lock();
            state.set_enabled(true);
            state.observe(1, "cancelled", 0, "discard");
            state.observe(2, "kept", 0, "keep");
            (state.generation, state.pending.front().unwrap().clone())
        };
        publisher.on_host_output(HostOutputEvent::StreamCancelled(1));
        let mut state = plugin.shared.state.lock();
        assert!(!state.is_current(generation, &cancelled.id));
        state.observe(1, "cancelled", 0, "stale replay");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending.front().unwrap().id.stream, 2);
        assert_eq!(state.pending_bytes, "keep".len());
    }

    #[test]
    fn cancelling_another_stream_does_not_repeat_an_in_flight_delivery() {
        let mut state = enabled_state();
        state.observe(1, "delivering", 0, "once");
        state.observe(2, "cancelled", 0, "discard");
        let generation = state.generation;
        let item = state.pending.front().unwrap().clone();
        state.cancel_stream(2);
        assert!(state.finish_attempt(generation, &item.id, Ok(true)));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn queue_bounds_preserve_accepted_order_and_report_overflow() {
        let mut state = enabled_state();
        for segment in 0..=MAX_PENDING as u32 {
            state.observe(1, "turn", segment, "text");
        }
        assert_eq!(state.pending.len(), MAX_PENDING);
        assert_eq!(state.pending.front().unwrap().id.segment, 0);
        assert_eq!(
            state.pending.back().unwrap().id.segment,
            MAX_PENDING as u32 - 1
        );
        assert!(state.error.is_some());
        state.clear_pending();
        state.observe(1, "oversized", 0, &"x".repeat(MAX_PENDING_BYTES + 1));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn identity_cache_is_bounded_without_forgetting_pending_results() {
        let mut state = enabled_state();
        state.observe(1, "waiting", 0, "waiting for focus");
        for segment in 0..(MAX_REMEMBERED + MAX_PENDING) as u32 {
            state.observe(2, "turn", segment, "text");
        }
        assert_eq!(state.remembered.len(), MAX_REMEMBERED);
        assert_eq!(state.remembered_order.len(), MAX_REMEMBERED);
        state.observe(1, "waiting", 0, "republished");
        assert_eq!(state.pending.len(), MAX_PENDING);
        assert_eq!(&*state.pending.front().unwrap().text, "waiting for focus");
        let item = state.pending.front().unwrap().clone();
        assert!(state.finish_attempt(state.generation, &item.id, Ok(true)));
        state.observe(1, "waiting", 0, "republished after delivery");
        assert_eq!(state.pending.len(), MAX_PENDING - 1);
    }

    #[test]
    fn shutdown_invalidates_pending_and_closes_retained_publishers() {
        let plugin = AutoInputPlugin::default();
        plugin.shared.state.lock().set_enabled(true);
        let publisher = plugin.publisher();
        publisher.0.state.lock().observe(1, "turn", 0, "text");
        drop(plugin);
        publisher.0.state.lock().observe(2, "turn", 0, "later");
        let state = publisher.0.state.lock();
        assert!(state.shutdown);
        assert!(!state.enabled);
        assert!(state.pending.is_empty());
    }
}
