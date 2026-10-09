//! Ordered, non-blocking ingestion of host session events.
//!
//! The host event pump must stay responsive, so SQLite work is serialized on
//! this plugin-owned worker. Finish/fail commands share the same queue as
//! segment upserts, guaranteeing that terminal state is committed only after
//! all previously observed transcript events.

use super::{
    controller::{ActiveMeetingCapture, SharedMeetingCapture},
    store::{MeetingStore, NewSegment, SegmentSource},
};
use crossbeam_channel::{Sender, unbounded};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use crate::{
    CaptureSource,
    session_coordinator::{
        SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSegment,
        TranslationSessionOwner,
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
    pub raw_speaker_id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    pub is_final: bool,
}

enum Command {
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
        let worker_active = Arc::clone(&active);
        let worker = std::thread::Builder::new()
            .name("meeting-event-store".into())
            .spawn(move || {
                let mut batches =
                    HashMap::<(String, String, u64, String, bool), Vec<String>>::new();
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Results(capture, segments, replace) => {
                            let Some(first) = segments.first() else {
                                continue;
                            };
                            let key = (
                                capture.recognition_run_id.clone(),
                                capture.topic_id.clone(),
                                first.stream_id,
                                first.turn_id.clone(),
                                first.translated.is_some(),
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
                        }
                        Command::SealStream(stream) => {
                            batches.retain(|(_, _, id, _, _), _| *id != stream);
                        }
                        Command::FinishActive(capture) => {
                            finish_active(&store, &worker_active, &capture)
                        }
                        Command::FailActive(capture, error) => {
                            fail_active(&store, &worker_active, &capture, error)
                        }
                    }
                }
            })
            .expect("failed to start meeting event store");
        Self {
            inner: Arc::new(MeetingEventSinkInner {
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
        self.inner.active_sessions.store(count, Ordering::Release);
        self.inner.finish_requested.store(false, Ordering::Release);
    }

    pub fn cancel_sessions(&self) {
        self.inner.active_sessions.store(0, Ordering::Release);
        self.inner.finish_requested.store(false, Ordering::Release);
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
            && self
                .capture_snapshot()
                .is_some_and(|capture| owner.operation_id() == Some(capture.meeting_id.as_str()))
    }

    fn on_translation_event(&self, _owner: &TranslationSessionOwner, event: &TranslationEvent) {
        if let TranslationEvent::Finished { stream_id, .. } = event {
            self.send(Command::SealStream(*stream_id));
        }
        match event {
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
                    self.finish_active();
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
            translated_text: segment.translated.clone(),
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
    let speaker_id = speaker_id.trim();
    if speaker_id.is_empty() {
        return None;
    }
    let numeric = speaker_id
        .rsplit(['-', '_'])
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    Some(numeric.map_or_else(|| "S?".into(), |number| format!("S{number}")))
}
