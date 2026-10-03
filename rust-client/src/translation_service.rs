//! Running translation tasks own their channels; service intent is independent.
use crate::{
    CaptureSource, RecognitionSettings, SystemAudioInputSelection,
    network::{SessionEvent, SessionHandle},
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationEventAdapter, TranslationOutcome,
        TranslationSessionOwner,
    },
};
use crossbeam_channel::{Sender, bounded};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use xrtranslate_engine::language::LanguageSelection;

pub(crate) struct ChannelScope {
    pub owner: TranslationSessionOwner,
    pub is_text: bool,
    pub active: AtomicBool,
    pub finished: AtomicBool,
    pub failed: AtomicBool,
    pub ready: AtomicBool,
    pub stream_id: AtomicU64,
    results: Mutex<TranslationEventAdapter>,
}

impl ChannelScope {
    pub fn new(owner: TranslationSessionOwner) -> Arc<Self> {
        Self::create(owner, false)
    }

    pub fn text(owner: TranslationSessionOwner) -> Arc<Self> {
        Self::create(owner, true)
    }

    fn create(owner: TranslationSessionOwner, is_text: bool) -> Arc<Self> {
        Arc::new(Self {
            owner,
            is_text,
            active: AtomicBool::new(true),
            finished: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            stream_id: AtomicU64::new(0),
            results: Mutex::new(TranslationEventAdapter::default()),
        })
    }
    /// Capture groups use the existing network session number.
    pub fn id(&self) -> u64 {
        self.stream_id.load(Ordering::Acquire)
    }

    pub fn accepts_events(&self) -> bool {
        self.active.load(Ordering::Acquire) && !self.finished.load(Ordering::Acquire)
    }

    pub fn publish(
        &self,
        event: &SessionEvent,
        subscribers: &[Box<dyn SessionEventSubscriber>],
    ) -> Vec<TranslationEvent> {
        let events = match self.results.lock() {
            Ok(mut results) => results.push(event, self.id()),
            Err(_) => return Vec::new(),
        };
        for event in &events {
            publish_result(&self.owner, event, subscribers);
            if let TranslationEvent::Finished { outcome, .. } = event {
                self.failed.store(
                    matches!(outcome, TranslationOutcome::Failed(_)),
                    Ordering::Release,
                );
                self.finished.store(true, Ordering::Release);
            }
        }
        events
    }

    pub fn cancel(&self, subscribers: &[Box<dyn SessionEventSubscriber>]) {
        if self.accepts_events() {
            let _ = self.publish(&SessionEvent::Disconnected("Cancelled".into()), subscribers);
        }
        self.active.store(false, Ordering::Release);
    }
}

pub(crate) fn publish_result(
    owner: &TranslationSessionOwner,
    event: &TranslationEvent,
    subscribers: &[Box<dyn SessionEventSubscriber>],
) {
    for subscriber in subscribers
        .iter()
        .filter(|subscriber| subscriber.accepts_owner(owner))
    {
        subscriber.on_translation_event(owner, event);
    }
}

pub(crate) struct TaskEvent {
    pub scope: Arc<ChannelScope>,
    pub event: SessionEvent,
}

/// One bounded adapter per recognition channel, including lifecycle events
/// without a stream ID. Cancellation invalidates queued and late results.
pub(crate) fn scoped_events(
    scope: Arc<ChannelScope>,
    target: Sender<TaskEvent>,
) -> Sender<SessionEvent> {
    let (tx, rx) = bounded(128);
    std::thread::Builder::new()
        .name("translation-events".into())
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                if scope.accepts_events()
                    && target
                        .send(TaskEvent {
                            scope: scope.clone(),
                            event,
                        })
                        .is_err()
                {
                    break;
                }
            }
        })
        .expect("failed to start translation event adapter");
    tx
}

pub(crate) struct TaskChannel {
    pub source: CaptureSource,
    pub scope: Arc<ChannelScope>,
    pub session: SessionHandle,
    /// None for finite input, whose producer owns the only audio sender.
    pub audio_tx: Option<Sender<Vec<f32>>>,
    pub recognition: RecognitionSettings,
    pub microphone_device_id: String,
    pub system_audio_input: SystemAudioInputSelection,
    pub capturing: bool,
}

pub(crate) struct AudioTask {
    pub owner: TranslationSessionOwner,
    pub languages: LanguageSelection,
    pub channels: Vec<TaskChannel>,
    pub paused: bool,
    pub routers: Vec<std::thread::JoinHandle<()>>,
}

impl AudioTask {
    pub fn is_live(&self) -> bool {
        self.channels
            .iter()
            .any(|channel| channel.audio_tx.is_some())
    }
    pub fn done(&self) -> bool {
        self.channels
            .iter()
            .all(|channel| channel.scope.finished.load(Ordering::Acquire))
    }
    pub fn invalidate(&self) {
        for channel in &self.channels {
            channel.scope.active.store(false, Ordering::Release);
        }
    }
    pub fn cancel(mut self) {
        self.invalidate();
        for channel in &mut self.channels {
            channel.session.cancel();
            channel.audio_tx.take();
        }
        for worker in self.routers {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_adapter_drops_canceled_input_and_does_not_block_another_task() {
        let a = ChannelScope::new(TranslationSessionOwner::Host {
            capture_source: CaptureSource::Microphone,
        });
        let (target, output) = bounded(4);
        let tx = scoped_events(a.clone(), target);
        a.active.store(false, Ordering::Release);
        tx.send(SessionEvent::Connected).unwrap();
        drop(tx);
        assert!(
            output
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_err()
        );
        let b = ChannelScope::new(TranslationSessionOwner::Host {
            capture_source: CaptureSource::SystemAudio,
        });
        let (target, output) = bounded(4);
        let tx = scoped_events(b.clone(), target);
        tx.send(SessionEvent::Connected).unwrap();
        let received = output
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(Arc::ptr_eq(&b, &received.scope));
    }

    #[test]
    fn cancellation_rejects_late_events_without_affecting_another_task() {
        let a = ChannelScope::new(TranslationSessionOwner::Host {
            capture_source: CaptureSource::Microphone,
        });
        let b = ChannelScope::new(TranslationSessionOwner::Host {
            capture_source: CaptureSource::SystemAudio,
        });

        a.active.store(false, Ordering::Release);
        assert!(!a.accepts_events());
        assert!(b.accepts_events());
        b.finished.store(true, Ordering::Release);
        assert!(!b.accepts_events());
    }
}
