//! One admission queue owns per-model capacity and live concurrency changes.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use tokio::sync::oneshot;
use xrtranslate_protocol::InferenceWorkload;

const REALTIME_BURST_LIMIT: usize = 8;

#[derive(Clone)]
pub(crate) struct InferenceScheduler {
    inner: Arc<SchedulerInner>,
}

impl InferenceScheduler {
    pub(crate) fn new(asr_slots: usize, translation_slots: usize) -> Self {
        assert!(asr_slots > 0 && translation_slots > 0);
        Self {
            inner: Arc::new(SchedulerInner {
                state: Mutex::new(SchedulerState {
                    capacity: [asr_slots, translation_slots],
                    active: [0; 2],
                    parallel: true,
                    realtime_burst: 0,
                    realtime: VecDeque::new(),
                    offline: VecDeque::new(),
                }),
            }),
        }
    }

    pub(crate) fn set_parallel(&self, enabled: bool) {
        self.inner
            .state
            .lock()
            .expect("scheduler lock poisoned")
            .parallel = enabled;
        // Existing work finishes normally. Waiting requests use the new policy.
        self.inner.dispatch();
    }

    pub(crate) async fn acquire_asr(&self, workload: InferenceWorkload) -> InferencePermit {
        self.acquire(Role::Asr, workload).await
    }

    pub(crate) async fn acquire_translation(&self, workload: InferenceWorkload) -> InferencePermit {
        self.acquire(Role::Translation, workload).await
    }

    async fn acquire(&self, role: Role, workload: InferenceWorkload) -> InferencePermit {
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.inner.state.lock().expect("scheduler lock poisoned");
            let queue = match workload {
                InferenceWorkload::Realtime => &mut state.realtime,
                InferenceWorkload::Offline => &mut state.offline,
            };
            queue.push_back(Waiting { role, sender });
        }
        self.inner.dispatch();
        receiver
            .await
            .expect("inference scheduler closed while a permit was queued")
    }
}

#[derive(Clone, Copy)]
enum Role {
    Asr,
    Translation,
}

struct Waiting {
    role: Role,
    sender: oneshot::Sender<InferencePermit>,
}

struct SchedulerInner {
    state: Mutex<SchedulerState>,
}

struct SchedulerState {
    capacity: [usize; 2],
    active: [usize; 2],
    parallel: bool,
    realtime_burst: usize,
    realtime: VecDeque<Waiting>,
    offline: VecDeque<Waiting>,
}

impl SchedulerState {
    fn next(&mut self) -> Option<Waiting> {
        let can_start = |waiting: &Waiting| {
            self.active[waiting.role as usize] < self.capacity[waiting.role as usize]
                && (self.parallel || self.active == [0; 2])
        };
        // A busy model must not block the other model when running in parallel.
        let realtime = self.realtime.iter().position(can_start);
        let offline = self.offline.iter().position(can_start);
        let prefer_offline = offline.is_some()
            && (realtime.is_none() || self.realtime_burst >= REALTIME_BURST_LIMIT);
        let next = if prefer_offline {
            let index = offline?;
            self.realtime_burst = 0;
            self.offline.remove(index)
        } else {
            let index = realtime?;
            self.realtime_burst = self.realtime_burst.saturating_add(1);
            self.realtime.remove(index)
        }?;
        self.active[next.role as usize] += 1;
        Some(next)
    }
}

pub(crate) struct InferencePermit {
    scheduler: Option<Arc<SchedulerInner>>,
    role: Role,
}

impl Drop for InferencePermit {
    fn drop(&mut self) {
        if let Some(scheduler) = self.scheduler.take() {
            scheduler.finish(self.role);
            scheduler.dispatch();
        }
    }
}

impl SchedulerInner {
    fn finish(&self, role: Role) {
        self.state.lock().expect("scheduler lock poisoned").active[role as usize] -= 1;
    }

    fn dispatch(self: &Arc<Self>) {
        loop {
            let next = self.state.lock().expect("scheduler lock poisoned").next();
            let Some(Waiting { role, sender }) = next else {
                return;
            };
            let permit = InferencePermit {
                scheduler: Some(Arc::clone(self)),
                role,
            };
            if let Err(mut permit) = sender.send(permit) {
                // Return a cancelled waiter's slot without recursively dispatching.
                permit.scheduler = None;
                self.finish(role);
            }
        }
    }
}

#[cfg(test)]
mod tests;
