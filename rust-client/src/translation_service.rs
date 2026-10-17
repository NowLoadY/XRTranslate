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
    /// Transport/input kind; output classification uses `is_typing`.
    pub is_text: bool,
    pub publish_to_host_outputs: bool,
    pub asr_only: AtomicBool,
    pub active: AtomicBool,
    pub finished: AtomicBool,
    pub failed: AtomicBool,
    pub ready: AtomicBool,
    pub stream_id: AtomicU64,
    results: Mutex<TranslationEventAdapter>,
}

impl ChannelScope {
    pub fn new(owner: TranslationSessionOwner) -> Arc<Self> {
        Self::audio(owner, true)
    }

    pub fn audio(owner: TranslationSessionOwner, publish_to_host_outputs: bool) -> Arc<Self> {
        Self::create(owner, false, publish_to_host_outputs)
    }

    pub fn text(owner: TranslationSessionOwner, publish_to_host_outputs: bool) -> Arc<Self> {
        Self::create(owner, true, publish_to_host_outputs)
    }

    pub fn text_audio_source(&self) -> Option<CaptureSource> {
        self.owner.plugin().and_then(|owner| owner.audio_source())
    }

    pub fn is_typing(&self) -> bool {
        self.is_text && self.text_audio_source().is_none()
    }

    fn create(
        owner: TranslationSessionOwner,
        is_text: bool,
        publish_to_host_outputs: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            owner,
            is_text,
            publish_to_host_outputs,
            asr_only: AtomicBool::new(false),
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

    /// Natural completion closes transport but lets completed TTS finish playing.
    pub fn retire(mut self) {
        self.invalidate();
        for channel in &mut self.channels {
            channel.session.disconnect();
            channel.audio_tx.take();
        }
        for worker in self.routers {
            let _ = worker.join();
        }
    }
}

/// Host audio and text lanes belong to one live conversation in this client.
/// A fresh process gets a fresh identity; plugins keep their own task scopes.
pub(crate) fn host_conversation_id() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("host-{}", uuid::Uuid::new_v4()))
        .clone()
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::session_coordinator::{AudioSourceFilter, PluginSessionOwner};

    #[test]
    fn text_inference_preserves_audio_origin_without_reclassifying_typed_input() {
        let owner = PluginSessionOwner::new("media", "sentence", "Media", "Media", "Media");
        let audio = ChannelScope::text(
            TranslationSessionOwner::Plugin(
                owner.clone().with_audio_source(CaptureSource::SystemAudio),
            ),
            true,
        );
        assert!(audio.is_text);
        assert!(!audio.is_typing());
        assert_eq!(audio.text_audio_source(), Some(CaptureSource::SystemAudio));
        let microphone_only = AudioSourceFilter {
            microphone: true,
            system_audio: false,
        };
        assert!(!microphone_only.allows(audio.text_audio_source().unwrap(), audio.is_typing()));
        for owner in [
            TranslationSessionOwner::Plugin(owner),
            TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
        ] {
            let typed = ChannelScope::text(owner, true);
            assert!(typed.is_typing());
            assert_eq!(typed.text_audio_source(), None);
            assert!(microphone_only.allows(CaptureSource::Microphone, typed.is_typing()));
        }
    }
}
