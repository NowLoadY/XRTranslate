use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use tokio::sync::oneshot;
use xrtranslate_protocol::InferenceWorkload;

const REALTIME_BURST_LIMIT: usize = 8;

#[derive(Clone)]
pub(crate) struct InferenceScheduler {
    asr: PriorityLimiter,
    translation: PriorityLimiter,
}

impl InferenceScheduler {
    pub(crate) fn new(asr_slots: usize, translation_slots: usize) -> Self {
        Self {
            asr: PriorityLimiter::new(asr_slots),
            translation: PriorityLimiter::new(translation_slots),
        }
    }

    pub(crate) async fn acquire_asr(&self, workload: InferenceWorkload) -> InferencePermit {
        self.asr.acquire(workload).await
    }

    pub(crate) async fn acquire_translation(&self, workload: InferenceWorkload) -> InferencePermit {
        self.translation.acquire(workload).await
    }
}

#[derive(Clone)]
struct PriorityLimiter {
    inner: Arc<LimiterInner>,
}

struct LimiterInner {
    state: Mutex<LimiterState>,
}

struct LimiterState {
    available: usize,
    realtime_burst: usize,
    realtime: VecDeque<oneshot::Sender<()>>,
    offline: VecDeque<oneshot::Sender<()>>,
}

impl PriorityLimiter {
    fn new(slots: usize) -> Self {
        assert!(slots > 0, "inference scheduler requires at least one slot");
        Self {
            inner: Arc::new(LimiterInner {
                state: Mutex::new(LimiterState {
                    available: slots,
                    realtime_burst: 0,
                    realtime: VecDeque::new(),
                    offline: VecDeque::new(),
                }),
            }),
        }
    }

    async fn acquire(&self, workload: InferenceWorkload) -> InferencePermit {
        let receiver = {
            let mut state = self.inner.state.lock().expect("scheduler lock poisoned");
            if state.available > 0 && state.realtime.is_empty() {
                state.available -= 1;
                None
            } else {
                let (sender, receiver) = oneshot::channel();
                match workload {
                    InferenceWorkload::Realtime => state.realtime.push_back(sender),
                    InferenceWorkload::Offline => state.offline.push_back(sender),
                }
                Some(receiver)
            }
        };
        if let Some(receiver) = receiver {
            receiver
                .await
                .expect("inference scheduler closed while a permit was queued");
        }
        InferencePermit {
            limiter: Arc::clone(&self.inner),
        }
    }
}

pub(crate) struct InferencePermit {
    limiter: Arc<LimiterInner>,
}

impl Drop for InferencePermit {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

impl LimiterInner {
    fn release(&self) {
        let mut state = self.state.lock().expect("scheduler lock poisoned");
        loop {
            let prefer_offline = !state.offline.is_empty()
                && (state.realtime.is_empty() || state.realtime_burst >= REALTIME_BURST_LIMIT);
            let next = if prefer_offline {
                state.realtime_burst = 0;
                state.offline.pop_front()
            } else if let Some(sender) = state.realtime.pop_front() {
                state.realtime_burst += 1;
                Some(sender)
            } else {
                state.realtime_burst = 0;
                state.offline.pop_front()
            };
            let Some(next) = next else {
                state.available += 1;
                return;
            };
            if next.send(()).is_ok() {
                return;
            }
        }
    }
}
