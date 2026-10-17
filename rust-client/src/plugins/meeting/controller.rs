use super::{
    MeetingAudioSource, MeetingInputRequest, MeetingStartRequest, MeetingUiSnapshot, recording,
    store::{
        AudioRun, MarkerKind, Meeting, MeetingBundle, MeetingSourceKind, MeetingStatus,
        MeetingStore, MeetingStoreError, NewMeeting, SegmentMarker,
    },
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingRoute {
    Library,
    Create,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingPane {
    Timeline,
    Minutes,
    Transcript,
}

#[derive(Debug, Clone)]
pub struct MeetingDraft {
    pub name: String,
    pub import_path: String,
    pub import_audio: bool,
    pub save_recording: bool,
    pub capture_source: MeetingAudioSource,
    pub source_language: String,
    pub target_language: String,
}

impl Default for MeetingDraft {
    fn default() -> Self {
        Self {
            name: "New meeting".into(),
            import_path: String::new(),
            import_audio: false,
            save_recording: false,
            capture_source: MeetingAudioSource::Microphone,
            source_language: "auto".into(),
            target_language: "zh".into(),
        }
    }
}

/// Immutable identity for one backend diarizer lifetime. The event pump reads
/// this lightweight binding without depending on UI state.
#[derive(Debug, Clone)]
pub struct ActiveMeetingCapture {
    pub meeting_id: String,
    pub topic_id: String,
    pub recognition_run_id: String,
    pub timeline_offset_ms: i64,
    pub imported_audio: bool,
}

pub type SharedMeetingCapture = Arc<Mutex<Option<ActiveMeetingCapture>>>;

pub struct MeetingController {
    pub(super) playback: super::playback::MeetingPlayback,
    pub store: Arc<MeetingStore>,
    pub route: MeetingRoute,
    pub pane: MeetingPane,
    pub meetings: Vec<Meeting>,
    pub bundle: Option<MeetingBundle>,
    pub previews: crate::session_coordinator::LivePreviews,
    pub draft: MeetingDraft,
    pub minutes_draft: String,
    pub minutes_dirty: bool,
    pub new_topic_title: String,
    pub search: String,
    pub browse_all: bool,
    pub highlights_only: bool,
    pub marker_drafts: HashMap<String, String>,
    pub evidence_target: Option<String>,
    pub error: Option<String>,
    pub active_capture: SharedMeetingCapture,
    pub pending_delete: Option<String>,
    pub speaker_name_drafts: HashMap<String, String>,
    pub speaker_merge_targets: HashMap<String, String>,
    persistent_storage_available: bool,
    recording_root: std::path::PathBuf,
    last_bundle_refresh: std::time::Instant,
}

impl MeetingController {
    pub fn storage_available(&self) -> bool {
        self.persistent_storage_available
    }

    pub fn open(project_root: &Path) -> Self {
        let database_path = project_root.join("runtime").join("meetings.sqlite3");
        let open_result = std::fs::create_dir_all(database_path.parent().unwrap_or(project_root))
            .map_err(|error| MeetingStoreError::InvalidData(error.to_string()))
            .and_then(|_| MeetingStore::open(&database_path));
        let (store, error, persistent_storage_available) = match open_result {
            Ok(store) => (Arc::new(store), None, true),
            Err(error) => {
                let fallback = MeetingStore::open_in_memory()
                    .expect("the in-memory meeting database must initialize");
                (
                    Arc::new(fallback),
                    Some(format!(
                        "Meeting database unavailable; recording is disabled to prevent data loss: {error}"
                    )),
                    false,
                )
            }
        };
        if let Err(error) = store.recover_interrupted_meetings() {
            log::error!("Could not recover interrupted meetings: {error}");
        }
        let meetings = store.list_meetings(200).unwrap_or_default();
        let recovery_meetings = store.list_meetings(usize::MAX).unwrap_or_default();
        recover_interrupted_recordings(&recovery_meetings);
        Self {
            playback: Default::default(),
            store,
            route: MeetingRoute::Library,
            pane: MeetingPane::Timeline,
            meetings,
            bundle: None,
            previews: Default::default(),
            draft: MeetingDraft::default(),
            minutes_draft: String::new(),
            minutes_dirty: false,
            new_topic_title: String::new(),
            search: String::new(),
            browse_all: false,
            highlights_only: false,
            marker_drafts: HashMap::new(),
            evidence_target: None,
            error,
            active_capture: Arc::new(Mutex::new(None)),
            pending_delete: None,
            speaker_name_drafts: HashMap::new(),
            speaker_merge_targets: HashMap::new(),
            persistent_storage_available,
            recording_root: project_root.join("runtime").join("meeting-recordings"),
            last_bundle_refresh: std::time::Instant::now(),
        }
    }

    pub fn refresh_library(&mut self) {
        match self.store.list_meetings(200) {
            Ok(meetings) => self.meetings = meetings,
            Err(error) => self.set_error(error),
        }
    }

    pub fn open_meeting(&mut self, meeting_id: &str) {
        if self
            .bundle
            .as_ref()
            .is_some_and(|bundle| bundle.meeting.id != meeting_id)
            && !self.save_pending_edits()
        {
            return;
        }
        match self.store.open_meeting(meeting_id) {
            Ok(bundle) => {
                let is_same_meeting = self
                    .bundle
                    .as_ref()
                    .is_some_and(|current| current.meeting.id == bundle.meeting.id);
                if !is_same_meeting {
                    self.pane = MeetingPane::Timeline;
                    self.search.clear();
                    self.highlights_only = false;
                    self.evidence_target = None;
                    self.new_topic_title.clear();
                    self.marker_drafts.clear();
                    self.speaker_name_drafts.clear();
                    self.speaker_merge_targets.clear();
                }
                for speaker in &bundle.speakers {
                    self.speaker_name_drafts
                        .entry(speaker.id.clone())
                        .or_insert_with(|| speaker.name.clone());
                }
                if !is_same_meeting || !self.minutes_dirty {
                    self.minutes_draft = bundle
                        .minutes
                        .as_ref()
                        .map(|minutes| minutes.markdown.clone())
                        .unwrap_or_default();
                    self.minutes_dirty = false;
                }
                if let Err(error) = self
                    .playback
                    .refresh(&bundle, self.active_meeting_id().is_none())
                {
                    self.error = Some(error);
                }
                self.bundle = Some(bundle);
                self.route = MeetingRoute::Detail;
            }
            Err(error) => self.set_error(error),
        }
    }

    pub fn reload_open_meeting(&mut self) {
        if let Some(id) = self.bundle.as_ref().map(|bundle| bundle.meeting.id.clone()) {
            self.open_meeting(&id);
        }
    }

    pub fn poll_live_view(&mut self) {
        if self.last_bundle_refresh.elapsed() < std::time::Duration::from_millis(400) {
            return;
        }
        if self.route == MeetingRoute::Library
            && self.meetings.iter().any(|meeting| {
                matches!(
                    meeting.status,
                    MeetingStatus::Live | MeetingStatus::Processing
                )
            })
        {
            self.last_bundle_refresh = std::time::Instant::now();
            self.refresh_library();
            return;
        }
        let waiting_for_terminal_refresh = self.bundle.as_ref().is_some_and(|bundle| {
            matches!(
                bundle.meeting.status,
                MeetingStatus::Live | MeetingStatus::Paused | MeetingStatus::Processing
            )
        });
        if self.route == MeetingRoute::Detail
            && (self.active_meeting_id().is_some() || waiting_for_terminal_refresh)
        {
            self.last_bundle_refresh = std::time::Instant::now();
            self.reload_open_meeting();
        }
    }

    pub fn reset_draft(&mut self, import_audio: bool, snapshot: &MeetingUiSnapshot) {
        self.draft = MeetingDraft {
            import_audio,
            capture_source: snapshot.default_audio_source,
            source_language: snapshot.default_source_language.clone(),
            target_language: snapshot.default_target_language.clone(),
            ..MeetingDraft::default()
        };
        self.error = None;
    }

    pub fn create(&mut self, request: &MeetingStartRequest) -> Option<String> {
        if !self.save_pending_edits() {
            return None;
        }
        if !self.persistent_storage_available {
            self.error = Some(
                "Meeting storage is unavailable. Fix the database error before recording.".into(),
            );
            return None;
        }
        let name = request.name.trim();
        let result = match &request.input {
            MeetingInputRequest::ImportedAudio { path } => {
                self.store.create_meeting(NewMeeting::imported_audio(
                    name,
                    path.display().to_string(),
                    &request.source_language,
                    &request.target_language,
                ))
            }
            MeetingInputRequest::Live {
                source,
                save_recording,
            } => {
                let mut meeting = NewMeeting::live(
                    name,
                    Some(capture_source_name(*source).into()),
                    &request.source_language,
                    &request.target_language,
                );
                if *save_recording {
                    meeting.recording_path = Some(
                        self.recording_root
                            .join(Uuid::new_v4().to_string())
                            .display()
                            .to_string(),
                    );
                    meeting.can_reprocess = true;
                }
                self.store.create_meeting(meeting)
            }
        };
        match result {
            Ok(bundle) => {
                let id = bundle.meeting.id.clone();
                self.open_meeting(&id);
                self.refresh_library();
                Some(id)
            }
            Err(error) => {
                self.set_error(error);
                None
            }
        }
    }

    pub fn begin_capture(&mut self, meeting_id: &str) -> bool {
        if !self.save_pending_edits() {
            return false;
        }
        self.playback.unload();
        let transition =
            self.store
                .get_meeting(meeting_id)
                .and_then(|meeting| match meeting.status {
                    MeetingStatus::Draft | MeetingStatus::Ended | MeetingStatus::Interrupted => {
                        self.store.start_meeting(meeting_id)
                    }
                    MeetingStatus::Imported | MeetingStatus::Failed => {
                        self.store.start_meeting(meeting_id)
                    }
                    MeetingStatus::Paused => self.store.resume_meeting(meeting_id),
                    MeetingStatus::Live => Ok(meeting),
                    _ => Err(MeetingStoreError::InvalidData(
                        "This meeting cannot start live capture in its current state".into(),
                    )),
                });
        match transition {
            Ok(_) => {
                let Ok(bundle) = self.store.open_meeting(meeting_id) else {
                    return false;
                };
                let topic_id = bundle.topics.last().map(|topic| topic.id.clone());
                let offset = bundle
                    .segments
                    .iter()
                    .map(|segment| segment.end_ms)
                    .max()
                    .unwrap_or(0);
                let Some(topic_id) = topic_id else {
                    return false;
                };
                let run = AudioRun {
                    recognition_run_id: Uuid::new_v4().to_string(),
                    timeline_offset_ms: offset,
                    audio_source_path: bundle.meeting.audio_source_path.clone(),
                };
                if let Err(error) = self.store.save_audio_run(meeting_id, &run) {
                    let _ = self.store.fail_meeting(meeting_id, error.to_string());
                    self.set_error(error);
                    return false;
                }
                if let Ok(mut active) = self.active_capture.lock() {
                    *active = Some(ActiveMeetingCapture {
                        meeting_id: meeting_id.to_owned(),
                        topic_id,
                        recognition_run_id: run.recognition_run_id.clone(),
                        timeline_offset_ms: offset,
                        imported_audio: bundle.meeting.source_kind
                            == MeetingSourceKind::ImportedAudio,
                    });
                }
                self.open_meeting(meeting_id);
                true
            }
            Err(error) => {
                self.set_error(error);
                false
            }
        }
    }

    pub fn pause_capture(&mut self) -> bool {
        let Some(meeting_id) = self.active_meeting_id() else {
            return false;
        };
        match self.store.pause_meeting(&meeting_id) {
            Ok(_) => {
                self.reload_open_meeting();
                self.refresh_library();
                true
            }
            Err(error) => {
                self.set_error(error);
                false
            }
        }
    }

    pub fn create_topic(&mut self) {
        let Some(meeting_id) = self.bundle.as_ref().map(|bundle| bundle.meeting.id.clone()) else {
            return;
        };
        let title =
            (!self.new_topic_title.trim().is_empty()).then_some(self.new_topic_title.trim());
        match self.store.create_topic(&meeting_id, title) {
            Ok(topic) => {
                if let Ok(mut active) = self.active_capture.lock()
                    && let Some(active) = active.as_mut()
                    && active.meeting_id == meeting_id
                {
                    active.topic_id = topic.id;
                }
                self.new_topic_title.clear();
                self.reload_open_meeting();
            }
            Err(error) => self.set_error(error),
        }
    }

    pub fn create_capture_topic(&mut self, meeting_id: &str, title: &str) -> bool {
        if self.active_meeting_id().as_deref() != Some(meeting_id) {
            self.error = Some("The meeting capture is no longer active".into());
            return false;
        }
        match self.store.create_topic(meeting_id, Some(title)) {
            Ok(topic) => {
                if let Ok(mut active) = self.active_capture.lock()
                    && let Some(active) = active.as_mut()
                {
                    active.topic_id = topic.id;
                }
                self.reload_open_meeting();
                true
            }
            Err(error) => {
                self.set_error(error);
                false
            }
        }
    }

    pub fn highlight(&mut self, segment_id: &str) {
        // A segment has one highlight; legacy annotations remain intact.
        if self.bundle.as_ref().is_some_and(|bundle| {
            bundle
                .markers
                .iter()
                .any(|marker| marker.segment_id == segment_id)
        }) {
            return;
        }
        // Keep the existing storage representation so old meeting databases work unchanged.
        match self.store.add_marker(segment_id, MarkerKind::Note, "") {
            Ok(_) => self.reload_open_meeting(),
            Err(error) => self.set_error(error),
        }
    }

    pub fn save_marker(&mut self, marker: &SegmentMarker) {
        match self
            .store
            .update_marker(&marker.id, marker.kind, &marker.text)
        {
            Ok(()) => {
                self.marker_drafts.remove(&marker.id);
                self.reload_open_meeting();
            }
            Err(error) => self.set_error(error),
        }
    }

    pub fn remove_marker(&mut self, marker_id: &str) {
        match self.store.delete_marker(marker_id) {
            Ok(()) => {
                self.marker_drafts.remove(marker_id);
                self.reload_open_meeting();
            }
            Err(error) => self.set_error(error),
        }
    }

    pub fn rename_speaker(&mut self, speaker_id: &str, name: &str) {
        match self.store.rename_speaker(speaker_id, name) {
            Ok(_) => self.reload_open_meeting(),
            Err(error) => self.set_error(error),
        }
    }

    pub fn merge_speakers(&mut self, source_id: &str, target_id: &str) {
        match self.store.merge_speakers(source_id, target_id) {
            Ok(_) => self.reload_open_meeting(),
            Err(error) => self.set_error(error),
        }
    }

    pub fn reprocessable_audio_path(&self) -> Option<std::path::PathBuf> {
        self.playback
            .clips
            .get(self.playback.selected)
            .map(|clip| clip.path.clone())
    }

    pub fn pause_playback(&mut self) {
        self.playback.pause();
    }

    /// Flush text drafts before leaving a meeting or exporting its records.
    pub fn save_pending_edits(&mut self) -> bool {
        let result = (|| -> Result<(), MeetingStoreError> {
            let Some(bundle) = self.bundle.as_mut() else {
                return Ok(());
            };
            if self.minutes_dirty {
                bundle.minutes = Some(
                    self.store
                        .save_minutes(&bundle.meeting.id, &self.minutes_draft)?,
                );
                self.minutes_dirty = false;
            }
            for marker in &mut bundle.markers {
                if let Some(text) = self.marker_drafts.get(&marker.id) {
                    if text != &marker.text {
                        self.store.update_marker(&marker.id, marker.kind, text)?;
                        marker.text.clone_from(text);
                    }
                    self.marker_drafts.remove(&marker.id);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.set_error(error);
            return false;
        }
        true
    }

    pub fn export_markdown(&mut self, meeting_id: &str) -> Option<(String, String)> {
        if !self.save_pending_edits() {
            return None;
        }
        match self.store.open_meeting(meeting_id) {
            Ok(bundle) => {
                let markdown = super::store::render_markdown(&bundle);
                Some((bundle.meeting.name, markdown))
            }
            Err(error) => {
                self.set_error(error);
                None
            }
        }
    }

    pub fn delete(&mut self, meeting_id: &str) {
        self.playback.unload();
        if self.active_meeting_id().as_deref() == Some(meeting_id) {
            self.error = Some("Finish the active meeting before deleting it".into());
            self.pending_delete = None;
            return;
        }
        let recording_path = self
            .store
            .get_meeting(meeting_id)
            .ok()
            .and_then(|meeting| meeting.recording_path.map(std::path::PathBuf::from));
        match self.store.delete_meeting(meeting_id) {
            Ok(()) => {
                if let Some(path) = recording_path {
                    self.remove_owned_recording_directory(&path);
                }
                if self
                    .bundle
                    .as_ref()
                    .is_some_and(|bundle| bundle.meeting.id == meeting_id)
                {
                    self.bundle = None;
                    self.route = MeetingRoute::Library;
                }
                self.pending_delete = None;
                self.refresh_library();
            }
            Err(error) => self.set_error(error),
        }
    }

    fn remove_owned_recording_directory(&mut self, path: &Path) {
        let Ok(relative) = path.strip_prefix(&self.recording_root) else {
            self.error = Some("Refused to delete a recording outside meeting storage".into());
            return;
        };
        if relative.components().count() != 1 {
            self.error = Some("Refused to delete an invalid meeting recording path".into());
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            self.error = Some(format!(
                "Meeting deleted, but its recording remains: {error}"
            ));
        }
    }

    pub fn active_meeting_id(&self) -> Option<String> {
        self.active_capture
            .lock()
            .ok()
            .and_then(|active| active.as_ref().map(|capture| capture.meeting_id.clone()))
    }

    pub fn meeting(&self, meeting_id: &str) -> Result<Meeting, MeetingStoreError> {
        self.store.get_meeting(meeting_id)
    }

    pub fn mark_imported_audio(&mut self, path: &Path) -> bool {
        let Some(capture) = self
            .active_capture
            .lock()
            .ok()
            .and_then(|active| active.clone())
        else {
            return false;
        };
        if let Err(error) =
            self.store
                .set_audio_run_source(&capture.meeting_id, &capture.recognition_run_id, path)
                .and_then(|_| self.store.process_meeting(&capture.meeting_id))
        {
            self.fail_active_meeting(&error.to_string());
            return false;
        }
        if let Ok(mut active) = self.active_capture.lock()
            && let Some(active) = active.as_mut()
        {
            active.imported_audio = true;
        }
        if let Some(run) = self.bundle.as_mut().and_then(|bundle| {
            bundle
                .audio_runs
                .iter_mut()
                .find(|run| run.recognition_run_id == capture.recognition_run_id)
        }) {
            run.audio_source_path = Some(path.to_string_lossy().into_owned());
        }
        true
    }

    /// Records a host-side startup failure and restores the controller to an
    /// idle, reloadable state. The store error is returned for host logging.
    pub fn fail_active_meeting(&mut self, error: &str) -> Option<MeetingStoreError> {
        let store_error = self
            .active_meeting_id()
            .and_then(|meeting_id| self.store.fail_meeting(&meeting_id, error).err());
        self.clear_active_capture();
        self.reload_open_meeting();
        self.refresh_library();
        self.error = Some(error.to_owned());
        store_error
    }

    pub fn resume_active_meeting(&mut self) -> Result<bool, MeetingStoreError> {
        let Some(meeting_id) = self.active_meeting_id() else {
            return Ok(false);
        };
        self.store.resume_meeting(&meeting_id)?;
        self.open_meeting(&meeting_id);
        Ok(true)
    }

    pub fn set_host_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn is_recording(&self, meeting_id: &str) -> bool {
        self.active_meeting_id().as_deref() == Some(meeting_id)
    }

    pub fn clear_active_capture(&self) {
        if let Ok(mut active) = self.active_capture.lock() {
            *active = None;
        }
    }

    fn set_error(&mut self, error: impl std::fmt::Display) {
        self.error = Some(error.to_string());
    }
}

impl Drop for MeetingController {
    fn drop(&mut self) {
        if !self.save_pending_edits() {
            log::error!("Could not save meeting edits: {:?}", self.error);
        }
    }
}

/// Publish durable WAV files left as `.pcm.part` by an abnormal exit. Each
/// recognition run is isolated in its own directory, so recovery never
/// overwrites a newer continuation of the same meeting.
fn recover_interrupted_recordings(meetings: &[Meeting]) {
    for meeting in meetings {
        let Some(root) = meeting.recording_path.as_deref() else {
            continue;
        };
        let Ok(runs) = std::fs::read_dir(root) else {
            continue;
        };
        for run in runs.flatten().filter(|entry| entry.path().is_dir()) {
            let directory = run.path();
            match recording::inspect_recoverable_recording(&directory) {
                Ok(Some(_)) => {
                    if let Err(error) = recording::finalize_recovered_recording(&directory) {
                        log::error!(
                            "Could not recover meeting recording {}: {error}",
                            directory.display()
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => log::error!(
                    "Could not inspect meeting recording {}: {error}",
                    directory.display()
                ),
            }
        }
    }
}

fn capture_source_name(source: MeetingAudioSource) -> &'static str {
    match source {
        MeetingAudioSource::Microphone => "microphone",
        MeetingAudioSource::SystemAudio => "system_audio",
        MeetingAudioSource::Both => "both",
    }
}

pub fn meeting_status_label(status: MeetingStatus) -> &'static str {
    match status {
        MeetingStatus::Draft => "Draft",
        MeetingStatus::Live => "Recording",
        MeetingStatus::Paused => "Paused",
        MeetingStatus::Ended => "Ended",
        MeetingStatus::Interrupted => "Interrupted",
        MeetingStatus::Imported => "Imported",
        MeetingStatus::Processing => "Processing",
        MeetingStatus::Failed => "Failed",
    }
}

pub fn can_continue(meeting: &Meeting) -> bool {
    meeting.source_kind == MeetingSourceKind::LiveCapture
        && matches!(
            meeting.status,
            MeetingStatus::Draft
                | MeetingStatus::Paused
                | MeetingStatus::Ended
                | MeetingStatus::Interrupted
                | MeetingStatus::Failed
        )
}
