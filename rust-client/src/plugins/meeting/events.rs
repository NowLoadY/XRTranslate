//! Ordered, non-blocking ingestion of host session events.
//!
//! The host event pump must stay responsive, so SQLite work is serialized on
//! this plugin-owned worker. Finish/fail commands share the same queue as
//! segment upserts, guaranteeing that terminal state is committed only after
//! all previously observed transcript events.

use super::{
    controller::SharedMeetingCapture,
    store::{MeetingStore, NewSegment, SegmentSource},
};
use crossbeam_channel::{Sender, unbounded};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use crate::{CaptureSource, network::SessionEvent, session_coordinator::SessionEventSubscriber};

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
    Segment(MeetingSegmentEvent),
    FinishActive,
    FailActive(String),
    #[cfg(test)]
    Flush(Sender<()>),
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
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Segment(event) => persist_segment(&store, &worker_active, event),
                        Command::FinishActive => finish_active(&store, &worker_active),
                        Command::FailActive(error) => fail_active(&store, &worker_active, error),
                        #[cfg(test)]
                        Command::Flush(done) => {
                            let _ = done.send(());
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

    pub fn persist(&self, event: MeetingSegmentEvent) {
        self.send(Command::Segment(event));
    }

    pub fn finish_active(&self) {
        self.send(Command::FinishActive);
    }

    pub fn fail_active(&self, error: impl Into<String>) {
        self.send(Command::FailActive(error.into()));
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

    /// Requests durable completion once every stream has drained.
    pub fn request_finish(&self) {
        self.inner.finish_requested.store(true, Ordering::Release);
    }

    pub fn cancel_sessions(&self) {
        self.inner.active_sessions.store(0, Ordering::Release);
        self.inner.finish_requested.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn flush(&self) {
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        self.command_sender()
            .expect("meeting event sink is open")
            .send(Command::Flush(done_tx))
            .unwrap();
        done_rx.recv().unwrap();
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
    fn on_session_event(&self, event: &SessionEvent) {
        match event {
            SessionEvent::SourceSegment {
                audio_source,
                text,
                turn_id,
                speaker_id,
                source_start_ms,
                source_end_ms,
                segment_index,
                revisable,
                ..
            } if !text.is_empty() => self.persist_segment(
                *audio_source,
                turn_id,
                *segment_index,
                text,
                None,
                speaker_id,
                *source_start_ms,
                *source_end_ms,
                !revisable,
            ),
            SessionEvent::Translation {
                audio_source,
                source,
                translated,
                turn_id,
                speaker_id,
                source_start_ms,
                source_end_ms,
                segment_index,
                revisable,
                ..
            } => self.persist_segment(
                *audio_source,
                turn_id,
                *segment_index,
                source,
                Some(translated),
                speaker_id,
                *source_start_ms,
                *source_end_ms,
                !revisable,
            ),
            SessionEvent::Disconnected(reason) if reason == "Finished" => {
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
            SessionEvent::Error(error) => {
                self.fail_active(error.clone());
                self.cancel_sessions();
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
impl MeetingEventSink {
    fn persist_segment(
        &self,
        audio_source: CaptureSource,
        turn_id: &str,
        segment_index: u32,
        source_text: &str,
        translated_text: Option<&str>,
        raw_speaker_id: &str,
        source_start_ms: f64,
        source_end_ms: f64,
        is_final: bool,
    ) {
        let source = match audio_source {
            CaptureSource::Microphone => MeetingSegmentSource::Microphone,
            CaptureSource::SystemAudio => MeetingSegmentSource::SystemAudio,
            CaptureSource::Both => return,
        };
        self.persist(MeetingSegmentEvent {
            source,
            turn_id: turn_id.to_owned(),
            segment_index,
            source_text: source_text.to_owned(),
            translated_text: translated_text.map(ToOwned::to_owned),
            raw_speaker_id: raw_speaker_id.to_owned(),
            source_start_ms,
            source_end_ms,
            is_final,
        });
    }
}

fn persist_segment(
    store: &MeetingStore,
    active: &SharedMeetingCapture,
    event: MeetingSegmentEvent,
) {
    let capture = active.lock().ok().and_then(|capture| capture.clone());
    let Some(capture) = capture else {
        return;
    };
    let source = if capture.imported_audio {
        SegmentSource::ImportedAudio
    } else {
        match event.source {
            MeetingSegmentSource::Microphone => SegmentSource::Microphone,
            MeetingSegmentSource::SystemAudio => SegmentSource::SystemAudio,
        }
    };
    let source_key = if capture.imported_audio {
        "import"
    } else {
        match event.source {
            MeetingSegmentSource::Microphone => "mic",
            MeetingSegmentSource::SystemAudio => "system",
        }
    };
    let speaker_token = (!event.raw_speaker_id.trim().is_empty())
        .then(|| format!("{source_key}:{}", event.raw_speaker_id.trim()));
    let external_key = format!(
        "{}:{source_key}:{}:{}:{}",
        capture.recognition_run_id,
        if event.turn_id.is_empty() {
            "turn"
        } else {
            &event.turn_id
        },
        event.source_start_ms.max(0.0).round() as i64,
        event.segment_index,
    );
    let segment = NewSegment {
        meeting_id: capture.meeting_id.clone(),
        external_key,
        topic_id: capture.topic_id.clone(),
        original_text: event.source_text,
        translated_text: event.translated_text,
        start_ms: capture.timeline_offset_ms + event.source_start_ms.max(0.0).round() as i64,
        end_ms: capture.timeline_offset_ms
            + event.source_end_ms.max(event.source_start_ms).round() as i64,
        source,
        recognition_run_id: capture.recognition_run_id.clone(),
        speaker_token: speaker_token.clone(),
        is_final: event.is_final,
    };
    if let Err(error) = store.upsert_segment(segment) {
        log::error!("Could not persist meeting segment: {error}");
        return;
    }
    if let Some(token) = speaker_token {
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

fn finish_active(store: &MeetingStore, active: &SharedMeetingCapture) {
    let Ok(mut capture) = active.lock() else {
        return;
    };
    if let Some(current) = capture.as_ref()
        && let Err(error) = store.end_meeting(&current.meeting_id)
    {
        log::error!("Could not finish meeting: {error}");
    }
    *capture = None;
}

fn fail_active(store: &MeetingStore, active: &SharedMeetingCapture, error: String) {
    let Ok(mut capture) = active.lock() else {
        return;
    };
    if let Some(current) = capture.as_ref()
        && let Err(store_error) = store.fail_meeting(&current.meeting_id, error)
    {
        log::error!("Could not mark failed meeting: {store_error}");
    }
    *capture = None;
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

#[cfg(test)]
mod tests {
    use super::super::controller::ActiveMeetingCapture;
    use super::super::store::{MeetingStatus, NewMeeting};
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn speaker_labels_stay_compact() {
        assert_eq!(speaker_label("speaker-02").as_deref(), Some("S2"));
        assert_eq!(speaker_label("unknown").as_deref(), Some("S?"));
        assert_eq!(speaker_label(""), None);
    }

    #[test]
    fn finish_is_ordered_after_queued_segments() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live(
                "Ordered",
                Some("default".into()),
                "en",
                "zh",
            ))
            .unwrap();
        store.start_meeting(&bundle.meeting.id).unwrap();
        let active = Arc::new(Mutex::new(Some(ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run-1".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        })));
        let sink = MeetingEventSink::start(Arc::clone(&store), Arc::clone(&active));
        sink.persist(MeetingSegmentEvent {
            source: MeetingSegmentSource::Microphone,
            turn_id: "turn-1".into(),
            segment_index: 1,
            source_text: "hello".into(),
            translated_text: Some("你好".into()),
            raw_speaker_id: "speaker-01".into(),
            source_start_ms: 0.0,
            source_end_ms: 500.0,
            is_final: true,
        });
        sink.finish_active();
        sink.flush();

        assert!(active.lock().unwrap().is_none());
        assert_eq!(
            store.get_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Ended
        );
        assert_eq!(
            store
                .open_meeting(&bundle.meeting.id)
                .unwrap()
                .segments
                .len(),
            1
        );
    }

    #[test]
    fn generic_session_events_are_adapted_inside_the_plugin() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live(
                "Subscriber",
                Some("default".into()),
                "en",
                "zh",
            ))
            .unwrap();
        store.start_meeting(&bundle.meeting.id).unwrap();
        let active = Arc::new(Mutex::new(Some(ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run-generic".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        })));
        let sink = MeetingEventSink::start(Arc::clone(&store), active);

        sink.on_session_event(&SessionEvent::Translation {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            continuous: false,
            publish_to_host_outputs: false,
            source: "hello".into(),
            translated: "你好".into(),
            turn_id: "turn-generic".into(),
            segment_index: 1,
            segment_count: 1,
            speaker_id: "speaker-03".into(),
            source_start_ms: 100.0,
            source_end_ms: 500.0,
            timing: xrtranslate_protocol::SegmentTiming::UtteranceWindow,
            boundary: xrtranslate_protocol::SegmentBoundary::Silence,
            term_matches: Vec::new(),
            prompt_trace: None,
            revisable: false,
            overlap_ratio: 0.0,
            authoritative_snapshot: false,
            revision: 0,
        });
        sink.flush();

        let stored = store.open_meeting(&bundle.meeting.id).unwrap();
        assert_eq!(stored.segments.len(), 1);
        assert_eq!(stored.segments[0].original_text, "hello");
        assert_eq!(stored.segments[0].translated_text.as_deref(), Some("你好"));
    }

    #[test]
    fn requested_finish_waits_for_every_generic_session() {
        let store = Arc::new(MeetingStore::open_in_memory().unwrap());
        let bundle = store
            .create_meeting(NewMeeting::live(
                "Lifecycle",
                Some("default".into()),
                "en",
                "zh",
            ))
            .unwrap();
        store.start_meeting(&bundle.meeting.id).unwrap();
        let active = Arc::new(Mutex::new(Some(ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "run-lifecycle".into(),
            timeline_offset_ms: 0,
            imported_audio: false,
        })));
        let sink = MeetingEventSink::start(Arc::clone(&store), Arc::clone(&active));
        sink.begin_sessions(2);
        sink.request_finish();

        sink.on_session_event(&SessionEvent::Disconnected("Finished".into()));
        sink.flush();
        assert!(active.lock().unwrap().is_some());

        sink.on_session_event(&SessionEvent::Disconnected("Finished".into()));
        sink.flush();
        assert!(active.lock().unwrap().is_none());
        assert_eq!(
            store.get_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Ended
        );
    }
}
