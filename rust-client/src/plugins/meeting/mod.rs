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
    /// True when another host feature owns the exclusive recognition session.
    pub host_session_busy: bool,
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
        ui::render(self, snapshot, ui)
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
