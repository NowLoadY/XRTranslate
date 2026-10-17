//! Built-in meeting-record plugin.
//!
//! This module is the boundary between the host application and meeting-specific
//! storage, capture bookkeeping, retained recording, and UI state. The host
//! supplies a small immutable snapshot and executes the returned actions; the
//! meeting UI never receives the host application itself. External audio import
//! is a host capability shared with other media consumers.

pub(crate) use crate::media_import as audio_file;
pub mod controller;
pub mod events;
pub mod i18n;
mod playback;
pub mod recording;
pub(crate) mod store;
mod ui;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationSessionPlugin,
};
use controller::MeetingController;

/// Audio inputs understood by the meeting plugin.
///
/// The host maps this stable plugin type to its concrete capture implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeetingAudioSource {
    #[default]
    Microphone,
    SystemAudio,
    Both,
}

/// Host-owned values which the meeting UI may read but never mutate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingUiSnapshot {
    pub default_audio_source: MeetingAudioSource,
    pub default_source_language: String,
    pub default_target_language: String,
    pub languages: xrtranslate_engine::language::LanguageCapabilities,
    /// Compatibility snapshot; shared capture no longer blocks other tasks.
    pub host_session_busy: bool,
    pub waiting_for_microphone: bool,
    pub language: crate::i18n::UiLanguage,
}

impl Default for MeetingUiSnapshot {
    fn default() -> Self {
        Self {
            default_audio_source: MeetingAudioSource::Microphone,
            default_source_language: "auto".into(),
            default_target_language: "zh".into(),
            languages: Default::default(),
            host_session_busy: false,
            waiting_for_microphone: false,
            language: crate::i18n::UiLanguage::English,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingInputRequest {
    Live {
        source: MeetingAudioSource,
        save_recording: bool,
    },
    ImportedAudio {
        path: PathBuf,
    },
}

/// Complete, immutable request captured when the user presses Start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingStartRequest {
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    pub input: MeetingInputRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingReprocessRequest {
    pub meeting_id: String,
    pub audio_path: PathBuf,
    /// The host creates and selects this topic only after `begin_capture`
    /// succeeds, preventing the topic from being attached to a stale run.
    pub topic_title: String,
}

/// Side effects which require host services such as audio capture, backend
/// sessions, file dialogs, or application-wide resource arbitration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MeetingAction {
    #[default]
    None,
    CreateAndStart(MeetingStartRequest),
    Continue(String),
    Pause,
    End,
    Export(String),
    Reprocess(MeetingReprocessRequest),
}

/// All mutable runtime state owned by the meeting plugin.
pub struct MeetingPlugin {
    pub controller: MeetingController,
    pub event_sink: events::MeetingEventSink,
    pub audio_import: Option<audio_file::AudioImportHandle>,
    pub meeting_recording: Option<recording::MeetingRecording>,
}

impl MeetingPlugin {
    pub fn open(project_root: &Path) -> Self {
        let controller = MeetingController::open(project_root);
        let event_sink = events::MeetingEventSink::start(
            Arc::clone(&controller.store),
            Arc::clone(&controller.active_capture),
        );
        Self {
            controller,
            event_sink,
            audio_import: None,
            meeting_recording: None,
        }
    }

    pub(crate) fn translation_context(&self) -> xrtranslate_prompt::TaskContext {
        let Ok(active) = self.controller.active_capture.lock() else {
            return Default::default();
        };
        let Some(active) = active.as_ref() else {
            return Default::default();
        };
        capture_context(active, self.controller.bundle.as_ref())
    }

    /// A busy plugin cannot be disabled without first completing or cancelling
    /// its active work and durably checkpointing owned data.
    pub fn is_busy(&self) -> bool {
        self.controller.active_meeting_id().is_some()
            || self.audio_import.is_some()
            || self.meeting_recording.is_some()
    }

    pub fn set_audio_import(&mut self, import: audio_file::AudioImportHandle) {
        self.audio_import = Some(import);
    }

    pub fn clear_audio_import(&mut self) {
        self.audio_import = None;
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.controller.set_host_error(error);
    }

    pub fn fail_active_startup(&mut self, error: &str) {
        if let Some(store_error) = self.controller.fail_active_meeting(error) {
            log::error!("Could not mark failed meeting startup: {store_error}");
        }
    }

    pub fn disable_block_reason(&self) -> Option<&'static str> {
        self.is_busy()
            .then_some("Finish the active meeting before disabling this plugin")
    }

    pub fn render_page(
        &mut self,
        snapshot: &MeetingUiSnapshot,
        ui: &mut eframe::egui::Ui,
    ) -> MeetingAction {
        self.controller.previews = if self.controller.bundle.as_ref().is_some_and(|bundle| {
            self.controller.active_meeting_id().as_deref() == Some(bundle.meeting.id.as_str())
        }) {
            self.event_sink.previews()
        } else {
            Default::default()
        };
        ui::render(self, snapshot, ui)
    }
}

fn capture_context(
    active: &controller::ActiveMeetingCapture,
    bundle: Option<&store::MeetingBundle>,
) -> xrtranslate_prompt::TaskContext {
    use xrtranslate_engine::language::SupportedLanguage;
    use xrtranslate_prompt::{ContextFacts, PromptTurn, SurroundingSource, TaskContext};
    let scope = format!(
        "meeting:{}:{}",
        active.meeting_id, active.recognition_run_id
    );
    let Some(bundle) =
        bundle.filter(|bundle| !active.imported_audio && bundle.meeting.id == active.meeting_id)
    else {
        return TaskContext::conversation(scope);
    };
    // Seed a new run from durable facts. The backend owns subsequent history.
    let mut segments: Vec<_> = bundle
        .segments
        .iter()
        .filter(|segment| segment.is_final && !segment.original_text.trim().is_empty())
        .collect();
    segments.sort_by_key(|segment| (segment.start_ms, segment.end_ms));
    let mut recent_turns = Vec::new();
    let mut source_before = Vec::new();
    for segment in &segments[segments.len().saturating_sub(8)..] {
        if let (Some(translated), Some(source), Some(target)) = (
            segment
                .translated_text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty()),
            SupportedLanguage::parse(&segment.source_language),
            SupportedLanguage::parse(&segment.target_language),
        ) && source != target
        {
            recent_turns.push(PromptTurn {
                turn_id: Some(segment.id.clone()),
                speaker_id: String::new(),
                source_language: source.code().to_owned(),
                target_language: target.code().to_owned(),
                source_text: segment.original_text.trim().to_owned(),
                translated_text: translated.to_owned(),
            });
        } else {
            source_before.push(segment.original_text.trim());
        }
    }
    let before = source_before.join("\n");
    TaskContext::Conversation {
        scope,
        seed: ContextFacts {
            recent_turns,
            surrounding_source: (!before.is_empty()).then_some(SurroundingSource {
                speaker_id: String::new(),
                source_language: String::new(),
                before,
                after: String::new(),
            }),
        },
    }
}

impl TranslationSessionPlugin for MeetingPlugin {
    fn translation_session_binding(&self) -> Option<PluginSessionBinding> {
        let active = self.controller.active_capture.lock().ok()?;
        let active = active.as_ref()?;
        let display_name_key = if active.imported_audio {
            "Meeting Audio Import"
        } else {
            "Meeting Notes"
        };
        Some(PluginSessionBinding {
            owner: PluginSessionOwner::new(
                super::PluginId::MEETING.as_str(),
                active.meeting_id.clone(),
                display_name_key,
                "Open meeting controls",
                "A meeting owns the active audio session",
            ),
            output_policy: SessionOutputPolicy::PluginOnly,
            host_tts: false,
            external_audio_gate: false,
            finish_when_audio_ends: active.imported_audio,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_prompt::TaskContext;

    #[test]
    fn resumed_meeting_context_uses_chronology_and_excludes_imports() {
        let store = store::MeetingStore::open_in_memory().unwrap();
        let bundle = store
            .create_meeting(store::NewMeeting::live("Meeting", None, "auto", "zh"))
            .unwrap();
        for (text, start_ms, is_final) in [
            ("Later", 2_000, true),
            ("Earlier", 0, true),
            ("Preview", 4_000, false),
        ] {
            store
                .upsert_segment(store::NewSegment {
                    meeting_id: bundle.meeting.id.clone(),
                    external_key: text.into(),
                    topic_id: bundle.topics[0].id.clone(),
                    original_text: text.into(),
                    translated_text: Some("Translation".into()),
                    source_language: String::new(),
                    target_language: String::new(),
                    start_ms,
                    end_ms: start_ms + 1_000,
                    source: store::SegmentSource::Microphone,
                    recognition_run_id: "previous-run".into(),
                    speaker_token: None,
                    is_final,
                })
                .unwrap();
        }
        let bundle = store.open_meeting(&bundle.meeting.id).unwrap();
        let mut capture = controller::ActiveMeetingCapture {
            meeting_id: bundle.meeting.id.clone(),
            topic_id: bundle.topics[0].id.clone(),
            recognition_run_id: "resumed-run".into(),
            timeline_offset_ms: 5_000,
            imported_audio: false,
        };
        let TaskContext::Conversation { scope, seed } = capture_context(&capture, Some(&bundle))
        else {
            panic!("A meeting run owns a conversation");
        };
        assert!(seed.recent_turns.is_empty());
        let source = seed.surrounding_source.unwrap();
        assert_eq!(source.before, "Earlier\nLater");
        assert!(source.source_language.is_empty());

        capture.imported_audio = true;
        assert_eq!(
            capture_context(&capture, Some(&bundle)),
            TaskContext::conversation(scope)
        );
        capture.imported_audio = false;
        capture.meeting_id = "another-meeting".into();
        assert_eq!(
            capture_context(&capture, Some(&bundle)),
            TaskContext::conversation("meeting:another-meeting:resumed-run")
        );
    }

    #[test]
    fn resumed_context_partitions_the_last_completed_records_without_guessing_routes() {
        let store = store::MeetingStore::open_in_memory().unwrap();
        let meeting = store
            .create_meeting(store::NewMeeting::live("Meeting", None, "auto", "zh"))
            .unwrap();
        for index in (0..11).rev() {
            let (source, target, translation) = match index {
                3 => ("zh", "en", Some("Decision")),
                4 => ("", "", Some("Legacy translation")),
                5 => ("en", "unknown", Some("Unknown route")),
                6 => ("en", "zh", None),
                7 => ("en", "zh", Some(" ")),
                8 => ("en", "en", Some("Source only")),
                _ => ("en", "zh", Some("译文")),
            };
            store
                .upsert_segment(store::NewSegment {
                    meeting_id: meeting.meeting.id.clone(),
                    external_key: index.to_string(),
                    topic_id: meeting.topics[0].id.clone(),
                    original_text: format!("Line {}", if index == 9 { 2 } else { index }),
                    translated_text: translation.map(str::to_owned),
                    source_language: source.into(),
                    target_language: target.into(),
                    start_ms: index * 1000,
                    end_ms: index * 1000 + 500,
                    source: store::SegmentSource::Microphone,
                    recognition_run_id: "previous-run".into(),
                    speaker_token: None,
                    is_final: index != 10,
                })
                .unwrap();
        }
        let bundle = store.open_meeting(&meeting.meeting.id).unwrap();
        let capture = controller::ActiveMeetingCapture {
            meeting_id: meeting.meeting.id,
            topic_id: meeting.topics[0].id.clone(),
            recognition_run_id: "resumed-run".into(),
            timeline_offset_ms: 10_000,
            imported_audio: false,
        };
        let TaskContext::Conversation { seed, .. } = capture_context(&capture, Some(&bundle))
        else {
            panic!("A meeting run owns a conversation");
        };
        assert_eq!(
            seed.recent_turns
                .iter()
                .map(|turn| turn.source_text.as_str())
                .collect::<Vec<_>>(),
            ["Line 2", "Line 3", "Line 2"],
        );
        assert_ne!(seed.recent_turns[0].turn_id, seed.recent_turns[2].turn_id);
        assert_eq!(seed.recent_turns[1].source_language, "zh");
        assert_eq!(seed.recent_turns[1].target_language, "en");
        assert_eq!(seed.recent_turns[1].translated_text, "Decision");
        let surrounding = seed.surrounding_source.unwrap();
        assert_eq!(surrounding.before, "Line 4\nLine 5\nLine 6\nLine 7\nLine 8");
        assert!(surrounding.after.is_empty());
    }
}
