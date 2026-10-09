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
