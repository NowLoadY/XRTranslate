use super::{
    backend::{MediaBackend, MediaSource, PlaybackStatus},
    subtitles::{SubtitleCue, SubtitleTimeline},
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SubtitleField {
    Start,
    End,
    Original,
    Translation,
}

#[derive(Clone)]
pub(super) struct SubtitleEdit {
    pub(super) task_id: String,
    pub(super) cue: SubtitleCue,
    pub(super) focus: Option<SubtitleField>,
    pub(super) retranslate_requested: bool,
    pub(super) retranslation: Option<String>,
}

pub struct MediaController {
    pub route: MediaRoute,
    pub store: MediaTaskStore,
    pub storage_dir: PathBuf,
    pub active_task_id: Option<String>,
    pub current_source: Option<MediaSource>,
    pub backend: Option<Box<dyn MediaBackend>>,
    pub render_state: Option<eframe::egui_wgpu::RenderState>,
    pub render_context: Option<eframe::egui::Context>,
    pub video_display: Option<super::backend::VideoDisplay>,
    pub subtitles: SubtitleTimeline,
    pub(super) subtitle_edit: Option<SubtitleEdit>,
    pub previews: crate::session_coordinator::LivePreviews,
    pub show_subtitles: bool,
    pub fullscreen_mode: bool,
    pub fullscreen_last_interaction: Instant,
    pub error: Option<String>,
    pub muted: bool,
    pub volume: f32,

    pub search_query: String,
    pub browse_all: bool,
    pub draft_title: String,
    pub draft_source: String,
    pub draft_source_lang: String,
    pub draft_target_lang: String,
    pub draft_subtitle_mode: SubtitleMode,
    pub draft_recognition: crate::client_settings::RecognitionSettings,

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
            render_state: None,
            render_context: None,
            video_display: None,
            subtitles: SubtitleTimeline::new(),
            subtitle_edit: None,
            previews: Default::default(),
            show_subtitles: true,
            fullscreen_mode: false,
            fullscreen_last_interaction: Instant::now(),
            error: None,
            muted: false,
            volume: 1.0,
            search_query: String::new(),
            browse_all: false,
            draft_title: String::new(),
            draft_source: String::new(),
            draft_source_lang: "auto".into(),
            draft_target_lang: "zh".into(),
            draft_subtitle_mode: SubtitleMode::RealtimeTranslation,
            draft_recognition: crate::client_settings::RecognitionSettings {
                background_noise: 0.6,
                pause_tolerance: 1.0,
                continuous_recognition: false,
            },
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
    #[cfg(target_os = "android")]
    pub(crate) fn handle_back(
        &mut self,
        ctx: &eframe::egui::Context,
    ) -> Option<super::MediaAction> {
        if self.fullscreen_mode {
            self.fullscreen_mode = false;
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Fullscreen(false));
            return Some(super::MediaAction::None);
        }
        if self.subtitle_edit.take().is_some() {
            return Some(super::MediaAction::None);
        }
        if self.route != MediaRoute::Library {
            self.open_library();
            return Some(super::MediaAction::StopTranslation);
        }
        None
    }

    pub fn hide_video(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.set_video_visible(false);
        }
    }

    pub fn open_library(&mut self) {
        self.subtitle_edit = None;
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        self.pause_task();
        self.route = MediaRoute::Library;
        self.previews = Default::default();
        self.active_task_id = None;
        self.current_source = None;
        self.last_auto_scrolled_cue_id = None;
        self.hide_video();
    }

    pub fn open_create(&mut self) {
        self.subtitle_edit = None;
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        self.pause_task();
        self.route = MediaRoute::Create;
        self.previews = Default::default();
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
        self.hide_video();
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
        self.subtitle_edit = None;
        self.previews = Default::default();
        self.active_task_id = Some(task_id.to_string());
        self.current_source = Some(task.source.clone());
        self.subtitles = task.subtitles.clone();
        self.hide_video();
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

        if task.media_type != super::task::MediaType::Subtitles
            && let Some(backend) = &mut self.backend
        {
            if task.media_type == super::task::MediaType::Video
                && cfg!(any(windows, target_os = "linux"))
                && backend.supports_video()
            {
                let initialized = match (&self.render_state, &self.render_context) {
                    (Some(state), Some(ctx)) => {
                        backend.initialize_video(state, ctx, self.video_display)
                    }
                    _ => Err("GPU video renderer is unavailable".into()),
                };
                if let Err(error) = initialized {
                    self.error = Some(error);
                    self.video_unavailable = true;
                }
            }
            backend.set_audio_only_mode(
                task.media_type == super::task::MediaType::AudioOnly
                    || !cfg!(any(windows, target_os = "linux"))
                    || self.video_unavailable,
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
        cfg!(any(windows, target_os = "linux"))
            && self
                .backend
                .as_ref()
                .is_some_and(|backend| backend.supports_video())
            && !self.video_unavailable
            && self.render_state.is_some()
            && self.can_play()
            && !self.is_audio_only_task()
    }

    pub fn can_play(&self) -> bool {
        self.playback_loaded && !self.is_subtitle_task()
    }

    pub(super) fn subtitle_time_bounds(&self, index: usize) -> std::ops::RangeInclusive<i64> {
        let bounds = self.subtitles.time_bounds(index);
        if index + 1 != self.subtitles.count() || self.is_subtitle_task() {
            return bounds;
        }
        let duration = self.get_duration_ms();
        let duration = if duration > 0 {
            duration
        } else {
            self.active_task_id
                .as_deref()
                .and_then(|id| self.store.get(id))
                .map_or(0, |task| task.duration_ms)
        };
        if duration > 0 {
            *bounds.start()..=duration.max(*bounds.start())
        } else {
            bounds
        }
    }

    pub(super) fn begin_subtitle_edit(&mut self, cue_id: &str, field: SubtitleField) {
        if self.subtitle_edit.is_some() {
            return;
        }
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .filter(|task| !task.is_task_running)
        else {
            return;
        };
        let Some(index) = self
            .subtitles
            .cues()
            .iter()
            .position(|cue| cue.id == cue_id)
        else {
            return;
        };
        let mut cue = self.subtitles.cues()[index].clone();
        cue.end_ms = self.subtitles.effective_end_at(index);
        self.subtitle_edit = Some(SubtitleEdit {
            task_id: task.id.clone(),
            cue,
            focus: Some(field),
            retranslate_requested: false,
            retranslation: None,
        });
        if let Some(backend) = &mut self.backend
            && backend.get_status() == PlaybackStatus::Playing
        {
            backend.pause();
        }
    }

    pub(super) fn save_subtitle_edit(&mut self) {
        let Some(mut edit) = self.subtitle_edit.clone() else {
            return;
        };
        if edit.retranslation.is_some() {
            return;
        }
        if self.active_task_id.as_deref() != Some(edit.task_id.as_str())
            || self
                .store
                .get(&edit.task_id)
                .is_none_or(|task| task.is_task_running)
        {
            return;
        }
        // Blank lines delimit cues in SRT/VTT; keep line breaks within one cue.
        let clean = |text: &str| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let original = clean(&edit.cue.original_text);
        if original.is_empty() {
            self.error = Some("Subtitle text cannot be empty".into());
            return;
        }
        let translated = clean(edit.cue.translated_text.as_deref().unwrap_or_default());
        edit.cue.original_text = original;
        edit.cue.translated_text = (!translated.is_empty()).then_some(translated);
        if let Some(index) = self
            .subtitles
            .cues()
            .iter()
            .position(|cue| cue.id == edit.cue.id)
            && (edit.cue.start_ms != self.subtitles.cues()[index].start_ms
                || edit.cue.end_ms != self.subtitles.effective_end_at(index))
        {
            let bounds = self.subtitle_time_bounds(index);
            edit.cue.start_ms = edit.cue.start_ms.clamp(*bounds.start(), *bounds.end());
            edit.cue.end_ms = edit.cue.end_ms.clamp(edit.cue.start_ms, *bounds.end());
        }
        let Some(previous) = self.subtitles.edit_cue(edit.cue) else {
            self.error = Some("Subtitle not found".into());
            return;
        };
        let was_dirty = self.dirty;
        self.error = None;
        self.dirty = true;
        self.save_active();
        if self.dirty {
            self.subtitles.restore_cue(previous.0.clone(), previous.1);
            if let Some(task) = self.store.get_mut(&edit.task_id) {
                task.subtitles.restore_cue(previous.0, previous.1);
            }
            self.dirty = was_dirty;
        } else {
            self.subtitle_edit = None;
        }
    }

    pub(super) fn merge_subtitles(&mut self, source_id: &str, target_id: &str) -> bool {
        if self.subtitle_edit.is_some()
            || self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get(id))
                .is_none_or(|task| task.is_task_running)
            || !self.subtitles.merge_adjacent(source_id, target_id)
        {
            return false;
        }
        self.error = None;
        self.dirty = true;
        self.save_active();
        true
    }

    pub(super) fn delete_subtitle(&mut self, cue_id: &str) -> bool {
        if self.subtitle_edit.is_some()
            || self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get(id))
                .is_none_or(|task| task.is_task_running)
        {
            return false;
        }
        let previous = self.subtitles.clone();
        if !self.subtitles.remove_cue(cue_id) {
            return false;
        }
        let was_dirty = self.dirty;
        self.error = None;
        self.dirty = true;
        self.save_active();
        if self.dirty {
            self.subtitles = previous;
            if let Some(task) = self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get_mut(id))
            {
                task.subtitles = self.subtitles.clone();
            }
            self.dirty = was_dirty;
            return false;
        }
        self.last_auto_scrolled_cue_id = None;
        self.last_manual_scroll = Some(Instant::now());
        true
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
        self.subtitle_edit = None;
        if !self.is_subtitle_task() {
            self.subtitles.clear();
        }
        self.previews = Default::default();
        self.error = None;
        self.is_extracting = false;
        self.extraction_progress = None;
        self.extract_position = None;
        self.extract_duration = None;
        self.recognition_progress = None;
        self.recognize_duration = None;
        self.recognize_position = None;
        self.last_manual_scroll = None;
        self.last_auto_scrolled_cue_id = None;
        if let Some(backend) = &mut self.backend {
            backend.set_osd_subtitle("");
        }
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
            self.hide_video();
            self.previews = Default::default();
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
        self.fullscreen_last_interaction = Instant::now();
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
                self.hide_video();
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
        let show_subs = self.show_subtitles && !is_audio_only && self.playback_loaded;

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

            if show_subs && backend.get_status() != PlaybackStatus::Stopped {
                let current_time_ms = backend.get_time_ms();
                if let Some(cue) = self.subtitles.active_cue_at(current_time_ms) {
                    backend.set_osd_subtitle(&cue.playback_text());
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
mod subtitle_edit_tests {
    use super::*;

    #[test]
    fn deletion_is_idle_only_and_persists_or_restores_the_whole_timeline() {
        let storage = tempfile::tempdir().unwrap();
        let mut controller = MediaController::default();
        controller.backend = None;
        controller.storage_dir = storage.path().to_owned();
        controller.store.tasks.clear();
        let mut task = MediaTask::new(
            "Test deletion".into(),
            MediaSource::LocalFile("test.srt".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            Default::default(),
        );
        task.subtitles = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nFirst.\n\n2\n00:00:01,000 --> 00:00:02,000\nSecond.\n\n3\n00:00:02,000 --> 00:00:03,000\nThird.\n",
        ).unwrap();
        task.is_task_running = true;
        controller.active_task_id = Some(task.id.clone());
        controller.subtitles = task.subtitles.clone();
        controller.store.add_or_update(task);
        controller.save_active();
        let before = serde_json::to_value(&controller.subtitles).unwrap();

        assert!(!controller.delete_subtitle("srt_1"));
        controller.store.tasks[0].is_task_running = false;
        controller.begin_subtitle_edit("srt_0", SubtitleField::Original);
        assert!(!controller.delete_subtitle("srt_1"));
        controller.subtitle_edit = None;
        let blocked = tempfile::NamedTempFile::new().unwrap();
        controller.storage_dir = blocked.path().to_owned();
        assert!(!controller.delete_subtitle("srt_1"));
        assert!(controller.error.is_some());
        assert_eq!(serde_json::to_value(&controller.subtitles).unwrap(), before);
        assert_eq!(
            serde_json::to_value(&controller.store.tasks[0].subtitles).unwrap(),
            before
        );

        controller.storage_dir = storage.path().to_owned();
        assert!(controller.delete_subtitle("srt_1"));
        assert!(controller.error.is_none());
        let saved = MediaTaskStore::load_from_dir(storage.path());
        assert_eq!(saved.tasks[0].subtitles.cues(), controller.subtitles.cues());
        assert_eq!(
            controller
                .subtitles
                .cues()
                .iter()
                .map(|cue| cue.original_text.as_str())
                .collect::<Vec<_>>(),
            ["First.", "Third."]
        );
        assert!(
            !std::fs::read_to_string(storage.path().join("video_tasks.json"))
                .unwrap()
                .contains("srt_1")
        );
        assert!(controller.delete_subtitle("srt_0"));
        assert!(controller.delete_subtitle("srt_2"));
        assert_eq!(
            MediaTaskStore::load_from_dir(storage.path()).tasks[0]
                .subtitles
                .count(),
            0
        );
    }

    #[test]
    fn merging_is_idle_only_and_immediately_replaces_persisted_subtitles() {
        let storage = tempfile::tempdir().unwrap();
        let mut controller = MediaController::default();
        controller.backend = None;
        controller.storage_dir = storage.path().to_owned();
        controller.store.tasks.clear();
        let mut task = MediaTask::new(
            "Test subtitles".into(),
            MediaSource::LocalFile("test.srt".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            Default::default(),
        );
        task.subtitles = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nFirst.\n\n2\n00:00:01,000 --> 00:00:02,000\nSecond.\n",
        )
        .unwrap();
        task.is_task_running = true;
        controller.active_task_id = Some(task.id.clone());
        controller.subtitles = task.subtitles.clone();
        controller.store.add_or_update(task);
        controller.save_active();

        assert!(!controller.merge_subtitles("srt_0", "srt_1"));
        controller.store.tasks[0].is_task_running = false;
        controller.begin_subtitle_edit("srt_0", SubtitleField::Original);
        assert!(!controller.merge_subtitles("srt_0", "srt_1"));
        controller.subtitle_edit = None;
        assert!(controller.merge_subtitles("srt_0", "srt_1"));
        assert!(controller.error.is_none());
        assert!(!controller.dirty);

        let saved = MediaTaskStore::load_from_dir(storage.path());
        assert_eq!(saved.tasks[0].subtitles.cues(), controller.subtitles.cues());
        assert_eq!(saved.tasks[0].subtitles.count(), 1);
        let cue = &saved.tasks[0].subtitles.cues()[0];
        assert_eq!(cue.id, "srt_1");
        assert_eq!(cue.original_text, "First. Second.");
        assert_eq!((cue.start_ms, cue.end_ms), (0, 2_000));
        let serialized = std::fs::read_to_string(storage.path().join("video_tasks.json")).unwrap();
        assert!(!serialized.contains("srt_0"));
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
    }
}
