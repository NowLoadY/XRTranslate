pub mod backend;
pub mod controller;
pub mod i18n;
pub mod installer;
pub mod subtitles;
pub mod task;
pub mod ui;

use crate::i18n::UiLanguage;
use crate::session_coordinator::{
    PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationSessionPlugin,
};
use controller::VideoPlayerController;
use std::path::PathBuf;
use std::time::Duration;
#[allow(unused_imports)]
pub use task::MediaType;
pub use task::VideoSubtitleMode;

#[derive(Clone, Debug)]
pub struct VideoPlayerUiSnapshot {
    pub language: UiLanguage,
    pub source_languages: Vec<(&'static str, &'static str)>,
    pub target_languages: Vec<(&'static str, &'static str)>,
}

pub(crate) fn runtime_bin_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(parent) = exe_path.parent()
    {
        dirs.push(parent.join("resources").join("bin"));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        dirs.push(current_dir.join("resources").join("bin"));
    }
    dirs.push(PathBuf::from("rust-client").join("resources").join("bin"));
    dirs
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerTranslationRequest {
    ImportMediaFile {
        path: PathBuf,
        source_language: String,
        target_language: String,
        recognition: crate::client_settings::RecognitionSettings,
        audio_channels: Vec<task::AudioChannelItem>,
    },
    LiveStream {
        source_language: String,
        target_language: String,
        recognition: crate::client_settings::RecognitionSettings,
        audio_channels: Vec<task::AudioChannelItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum VideoPlayerAction {
    #[default]
    None,
    StartTranslation(PlayerTranslationRequest),
    StopTranslation,
}

pub struct VideoPlayerPlugin {
    pub controller: VideoPlayerController,
}

impl Default for VideoPlayerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoPlayerPlugin {
    pub fn new() -> Self {
        Self {
            controller: VideoPlayerController::default(),
        }
    }

    pub fn render_page(
        &mut self,
        snapshot: &VideoPlayerUiSnapshot,
        ui: &mut eframe::egui::Ui,
    ) -> VideoPlayerAction {
        ui::render(self, snapshot, ui)
    }

    pub fn on_visibility_changed(&mut self, is_visible: bool) {
        if !is_visible || self.controller.route != controller::VideoPlayerRoute::Player {
            self.controller.fullscreen_mode = false;
            self.controller.release_native_host();
        }
    }

    pub fn on_translation_cue(
        &mut self,
        cue: subtitles::SubtitleCue,
        metadata: subtitles::SubtitleMetadata,
    ) {
        self.controller
            .subtitles
            .add_cue_with_metadata(cue, metadata);
    }

    pub fn active_task_id(&self) -> Option<String> {
        self.controller.active_task_id.clone()
    }
    pub fn has_active_task(&self) -> bool {
        self.controller.active_task_id.is_some()
    }
    pub fn pause_task(&mut self) {
        self.controller.pause_task();
    }
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.controller.error = Some(error.into());
    }
    pub fn update_import_progress(
        &mut self,
        stage: ImportProgressStage,
        fraction: Option<f32>,
        position: Duration,
        duration: Option<Duration>,
    ) {
        match stage {
            ImportProgressStage::Extracting => {
                self.controller.is_extracting = true;
                self.controller.extraction_progress = fraction;
                self.controller.extract_position = Some(position);
                self.controller.extract_duration = duration;
            }
            ImportProgressStage::Recognizing => {
                self.controller.is_extracting = false;
                self.controller.extraction_progress = Some(1.0);
                self.controller.recognition_progress = fraction;
                self.controller.recognize_position = Some(position);
                self.controller.recognize_duration = duration;
            }
        }
    }
    pub fn complete_import(&mut self) {
        self.controller.is_extracting = false;
        self.controller.extraction_progress = Some(1.0);
        self.controller.recognition_progress = Some(1.0);
    }
    pub fn stop_import(&mut self) {
        self.controller.is_extracting = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportProgressStage {
    Extracting,
    Recognizing,
}

impl TranslationSessionPlugin for VideoPlayerPlugin {
    fn translation_session_binding(&self) -> Option<PluginSessionBinding> {
        let task_id = self.controller.active_task_id.clone()?;
        let is_file = matches!(
            &self.controller.current_source,
            Some(backend::MediaSource::LocalFile(_))
        );
        Some(PluginSessionBinding {
            owner: PluginSessionOwner::new(
                super::PluginId::VIDEO_PLAYER.as_str(),
                task_id,
                "Media Player",
                "Open Media Player",
                "Media Player owns the active translation session",
            ),
            output_policy: SessionOutputPolicy::Host,
            host_tts: !is_file,
            external_audio_gate: !is_file,
            finish_when_audio_ends: is_file,
        })
    }
}

#[cfg(test)]
mod session_binding_tests {
    use super::*;

    #[test]
    fn local_files_disable_live_only_session_capabilities() {
        let mut plugin = VideoPlayerPlugin::new();
        plugin.controller.active_task_id = Some("file-task".into());
        plugin.controller.current_source =
            Some(backend::MediaSource::LocalFile("movie.mp4".into()));

        let binding = plugin.translation_session_binding().unwrap();
        assert!(binding.publish_to_host_outputs());
        assert!(!binding.host_tts);
        assert!(!binding.external_audio_gate);
        assert!(binding.finish_when_audio_ends);
    }

    #[test]
    fn network_streams_inherit_host_live_capabilities() {
        let mut plugin = VideoPlayerPlugin::new();
        plugin.controller.active_task_id = Some("stream-task".into());
        plugin.controller.current_source = Some(backend::MediaSource::NetworkStream(
            "https://example.test/live".into(),
        ));

        let binding = plugin.translation_session_binding().unwrap();
        assert!(binding.host_tts);
        assert!(binding.external_audio_gate);
        assert!(!binding.finish_when_audio_ends);
    }
}
