//! Ordered, non-blocking ingestion of host session events.
//!
//! The host event pump must stay responsive, so SQLite work is serialized on
//! this plugin-owned worker. Finish/fail commands share the same queue as
//! segment upserts, guaranteeing that terminal state is committed only after
//! all previously observed transcript events.

use super::{
    controller::{ActiveMeetingCapture, SharedMeetingCapture},
    store::{MeetingStore, NewSegment, SegmentSource},
    translation::MeetingTranslation,
};
use crossbeam_channel::{Sender, unbounded};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{
    CaptureSource,
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSegment,
        TranslationSessionOwner, TranslationTask,
    },
};

#[derive(Clone, Copy, Debug)]
pub enum MeetingSegmentSource {
    Microphone,
    SystemAudio,
}

#[derive(Clone, Debug)]
pub struct MeetingSegmentEvent {
    pub source: MeetingSegmentSource,
    pub turn_id: String,
    pub segment_index: u32,
    pub source_text: String,
    pub translated_text: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub raw_speaker_id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    pub is_final: bool,
}

enum Command {
    Begin(
        ActiveMeetingCapture,
        xrtranslate_engine::language::LanguageSelection,
    ),
    Translation(String, TranslationEvent),
    Paused(bool),
    CancelTranslation,
    Drain(ActiveMeetingCapture),
    Results(ActiveMeetingCapture, Vec<TranslationSegment>, bool),
    SealStream(u64),
    FinishActive(ActiveMeetingCapture),
    FailActive(ActiveMeetingCapture, String),
}

#[derive(Clone)]
pub struct MeetingEventSink {
    inner: Arc<MeetingEventSinkInner>,
}

struct MeetingEventSinkInner {
    next_translation: Arc<std::sync::Mutex<Option<TranslationTask>>>,
    translation_error: Arc<std::sync::Mutex<Option<String>>>,
    dispatch_enabled: AtomicBool,
    previews: std::sync::Mutex<crate::session_coordinator::LivePreviews>,
    tx: std::sync::Mutex<Option<Sender<Command>>>,
    active: SharedMeetingCapture,
    active_sessions: Arc<AtomicUsize>,
    finish_requested: Arc<AtomicBool>,
    worker: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for MeetingEventSinkInner {
    fn drop(&mut self) {
        if let Ok(mut tx) = self.tx.lock() {
            tx.take();
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(handle) = worker.take()
        {
            let _ = handle.join();
        }
    }
}

impl MeetingEventSink {
    pub fn start(store: Arc<MeetingStore>, active: SharedMeetingCapture) -> Self {
        let (tx, rx) = unbounded();
        let next_translation = Arc::new(std::sync::Mutex::new(None));
        let translation_error = Arc::new(std::sync::Mutex::new(None));
        let worker_next = Arc::clone(&next_translation);
        let worker_error = Arc::clone(&translation_error);
        let worker_active = Arc::clone(&active);
        let worker = std::thread::Builder::new()
            .name("meeting-event-store".into())
            .spawn(move || {
                let mut batches = HashMap::<(String, u64, String, bool), Vec<String>>::new();
                let mut translation: Option<MeetingTranslation> = None;
                loop {
                    let command = match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => Some(command),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    match command {
                        Some(Command::Begin(capture, languages)) => {
                            *worker_next.lock().unwrap() = None;
                            *worker_error.lock().unwrap() = None;
                            translation = (!languages.asr_only())
                                .then(|| MeetingTranslation::new(capture, languages));
                            *worker_next.lock().unwrap() = None;
                            *worker_error.lock().unwrap() = None;
                        }
                        Some(Command::Translation(operation, event)) => {
                            if let Some(job) = &mut translation {
                                if let Err(error) = job.accept(&store, &operation, &event) {
                                    *worker_error.lock().unwrap() = Some(error.clone());
                                    job.error = Some(error);
                                }
                            }
                        }
                        Some(Command::Paused(paused)) => {
                            if let Some(job) = &mut translation {
                                job.paused = paused;
                                job.changed();
                            }
                        }
                        Some(Command::CancelTranslation) => {
                            translation = None;
                            *worker_next.lock().unwrap() = None;
                        }
                        Some(Command::Drain(capture)) => {
                            if let Some(job) = translation.as_mut().filter(|job| {
                                job.capture.recognition_run_id == capture.recognition_run_id
                            }) {
                                job.closed = true;
                                job.changed();
                            } else {
                                finish_active(&store, &worker_active, &capture);
                            }
                        }
                        Some(Command::Results(capture, segments, replace)) => {
                            let Some(first) = segments.first() else {
                                continue;
                            };
                            let key = (
                                capture.recognition_run_id.clone(),
                                first.stream_id,
                                first.turn_id.clone(),
                                first.translated.is_some() && !first.asr_only,
                            );
                            let revisable = segments.iter().any(|segment| segment.revisable);
                            let events = segments
                                .iter()
                                .filter_map(MeetingSegmentEvent::from_translation)
                                .collect();
                            let previous = if replace {
                                batches.get(&key).cloned().unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            if let Some(keys) =
                                persist_segments(&store, &capture, events, &previous)
                            {
                                if replace && !revisable {
                                    // Final rows remain durable, but no longer belong to a
                                    // replaceable live batch.
                                    batches.remove(&key);
                                } else if replace || revisable {
                                    batches.insert(key, keys);
                                } else if let Some(batch) = batches.get_mut(&key) {
                                    batch.retain(|key| !keys.contains(key));
                                }
                            }
                            if let Some(job) = translation.as_mut().filter(|job| {
                                job.capture.recognition_run_id == capture.recognition_run_id
                            }) {
                                job.changed();
                            }
                        }
                        Some(Command::SealStream(stream)) => {
                            batches.retain(|(_, id, _, _), _| *id != stream);
                        }
                        Some(Command::FinishActive(capture)) => {
                            translation = None;
                            *worker_next.lock().unwrap() = None;
                            finish_active(&store, &worker_active, &capture);
                        }
                        Some(Command::FailActive(capture, error)) => {
                            translation = None;
                            *worker_next.lock().unwrap() = None;
                            fail_active(&store, &worker_active, &capture, error);
                        }
                        None => {}
                    }
                    if let Some(job) = &mut translation {
                        match job.next(&store, Instant::now()) {
                            Ok(Some(task)) => *worker_next.lock().unwrap() = Some(task),
                            Ok(None) => {}
                            Err(error) => {
                                *worker_error.lock().unwrap() = Some(error.clone());
                                job.error = Some(error);
                            }
                        }
                        if job.finished() {
                            if let Some(error) = &job.error {
                                fail_active(&store, &worker_active, &job.capture, error.clone());
                            } else {
                                finish_active(&store, &worker_active, &job.capture);
                            }
                            translation = None;
                        }
                    }
                }
            })
            .expect("failed to start meeting event store");
        Self {
            inner: Arc::new(MeetingEventSinkInner {
                next_translation,
                translation_error,
                dispatch_enabled: AtomicBool::new(false),
                previews: Default::default(),
                tx: std::sync::Mutex::new(Some(tx)),
                active,
                active_sessions: Arc::new(AtomicUsize::new(0)),
                finish_requested: Arc::new(AtomicBool::new(false)),
                worker: std::sync::Mutex::new(Some(worker)),
            }),
        }
    }

    fn capture_snapshot(&self) -> Option<ActiveMeetingCapture> {
        self.inner.active.lock().ok()?.clone()
    }

    pub(crate) fn owns_recognition(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_plugin(super::super::PluginId::MEETING.as_str())
            && self.capture_snapshot().is_some_and(|capture| {
                owner.operation_id() == Some(capture.recognition_run_id.as_str())
            })
    }

    pub(crate) fn begin_translation(
        &self,
        languages: xrtranslate_engine::language::LanguageSelection,
    ) {
        if let Some(capture) = self.capture_snapshot() {
            self.inner
                .dispatch_enabled
                .store(!languages.asr_only(), Ordering::Release);
            self.send(Command::Begin(capture, languages));
        }
    }

    pub(crate) fn next_translation_task(&self) -> Option<TranslationTask> {
        let task = self.inner.next_translation.lock().ok()?.take()?;
        let capture = self.capture_snapshot()?;
        (self.inner.dispatch_enabled.load(Ordering::Acquire)
            && task.owner().operation_id().is_some_and(|id| {
                id.starts_with(&format!("{}/translate/", capture.recognition_run_id))
            }))
        .then_some(task)
    }

    pub(crate) fn take_translation_error(&self) -> Option<String> {
        self.inner.translation_error.lock().ok()?.take()
    }

    pub(crate) fn set_translation_paused(&self, paused: bool) {
        self.send(Command::Paused(paused));
    }

    fn drain(&self) {
        if let Some(capture) = self.capture_snapshot() {
            self.send(Command::Drain(capture));
        }
    }

    pub fn previews(&self) -> crate::session_coordinator::LivePreviews {
        self.inner
            .previews
            .lock()
            .map(|previews| previews.clone())
            .unwrap_or_default()
    }

    pub fn finish_active(&self) {
        if let Some(capture) = self.capture_snapshot() {
            self.send(Command::FinishActive(capture));
        }
    }

    pub fn fail_active(&self, error: impl Into<String>) {
        if let Some(capture) = self.capture_snapshot() {
            self.send(Command::FailActive(capture, error.into()));
        }
    }

    pub fn active_is_imported(&self) -> bool {
        self.inner
            .active
            .lock()
            .ok()
            .and_then(|capture| capture.as_ref().map(|capture| capture.imported_audio))
            .unwrap_or(false)
    }

    /// Registers all recognition streams belonging to one meeting operation.
    pub fn begin_sessions(&self, count: usize) {
        if let Ok(mut previews) = self.inner.previews.lock() {
            *previews = Default::default();
        }
        self.inner.active_sessions.store(count, Ordering::Release);
        self.inner.finish_requested.store(false, Ordering::Release);
    }

    pub fn cancel_sessions(&self) {
        self.inner.dispatch_enabled.store(false, Ordering::Release);
        self.send(Command::CancelTranslation);
        if let Ok(mut previews) = self.inner.previews.lock() {
            *previews = Default::default();
        }
        self.inner.active_sessions.store(0, Ordering::Release);
        self.inner.finish_requested.store(false, Ordering::Release);
    }

    pub fn request_finish(&self) {
        self.inner.finish_requested.store(true, Ordering::Release);
        if self.inner.active_sessions.load(Ordering::Acquire) == 0 {
            self.drain();
        }
    }

    fn send(&self, command: Command) {
        if let Some(tx) = self.command_sender() {
            let _ = tx.send(command);
        }
    }

    fn command_sender(&self) -> Option<Sender<Command>> {
        self.inner.tx.lock().ok()?.as_ref().cloned()
    }
}

impl SessionEventSubscriber for MeetingEventSink {
    fn accepts_owner(&self, owner: &crate::session_coordinator::TranslationSessionOwner) -> bool {
        owner.is_plugin(super::super::PluginId::MEETING.as_str())
            && self.capture_snapshot().is_some_and(|capture| {
                owner.operation_id().is_some_and(|id| {
                    id == capture.recognition_run_id
                        || id.starts_with(&format!("{}/translate/", capture.recognition_run_id))
                })
            })
    }

    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if let Some(operation) = owner.operation_id()
            && self
                .capture_snapshot()
                .is_some_and(|capture| operation != capture.recognition_run_id)
        {
            // Text completions update their original row and never participate in
            // audio stream counting or create new transcript identities.
            self.send(Command::Translation(operation.to_owned(), event.clone()));
            return;
        }
        if let Ok(mut previews) = self.inner.previews.lock() {
            previews.observe(event);
        }
        if let TranslationEvent::Finished { stream_id, .. } = event {
            self.send(Command::SealStream(*stream_id));
        }
        match event {
            TranslationEvent::Preview(_) => {}
            TranslationEvent::Segment(segment) => {
                if let Some(capture) = self.capture_snapshot() {
                    self.send(Command::Results(capture, vec![segment.clone()], false));
                }
            }
            TranslationEvent::ReplaceSegments(segments) => {
                if let Some(capture) = self.capture_snapshot() {
                    self.send(Command::Results(capture, segments.clone(), true));
                }
            }
            TranslationEvent::StreamEnded { stream_id } => {
                self.send(Command::SealStream(*stream_id))
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Completed,
                ..
            } => {
                let previous = self
                    .inner
                    .active_sessions
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                        value.checked_sub(1)
                    })
                    .unwrap_or(0);
                if previous == 1
                    && (self.active_is_imported()
                        || self.inner.finish_requested.load(Ordering::Acquire))
                {
                    self.drain();
                    self.inner.finish_requested.store(false, Ordering::Release);
                }
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Failed(error),
                ..
            } => {
                self.fail_active(error.clone());
                self.cancel_sessions();
            }
            TranslationEvent::Finished {
                outcome: TranslationOutcome::Cancelled,
                ..
            } => {
                self.finish_active();
                self.cancel_sessions();
            }
        }
    }
}

impl MeetingSegmentEvent {
    fn from_translation(segment: &TranslationSegment) -> Option<Self> {
        if segment.source.is_empty() {
            return None;
        }
        Some(Self {
            source: match segment.audio_source {
                CaptureSource::Microphone => MeetingSegmentSource::Microphone,
                CaptureSource::SystemAudio => MeetingSegmentSource::SystemAudio,
                CaptureSource::Both => return None,
            },
            turn_id: segment.turn_id.clone(),
            segment_index: segment.segment_index,
            source_text: segment.source.clone(),
            translated_text: segment.translated.clone().filter(|_| !segment.asr_only),
            source_language: segment.source_lang.clone(),
            target_language: if segment.asr_only {
                String::new()
            } else {
                segment.target_lang.clone()
            },
            raw_speaker_id: segment.speaker_id.clone(),
            source_start_ms: segment.source_start_ms,
            source_end_ms: segment.source_end_ms,
            is_final: !segment.revisable,
        })
    }
}

fn stored_segment(capture: &ActiveMeetingCapture, event: &MeetingSegmentEvent) -> NewSegment {
    let (source, source_key) = if capture.imported_audio {
        (SegmentSource::ImportedAudio, "import")
    } else {
        match event.source {
            MeetingSegmentSource::Microphone => (SegmentSource::Microphone, "mic"),
            MeetingSegmentSource::SystemAudio => (SegmentSource::SystemAudio, "system"),
        }
    };
    let turn = if event.turn_id.is_empty() {
        format!("time-{}", event.source_start_ms.max(0.0).round() as i64)
    } else {
        event.turn_id.clone()
    };
    NewSegment {
        meeting_id: capture.meeting_id.clone(),
        external_key: format!(
            "{}:{source_key}:{turn}:{}",
            capture.recognition_run_id, event.segment_index
        ),
        topic_id: capture.topic_id.clone(),
        original_text: event.source_text.clone(),
        translated_text: event.translated_text.clone(),
        source_language: event.source_language.clone(),
        target_language: event.target_language.clone(),
        start_ms: capture.timeline_offset_ms + event.source_start_ms.max(0.0).round() as i64,
        end_ms: capture.timeline_offset_ms
            + event.source_end_ms.max(event.source_start_ms).round() as i64,
        source,
        recognition_run_id: capture.recognition_run_id.clone(),
        speaker_token: (!event.raw_speaker_id.trim().is_empty())
            .then(|| format!("{source_key}:{}", event.raw_speaker_id.trim())),
        is_final: event.is_final,
    }
}

fn persist_segments(
    store: &MeetingStore,
    capture: &ActiveMeetingCapture,
    events: Vec<MeetingSegmentEvent>,
    previous_keys: &[String],
) -> Option<Vec<String>> {
    let segments: Vec<_> = events
        .iter()
        .map(|event| stored_segment(capture, event))
        .collect();
    let keys = segments
        .iter()
        .map(|segment| segment.external_key.clone())
        .collect();
    let speakers: Vec<_> = segments
        .iter()
        .map(|segment| segment.speaker_token.clone())
        .collect();
    if let Err(error) = store.replace_segments(segments, previous_keys) {
        log::error!("Could not persist meeting segment: {error}");
        return None;
    }
    for (event, token) in events.iter().zip(speakers) {
        if let Some(token) = token {
            let suggested = speaker_label(&event.raw_speaker_id)
                .map(|label| format!("{label} · automatic"))
                .unwrap_or_else(|| "Automatic speaker".into());
            if let Err(error) = store.assign_speaker_token(
                &capture.meeting_id,
                &capture.recognition_run_id,
                &token,
                &suggested,
            ) {
                log::error!("Could not persist provisional speaker: {error}");
            }
        }
    }
    Some(keys)
}

fn finish_active(
    store: &MeetingStore,
    active: &SharedMeetingCapture,
    target: &ActiveMeetingCapture,
) {
    let Ok(mut capture) = active.lock() else {
        return;
    };
    if capture.as_ref().is_some_and(|current| {
        current.meeting_id == target.meeting_id
            && current.recognition_run_id != target.recognition_run_id
    }) {
        return;
    }
    if let Err(error) = store.end_meeting(&target.meeting_id) {
        log::error!("Could not finish meeting: {error}");
    }
    if capture.as_ref().is_some_and(|current| {
        current.recognition_run_id == target.recognition_run_id
            && current.meeting_id == target.meeting_id
    }) {
        *capture = None;
    }
}

fn fail_active(
    store: &MeetingStore,
    active: &SharedMeetingCapture,
    target: &ActiveMeetingCapture,
    error: String,
) {
    let Ok(mut capture) = active.lock() else {
        return;
    };
    if capture.as_ref().is_some_and(|current| {
        current.meeting_id == target.meeting_id
            && current.recognition_run_id != target.recognition_run_id
    }) {
        return;
    }
    if let Err(error) = store.fail_meeting(&target.meeting_id, error) {
        log::error!("Could not mark failed meeting: {error}");
    }
    if capture.as_ref().is_some_and(|current| {
        current.recognition_run_id == target.recognition_run_id
            && current.meeting_id == target.meeting_id
    }) {
        *capture = None;
    }
}

fn speaker_label(speaker_id: &str) -> Option<String> {
    crate::compact_speaker_label(speaker_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::meeting::store::{MarkerKind, NewMeeting};

    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(Instant::now() < deadline, "meeting worker did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn both_audio_streams_drain_before_the_last_translation_ends_the_meeting() {
        use crate::session_coordinator::PluginSessionOwner;
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "en", "zh"))
            .unwrap();
        store.start_meeting(&bundle.meeting.id).unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        };
        let active = Arc::new(std::sync::Mutex::new(Some(capture.clone())));
        let sink = MeetingEventSink::start(store.clone(), active.clone());
        sink.begin_translation(
            xrtranslate_engine::language::LanguageSelection::parse("en", "zh").unwrap(),
        );
        sink.begin_sessions(2);
        let owner = TranslationSessionOwner::Plugin(PluginSessionOwner::new(
            super::super::super::PluginId::MEETING.as_str(),
            "run",
            "Meeting Notes",
            "Open meeting controls",
            "Meeting",
        ));
        for (index, source) in [
            (1, CaptureSource::Microphone),
            (2, CaptureSource::SystemAudio),
        ] {
            let mut segment = recognized(index, 1);
            segment.audio_source = source;
            segment.stream_id = index as u64;
            segment.revisable = false;
            sink.on_translation_event(&owner, &TranslationEvent::Segment(segment));
        }
        sink.request_finish();
        sink.on_translation_event(
            &owner,
            &TranslationEvent::Finished {
                stream_id: 1,
                outcome: TranslationOutcome::Completed,
            },
        );
        assert!(active.lock().unwrap().is_some());
        sink.on_translation_event(
            &owner,
            &TranslationEvent::Finished {
                stream_id: 2,
                outcome: TranslationOutcome::Completed,
            },
        );
        for text in ["第一句。", "第二句。"] {
            let mut task = None;
            wait_for(|| {
                task = sink.next_translation_task();
                task.is_some()
            });
            let task = task.unwrap();
            assert!(active.lock().unwrap().is_some());
            assert!(sink.accepts_owner(&task.owner()));
            let mut result = recognized(1, 1);
            result.revisable = false;
            result.asr_only = false;
            result.translated = Some(text.into());
            result.target_lang = "zh".into();
            sink.on_translation_event(&task.owner(), &TranslationEvent::Segment(result));
            sink.on_translation_event(
                &task.owner(),
                &TranslationEvent::Finished {
                    stream_id: 90,
                    outcome: TranslationOutcome::Completed,
                },
            );
        }
        wait_for(|| active.lock().unwrap().is_none());
        let saved = store.open_meeting(&capture.meeting_id).unwrap();
        assert_eq!(
            saved.meeting.status,
            super::super::store::MeetingStatus::Ended
        );
        assert_eq!(saved.segments.len(), 2);
        assert!(saved.segments.iter().all(|s| s.translated_text.is_some()));
        assert!(!sink.accepts_owner(&owner));
    }

    #[test]
    fn cancellation_discards_a_late_translation_without_erasing_the_source() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "en", "zh"))
            .unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        };
        let active = Arc::new(std::sync::Mutex::new(Some(capture.clone())));
        let sink = MeetingEventSink::start(store.clone(), active);
        sink.begin_translation(
            xrtranslate_engine::language::LanguageSelection::parse("en", "zh").unwrap(),
        );
        sink.set_translation_paused(true);
        let mut segment = recognized(1, 1);
        segment.revisable = false;
        sink.send(Command::Results(
            capture.clone(),
            vec![segment.clone()],
            false,
        ));
        let mut task = None;
        wait_for(|| {
            task = sink.next_translation_task();
            task.is_some()
        });
        let task = task.unwrap();
        sink.cancel_sessions();
        segment.translated = Some("Discard me".into());
        segment.asr_only = false;
        segment.target_lang = "zh".into();
        sink.on_translation_event(&task.owner(), &TranslationEvent::Segment(segment));
        sink.on_translation_event(
            &task.owner(),
            &TranslationEvent::Finished {
                stream_id: 1,
                outcome: TranslationOutcome::Completed,
            },
        );
        drop(sink);
        let saved = store.list_segments(&capture.meeting_id).unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved[0].translated_text.is_none());
    }

    #[test]
    fn translation_failure_keeps_accepting_recognition_and_reports_failure_after_drain() {
        use crate::session_coordinator::PluginSessionOwner;
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "en", "zh"))
            .unwrap();
        store.start_meeting(&bundle.meeting.id).unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id,
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        };
        let active = Arc::new(std::sync::Mutex::new(Some(capture.clone())));
        let sink = MeetingEventSink::start(store.clone(), active.clone());
        sink.begin_translation(
            xrtranslate_engine::language::LanguageSelection::parse("en", "zh").unwrap(),
        );
        sink.begin_sessions(1);
        sink.set_translation_paused(true);
        let mut segment = recognized(1, 1);
        segment.revisable = false;
        sink.send(Command::Results(
            capture.clone(),
            vec![segment.clone()],
            false,
        ));
        let mut task = None;
        wait_for(|| {
            task = sink.next_translation_task();
            task.is_some()
        });
        sink.on_translation_event(
            &task.unwrap().owner(),
            &TranslationEvent::Finished {
                stream_id: 99,
                outcome: TranslationOutcome::Failed("Translation unavailable".into()),
            },
        );
        wait_for(|| sink.take_translation_error().is_some());
        assert!(active.lock().unwrap().is_some());
        segment.turn_id = "later-turn".into();
        segment.source = "Later source survives.".into();
        sink.send(Command::Results(capture.clone(), vec![segment], false));
        sink.request_finish();
        let owner = TranslationSessionOwner::Plugin(PluginSessionOwner::new(
            super::super::super::PluginId::MEETING.as_str(),
            "run",
            "Meeting Notes",
            "Open meeting controls",
            "Meeting",
        ));
        sink.on_translation_event(
            &owner,
            &TranslationEvent::Finished {
                stream_id: 1,
                outcome: TranslationOutcome::Completed,
            },
        );
        wait_for(|| active.lock().unwrap().is_none());
        let saved = store.open_meeting(&capture.meeting_id).unwrap();
        assert_eq!(saved.segments.len(), 2);
        assert_eq!(
            saved.meeting.status,
            super::super::store::MeetingStatus::Failed
        );
        assert_eq!(
            saved.meeting.failure_message.as_deref(),
            Some("Translation unavailable")
        );
    }

    fn recognized(index: u32, count: u32) -> TranslationSegment {
        TranslationSegment {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            turn_id: "turn-1".into(),
            segment_index: index,
            segment_count: count,
            source: format!("Sentence {index}."),
            translated: None,
            source_lang: "en".into(),
            target_lang: String::new(),
            additional_translations: Vec::new(),
            asr_only: true,
            speaker_id: String::new(),
            source_start_ms: f64::from(index - 1) * 1_000.0,
            source_end_ms: f64::from(index) * 1_000.0,
            timing: Default::default(),
            boundary: Default::default(),
            revisable: true,
            live: true,
        }
    }

    #[test]
    fn revisions_across_topics_preserve_identity_and_marked_evidence() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "auto", "zh"))
            .unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run-1".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        };
        let initial: Vec<_> = (1..=3).map(|index| recognized(index, 3)).collect();
        let stored: Vec<_> = initial
            .iter()
            .map(|segment| {
                store
                    .upsert_segment(stored_segment(
                        &capture,
                        &MeetingSegmentEvent::from_translation(segment).unwrap(),
                    ))
                    .unwrap()
            })
            .collect();
        let marker = store
            .add_marker(&stored[2].id, MarkerKind::Note, "Keep this evidence")
            .unwrap();
        let topic_b = store
            .create_topic(&capture.meeting_id, Some("Next topic"))
            .unwrap();
        let capture_b = ActiveMeetingCapture {
            topic_id: topic_b.id.clone(),
            ..capture.clone()
        };
        let sink = MeetingEventSink::start(
            Arc::clone(&store),
            Arc::new(std::sync::Mutex::new(Some(capture.clone()))),
        );
        // Replay the persisted preview to register its replaceable batch.
        sink.send(Command::Results(capture.clone(), initial, true));
        let mut revised = recognized(1, 1);
        revised.source = "Revised sentence.".into();
        revised.source_end_ms = 3_000.0;
        revised.revisable = false;
        sink.send(Command::Results(
            capture_b.clone(),
            vec![revised.clone()],
            true,
        ));
        let mut translated = revised.clone();
        translated.translated = Some("修订后的句子。".into());
        translated.target_lang = "zh".into();
        translated.asr_only = false;
        sink.send(Command::Results(capture_b.clone(), vec![translated], false));
        let mut next = revised;
        next.turn_id = "turn-2".into();
        next.source = "Next sentence.".into();
        next.source_start_ms = 4_000.0;
        next.source_end_ms = 5_000.0;
        sink.send(Command::Results(capture_b, vec![next], false));
        // Dropping the last sink closes its queue and joins the ordered worker.
        drop(sink);

        let saved = store.open_meeting(&capture.meeting_id).unwrap();
        assert_eq!(saved.segments.len(), 3);
        let revised = saved
            .segments
            .iter()
            .find(|segment| segment.id == stored[0].id)
            .unwrap();
        assert_eq!(revised.topic_id, capture.topic_id);
        assert_eq!(revised.sequence, stored[0].sequence);
        assert_eq!(revised.original_text, "Revised sentence.");
        assert_eq!(revised.translated_text.as_deref(), Some("修订后的句子。"));
        assert_eq!(revised.source_language, "en");
        assert_eq!(revised.target_language, "zh");
        assert!(revised.is_final);
        assert!(
            saved
                .segments
                .iter()
                .all(|segment| segment.id != stored[1].id)
        );
        let evidence = saved
            .segments
            .iter()
            .find(|segment| segment.id == stored[2].id)
            .unwrap();
        assert_eq!(evidence.topic_id, capture.topic_id);
        assert_eq!(evidence.original_text, stored[2].original_text);
        assert_eq!(saved.markers, vec![marker]);
        let next = saved
            .segments
            .iter()
            .find(|segment| segment.external_key.ends_with(":turn-2:1"))
            .unwrap();
        assert_eq!(next.topic_id, topic_b.id);
        assert_eq!(next.original_text, "Next sentence.");
    }

    #[test]
    fn asr_only_results_replace_the_source_batch_without_storing_a_translation() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live("Meeting", None, "auto", "zh"))
            .unwrap();
        let capture = ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run-1".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        };
        let sink = MeetingEventSink::start(
            Arc::clone(&store),
            Arc::new(std::sync::Mutex::new(Some(capture.clone()))),
        );
        sink.send(Command::Results(
            capture.clone(),
            vec![recognized(1, 2), recognized(2, 2)],
            true,
        ));
        let mut finalized = recognized(1, 1);
        finalized.translated = Some(finalized.source.clone());
        finalized.target_lang = finalized.source_lang.clone();
        finalized.revisable = false;
        sink.send(Command::Results(capture.clone(), vec![finalized], true));
        drop(sink);

        let segments = store.list_segments(&capture.meeting_id).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].original_text, "Sentence 1.");
        assert_eq!(segments[0].source_language, "en");
        assert!(segments[0].translated_text.is_none());
        assert!(segments[0].target_language.is_empty());
        assert!(segments[0].is_final);
    }
}
