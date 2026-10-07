use super::{
    backend::{MediaBackend, MediaSource, PlaybackStatus, window::NativeVideoHost},
    subtitles::SubtitleTimeline,
    task::{AudioChannelItem, MediaTask, MediaTaskStore, SubtitleMode},
};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MediaRoute {
    #[default]
    Library,
    Create,
    Detail,
}

pub struct MediaController {
    pub route: MediaRoute,
    pub store: MediaTaskStore,
    pub storage_dir: PathBuf,
    pub active_task_id: Option<String>,
    pub current_source: Option<MediaSource>,
    pub backend: Option<Box<dyn MediaBackend>>,
    pub native_host: Option<NativeVideoHost>,
    pub subtitles: SubtitleTimeline,
    pub show_subtitles: bool,
    pub fullscreen_mode: bool,
    pub error: Option<String>,
    pub muted: bool,
    pub volume: f32,

    pub search_query: String,
    pub draft_title: String,
    pub draft_subtitles: bool,
    pub draft_source: String,
    pub draft_source_lang: String,
    pub draft_target_lang: String,
    pub draft_subtitle_mode: SubtitleMode,
    pub draft_recognition: crate::client_settings::RecognitionSettings,

    pub parent_window: usize,
    pub video_unavailable: bool,
    pub last_save_instant: Instant,
    pub last_manual_scroll: Option<Instant>,
    pub last_auto_scrolled_cue_id: Option<String>,
    pub(super) dirty: bool,
    playback_loaded: bool,
    pub mpv_installer: super::installer::MpvInstaller,

    pub is_extracting: bool,
    pub extraction_progress: Option<f32>,
    pub extract_position: Option<std::time::Duration>,
    pub extract_duration: Option<std::time::Duration>,
    pub recognition_progress: Option<f32>,
    pub recognize_position: Option<std::time::Duration>,
    pub recognize_duration: Option<std::time::Duration>,
}

impl Default for MediaController {
    fn default() -> Self {
        let storage_dir = PathBuf::from("runtime");
        let store = MediaTaskStore::load_from_dir(&storage_dir);
        let backend: Option<Box<dyn MediaBackend>> =
            if !crate::plugins::PluginId::MEDIA.is_supported() {
                None
            } else {
                match super::backend::mpv::MpvBackend::new() {
                    Ok(b) => Some(Box::new(b)),
                    Err(e) => {
                        log::warn!("MPV backend not initialized on startup: {}", e);
                        #[cfg(any(windows, target_os = "linux"))]
                        {
                            super::backend::audio::AudioBackend::new()
                                .ok()
                                .map(|backend| Box::new(backend) as Box<dyn MediaBackend>)
                        }
                        #[cfg(not(any(windows, target_os = "linux")))]
                        {
                            None
                        }
                    }
                }
            };

        Self {
            route: MediaRoute::Library,
            store,
            storage_dir,
            active_task_id: None,
            current_source: None,
            backend,
            native_host: None,
            subtitles: SubtitleTimeline::new(),
            show_subtitles: true,
            fullscreen_mode: false,
            error: None,
            muted: false,
            volume: 1.0,
            search_query: String::new(),
            draft_title: String::new(),
            draft_subtitles: false,
            draft_source: String::new(),
            draft_source_lang: "auto".into(),
            draft_target_lang: "zh".into(),
            draft_subtitle_mode: SubtitleMode::RealtimeTranslation,
            draft_recognition: crate::client_settings::RecognitionSettings {
                background_noise: 0.6,
                pause_tolerance: 1.0,
                continuous_recognition: false,
            },
            parent_window: 0,
            video_unavailable: false,
            last_save_instant: Instant::now(),
            last_manual_scroll: None,
            last_auto_scrolled_cue_id: None,
            dirty: false,
            playback_loaded: false,
            mpv_installer: super::installer::MpvInstaller::default(),
            is_extracting: false,
            extraction_progress: None,
            extract_position: None,
            extract_duration: None,
            recognition_progress: None,
            recognize_position: None,
            recognize_duration: None,
        }
    }
}

impl MediaController {
    pub fn release_native_host(&mut self) {
        if self.native_host.is_none() {
            return;
        }
        if let Some(backend) = &mut self.backend {
            backend.attach_native_host(std::ptr::null_mut());
        }
        self.native_host.take();
    }

    pub fn open_library(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        self.pause_task();
        self.route = MediaRoute::Library;
        self.active_task_id = None;
        self.current_source = None;
        self.last_auto_scrolled_cue_id = None;
        self.release_native_host();
    }

    pub fn open_create(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        self.pause_task();
        self.route = MediaRoute::Create;
        self.active_task_id = None;
        self.current_source = None;
        self.last_auto_scrolled_cue_id = None;
        self.draft_title.clear();
        self.draft_source.clear();
        self.draft_source_lang = "auto".into();
        self.draft_target_lang = "zh".into();
        self.draft_subtitle_mode = SubtitleMode::RealtimeTranslation;
        self.draft_recognition = crate::client_settings::RecognitionSettings {
            background_noise: 0.6,
            pause_tolerance: 1.0,
            continuous_recognition: false,
        };
        self.error = None;
        self.release_native_host();
    }

    pub fn start_draft_task(&mut self) -> Result<String, String> {
        let input = self.draft_source.trim();
        if input.is_empty() {
            return Err("Please enter a media stream URL or select a local file".into());
        }

        let (source, default_title, media_type) = if input.starts_with("http://")
            || input.starts_with("https://")
            || input.starts_with("rtsp://")
            || input.starts_with("rtmp://")
        {
            let title = input
                .split('?')
                .next()
                .unwrap_or(input)
                .split('/')
                .next_back()
                .filter(|s| !s.is_empty())
                .unwrap_or("Network Stream")
                .to_string();
            (
                MediaSource::NetworkStream(input.to_string()),
                title,
                super::task::MediaType::Video,
            )
        } else {
            let path = PathBuf::from(input);
            if !path.is_file() {
                return Err("Local media file does not exist or URL is invalid".into());
            }
            let title = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Local Media".into());
            let media_type = super::task::detect_media_type(&path);
            (MediaSource::LocalFile(path), title, media_type)
        };

        if self.draft_subtitles && media_type != super::task::MediaType::Subtitles {
            return Err("Choose an SRT or VTT file".into());
        }

        let title = if self.draft_title.trim().is_empty() {
            default_title
        } else {
            self.draft_title.trim().to_string()
        };

        let mut task = MediaTask::new_with_media_type(
            title,
            source.clone(),
            media_type,
            self.draft_source_lang.clone(),
            self.draft_target_lang.clone(),
            self.draft_subtitle_mode.clone(),
            self.draft_recognition.clone(),
        );

        if media_type == super::task::MediaType::Subtitles {
            task.subtitles = SubtitleTimeline::parse(
                &std::fs::read_to_string(input).map_err(|e| e.to_string())?,
            )?;
            task.duration_ms = task
                .subtitles
                .cues()
                .iter()
                .map(|cue| cue.end_ms)
                .max()
                .unwrap_or(0);
            task.subtitle_mode = SubtitleMode::ImportedSrt(PathBuf::from(input));
        }

        let task_id = task.id.clone();
        self.store.add_or_update(task);
        self.store
            .save_to_dir(&self.storage_dir)
            .map_err(|e| e.to_string())?;

        self.open_task(&task_id)?;
        Ok(task_id)
    }

    pub fn open_task(&mut self, task_id: &str) -> Result<(), String> {
        let task = self.store.get(task_id).ok_or("Task not found")?.clone();
        self.active_task_id = Some(task_id.to_string());
        self.current_source = Some(task.source.clone());
        self.subtitles = task.subtitles.clone();
        self.release_native_host();
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        self.route = MediaRoute::Detail;
        self.error = None;
        self.recognize_duration = None;
        self.recognition_progress = None;
        self.last_manual_scroll = None;
        self.last_auto_scrolled_cue_id = None;
        self.playback_loaded = false;
        self.video_unavailable = false;

        if task.media_type == super::task::MediaType::AudioOnly {
            self.release_native_host();
        }

        if task.media_type != super::task::MediaType::Subtitles
            && let Some(backend) = &mut self.backend
        {
            backend.set_audio_only_mode(
                task.media_type == super::task::MediaType::AudioOnly || !cfg!(windows),
            );
            let loaded = match &task.source {
                MediaSource::LocalFile(path) => backend.load_local_file(path.clone()),
                MediaSource::NetworkStream(url) => backend.load_stream_url(url.clone()),
            };
            match loaded {
                Ok(()) => self.playback_loaded = true,
                Err(error) => self.error = Some(error),
            }
            backend.set_channel_routing(&task.audio_channels);
        }

        if let Some(t) = self.store.get_mut(task_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            t.last_played_sec = now;
        }
        self.save_active();
        Ok(())
    }

    pub fn is_subtitle_task(&self) -> bool {
        self.active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .is_some_and(|task| task.media_type == super::task::MediaType::Subtitles)
    }

    pub fn can_show_video(&self) -> bool {
        cfg!(windows)
            && self
                .backend
                .as_ref()
                .is_some_and(|backend| backend.supports_video())
            && !self.video_unavailable
            && self.parent_window != 0
            && self.can_play()
            && !self.is_audio_only_task()
    }

    pub fn can_play(&self) -> bool {
        self.playback_loaded && !self.is_subtitle_task()
    }

    pub fn save_active(&mut self) {
        if let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get_mut(id))
        {
            task.subtitles = self.subtitles.clone();
        }
        match self.store.save_to_dir(&self.storage_dir) {
            Ok(()) => self.dirty = false,
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn is_audio_only_task(&self) -> bool {
        self.active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .is_some_and(|task| task.media_type == super::task::MediaType::AudioOnly)
    }

    pub fn start_task(&mut self) {
        self.is_extracting = false;
        self.extraction_progress = None;
        self.recognition_progress = None;
        self.recognize_duration = None;
        self.recognize_position = None;
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get_mut(task_id) {
                task.is_task_running = true;
            }
            self.save_active();
        }
    }

    pub fn pause_task(&mut self) {
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get_mut(task_id) {
                task.is_task_running = false;
            }
            self.save_active();
        }
    }

    pub fn clear_and_restart_task(&mut self) {
        if self.is_subtitle_task() {
            self.subtitles.clear_translations();
        } else {
            self.subtitles.clear();
        }
        self.is_extracting = false;
        self.extraction_progress = None;
        self.extract_position = None;
        self.extract_duration = None;
        self.recognition_progress = None;
        self.recognize_position = None;
        self.recognize_duration = None;
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get_mut(task_id) {
                task.subtitles = self.subtitles.clone();
                task.is_task_running = true;
            }
            self.save_active();
        }
        self.last_manual_scroll = None;
    }

    pub fn apply_channel_routing(&mut self) {
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get(task_id) {
                let channels = task.audio_channels.clone();
                if let Some(backend) = &mut self.backend {
                    backend.set_channel_routing(&channels);
                }
            }
            self.save_active();
        }
    }

    pub fn delete_task(&mut self, task_id: &str) {
        if self.active_task_id.as_deref() == Some(task_id) {
            if let Some(backend) = &mut self.backend {
                backend.stop();
            }
            self.release_native_host();
            self.active_task_id = None;
            self.current_source = None;
            self.route = MediaRoute::Library;
        }
        self.store.delete(task_id);
        self.save_active();
    }

    pub fn toggle_play(&mut self) {
        let Some(source) = &self.current_source else {
            return;
        };
        if let Some(backend) = &mut self.backend {
            if backend.get_status() == PlaybackStatus::Playing {
                backend.pause();
            } else {
                if backend.get_status() == PlaybackStatus::Stopped && backend.get_duration_ms() == 0
                {
                    match source {
                        MediaSource::LocalFile(p) => {
                            let _ = backend.load_local_file(p.clone());
                        }
                        MediaSource::NetworkStream(u) => {
                            let _ = backend.load_stream_url(u.clone());
                        }
                    }
                }
                backend.play();
            }
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Some(backend) = &mut self.backend {
            backend.set_mute(self.muted);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(backend) = &mut self.backend {
            backend.set_volume(volume);
        }
    }

    pub fn toggle_fullscreen(&mut self) {
        self.fullscreen_mode = !self.fullscreen_mode;
    }

    pub fn try_init_backend(&mut self) -> bool {
        if self
            .backend
            .as_ref()
            .is_some_and(|backend| backend.supports_video())
        {
            return true;
        }
        match super::backend::mpv::MpvBackend::new() {
            Ok(b) => {
                self.release_native_host();
                self.backend = Some(Box::new(b));
                if let Some(backend) = &mut self.backend {
                    backend.set_volume(self.volume);
                    backend.set_mute(self.muted);
                }
                self.error = None;
                if let Some(id) = self.active_task_id.clone() {
                    let _ = self.open_task(&id);
                }
                log::info!("MPV backend initialized successfully.");
                true
            }
            Err(e) => {
                log::warn!("MPV backend initialization: {e}");
                false
            }
        }
    }

    pub fn tick(&mut self) {
        if let Some(Ok(())) = self.mpv_installer.poll() {
            self.try_init_backend();
        }

        let is_audio_only = self.is_audio_only_task();
        let show_subs = self.show_subtitles && !is_audio_only && self.current_source.is_some();

        if let Some(backend) = &mut self.backend {
            backend.tick();
            if let Some(error) = backend.take_error() {
                self.error = Some(error);
            }

            // Synchronize detected audio channels from backend stream
            if self.playback_loaded
                && let Some(active_id) = &self.active_task_id
                && let Some(detected_count) = backend.get_audio_channel_count()
                && detected_count > 0
                && let Some(task) = self.store.get_mut(active_id)
                && task.audio_channels.len() != detected_count
            {
                task.audio_channels = AudioChannelItem::default_for_count(detected_count);
                backend.set_channel_routing(&task.audio_channels);
                self.dirty = true;
            }

            if show_subs && backend.get_status() == PlaybackStatus::Playing {
                let current_time_ms = backend.get_time_ms();
                if let Some(cue) = self.subtitles.active_cue_at(current_time_ms) {
                    let text = if let Some(trans) = &cue.translated_text {
                        if trans != &cue.original_text && !trans.trim().is_empty() {
                            format!("{}\n{}", cue.original_text, trans)
                        } else {
                            cue.original_text.clone()
                        }
                    } else {
                        cue.original_text.clone()
                    };
                    backend.set_osd_subtitle(&text);
                } else {
                    backend.set_osd_subtitle("");
                }
            } else {
                backend.set_osd_subtitle("");
            }
        }

        if self.dirty
            && self.active_task_id.is_some()
            && self.last_save_instant.elapsed().as_secs() >= 5
        {
            let duration = self.get_duration_ms();
            if let Some(task) = self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get_mut(id))
                && duration > 0
            {
                task.duration_ms = duration;
            }
            self.last_save_instant = Instant::now();
            self.save_active();
        }
    }

    pub fn get_time_ms(&self) -> i64 {
        self.backend.as_ref().map(|b| b.get_time_ms()).unwrap_or(0)
    }

    pub fn seek_to(&mut self, ms: i64, play: bool) {
        if let Some(backend) = &mut self.backend {
            backend.seek(ms);
            if play {
                backend.play();
            }
            self.last_manual_scroll = None;
            self.last_auto_scrolled_cue_id = None;
        }
    }

    /// File delivery is separate from backend recognition/translation progress.
    pub fn translation_progress(&self) -> Option<f32> {
        let duration = self
            .recognize_duration
            .map(|duration| duration.as_millis() as f64)
            .filter(|duration| *duration > 0.0)
            .or_else(|| {
                self.active_task_id
                    .as_deref()
                    .and_then(|id| self.store.get(id))
                    .map(|task| task.duration_ms as f64)
                    .filter(|duration| *duration > 0.0)
            })
            .or_else(|| (self.get_duration_ms() > 0).then_some(self.get_duration_ms() as f64))?;
        let translated_to = self
            .subtitles
            .cues()
            .iter()
            .filter(|cue| {
                cue.translated_text
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
            })
            .map(|cue| cue.end_ms)
            .max()
            .unwrap_or(0)
            .max(0) as f64;
        // Only the session Finished event can declare completion; rounded percentages
        // must not advertise 100% while the backend is still draining its last turn.
        Some((translated_to / duration).clamp(0.0, 0.99) as f32)
    }

    pub fn get_duration_ms(&self) -> i64 {
        if !self.can_play() {
            return 0;
        }
        self.backend
            .as_ref()
            .map(|b| b.get_duration_ms())
            .unwrap_or(0)
    }

    pub fn get_status(&self) -> PlaybackStatus {
        self.backend
            .as_ref()
            .map(|b| b.get_status())
            .unwrap_or(PlaybackStatus::Stopped)
    }

    pub fn get_diagnostics(&self) -> super::backend::PlayerDiagnostics {
        self.backend
            .as_ref()
            .map(|b| b.get_diagnostics())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::super::subtitles::SubtitleCue;
    use super::*;

    #[test]
    fn delivering_all_audio_does_not_complete_translation_progress() {
        let mut controller = MediaController {
            backend: None,
            ..Default::default()
        };
        controller.recognize_duration = Some(std::time::Duration::from_secs(100));
        controller.recognition_progress = Some(1.0);
        assert_eq!(controller.translation_progress(), Some(0.0));
        controller.subtitles.add_cue_with_metadata(
            SubtitleCue {
                id: "first".into(),
                start_ms: 0,
                end_ms: 30_000,
                original_text: "source".into(),
                translated_text: Some("translation".into()),
                speaker_name: None,
            },
            Default::default(),
        );
        assert_eq!(controller.translation_progress(), Some(0.3));
        controller.subtitles.add_cue_with_metadata(
            SubtitleCue {
                id: "last".into(),
                start_ms: 30_000,
                end_ms: 100_000,
                original_text: "source".into(),
                translated_text: Some("translation".into()),
                speaker_name: None,
            },
            Default::default(),
        );
        assert_eq!(controller.translation_progress(), Some(0.99));
    }
}
