use super::{
    backend::{MediaBackend, MediaSource, PlaybackStatus, window::NativeVideoHost},
    subtitles::{SubtitleCue, SubtitleMetadata, SubtitleTimeline},
    task::{AudioChannelItem, VideoSubtitleMode, VideoTask, VideoTaskStore},
};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum VideoPlayerRoute {
    #[default]
    Library,
    Create,
    Player,
}

pub struct VideoPlayerController {
    pub route: VideoPlayerRoute,
    pub store: VideoTaskStore,
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
    pub draft_source: String,
    pub draft_source_lang: String,
    pub draft_target_lang: String,
    pub draft_subtitle_mode: VideoSubtitleMode,
    pub draft_recognition: crate::client_settings::RecognitionSettings,

    pub last_hover_instant: Option<Instant>,
    pub last_save_instant: Instant,
    pub last_manual_scroll: Option<Instant>,
    pub last_auto_scrolled_cue_id: Option<String>,
    pub timeline_viewport_height: Option<f32>,
    pub mpv_installer: super::installer::MpvInstaller,

    pub is_extracting: bool,
    pub extraction_progress: Option<f32>,
    pub extract_position: Option<std::time::Duration>,
    pub extract_duration: Option<std::time::Duration>,
    pub recognition_progress: Option<f32>,
    pub recognize_position: Option<std::time::Duration>,
    pub recognize_duration: Option<std::time::Duration>,
}

impl Default for VideoPlayerController {
    fn default() -> Self {
        let storage_dir = PathBuf::from("runtime");
        let store = VideoTaskStore::load_from_dir(&storage_dir);
        let backend: Option<Box<dyn MediaBackend>> =
            if !crate::plugins::PluginId::VIDEO_PLAYER.is_supported() {
                None
            } else {
                match super::backend::mpv::MpvBackend::new() {
                    Ok(b) => Some(Box::new(b)),
                    Err(e) => {
                        log::warn!("MPV backend not initialized on startup: {}", e);
                        None
                    }
                }
            };

        Self {
            route: VideoPlayerRoute::Library,
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
            draft_source: String::new(),
            draft_source_lang: "auto".into(),
            draft_target_lang: "zh".into(),
            draft_subtitle_mode: VideoSubtitleMode::RealtimeTranslation,
            draft_recognition: crate::client_settings::RecognitionSettings {
                background_noise: 0.6,
                pause_tolerance: 1.0,
                continuous_recognition: false,
            },
            last_hover_instant: Some(Instant::now()),
            last_save_instant: Instant::now(),
            last_manual_scroll: None,
            last_auto_scrolled_cue_id: None,
            timeline_viewport_height: None,
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

impl VideoPlayerController {
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
        let _ = self.store.save_to_dir(&self.storage_dir);
        self.route = VideoPlayerRoute::Library;
        self.active_task_id = None;
        self.current_source = None;
        self.last_auto_scrolled_cue_id = None;
        self.release_native_host();
    }

    pub fn open_create(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.stop();
        }
        let _ = self.store.save_to_dir(&self.storage_dir);
        self.route = VideoPlayerRoute::Create;
        self.active_task_id = None;
        self.current_source = None;
        self.last_auto_scrolled_cue_id = None;
        self.draft_title.clear();
        self.draft_source.clear();
        self.draft_source_lang = "auto".into();
        self.draft_target_lang = "zh".into();
        self.draft_subtitle_mode = VideoSubtitleMode::RealtimeTranslation;
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
                .last()
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

        let mut task = VideoTask::new_with_media_type(
            title,
            source.clone(),
            media_type,
            self.draft_source_lang.clone(),
            self.draft_target_lang.clone(),
            self.draft_subtitle_mode.clone(),
            self.draft_recognition.clone(),
        );

        if let VideoSubtitleMode::ImportedSrt(srt_path) = &self.draft_subtitle_mode {
            if let Ok(content) = std::fs::read_to_string(srt_path) {
                task.subtitles = parse_srt_to_timeline(&content);
            }
        }

        let task_id = task.id.clone();
        self.store.add_or_update(task);
        let _ = self.store.save_to_dir(&self.storage_dir);

        self.play_task(&task_id)?;
        Ok(task_id)
    }

    pub fn play_task(&mut self, task_id: &str) -> Result<(), String> {
        let task = self.store.get(task_id).ok_or("Task not found")?.clone();
        self.active_task_id = Some(task_id.to_string());
        self.current_source = Some(task.source.clone());
        self.subtitles = task.subtitles.clone();
        self.route = VideoPlayerRoute::Player;
        self.error = None;

        if task.media_type == super::task::MediaType::AudioOnly {
            self.release_native_host();
        }

        if let Some(backend) = &mut self.backend {
            backend.set_audio_only_mode(task.media_type == super::task::MediaType::AudioOnly);
            match &task.source {
                MediaSource::LocalFile(path) => {
                    backend.load_local_file(path.clone())?;
                }
                MediaSource::NetworkStream(url) => {
                    backend.load_stream_url(url.clone())?;
                }
            }
            backend.set_channel_routing(&task.audio_channels);
        } else {
            return Err("Media backend is not available".into());
        }

        if let Some(t) = self.store.get_mut(task_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            t.last_played_sec = now;
        }
        let _ = self.store.save_to_dir(&self.storage_dir);
        Ok(())
    }

    pub fn is_audio_only_task(&self) -> bool {
        self.active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .map_or(false, |task| {
                task.media_type == super::task::MediaType::AudioOnly
            })
    }

    pub fn start_task(&mut self) {
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get_mut(task_id) {
                task.is_task_running = true;
            }
            let _ = self.store.save_to_dir(&self.storage_dir);
        }
    }

    pub fn pause_task(&mut self) {
        if let Some(task_id) = &self.active_task_id {
            if let Some(task) = self.store.get_mut(task_id) {
                task.is_task_running = false;
            }
            let _ = self.store.save_to_dir(&self.storage_dir);
        }
    }

    pub fn clear_and_restart_task(&mut self) {
        self.subtitles.clear();
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
            let _ = self.store.save_to_dir(&self.storage_dir);
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
            let _ = self.store.save_to_dir(&self.storage_dir);
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
            self.route = VideoPlayerRoute::Library;
        }
        self.store.delete(task_id);
        let _ = self.store.save_to_dir(&self.storage_dir);
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

    pub fn note_mouse_motion(&mut self) {
        let should_trigger = self
            .last_hover_instant
            .map_or(true, |inst| inst.elapsed().as_secs_f32() > 2.5);
        self.last_hover_instant = Some(Instant::now());
        if should_trigger {
            if let Some(src) = &self.current_source {
                if let Some(backend) = &mut self.backend {
                    backend.show_osd_title(&src.display_title());
                }
            }
        }
    }

    pub fn try_init_backend(&mut self) -> bool {
        if self.backend.is_some() {
            return true;
        }
        match super::backend::mpv::MpvBackend::new() {
            Ok(b) => {
                self.backend = Some(Box::new(b));
                self.error = None;
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
        if let Some(res) = self.mpv_installer.poll() {
            if res.is_ok() {
                self.try_init_backend();
            }
        }

        let is_audio_only = self.is_audio_only_task();
        let show_subs = self.show_subtitles && !is_audio_only && self.current_source.is_some();

        if let Some(backend) = &mut self.backend {
            backend.tick();

            // Synchronize detected audio channels from backend stream
            if let Some(active_id) = &self.active_task_id {
                if let Some(detected_count) = backend.get_audio_channel_count() {
                    if detected_count > 0 {
                        if let Some(task) = self.store.get_mut(active_id) {
                            if task.audio_channels.len() != detected_count {
                                task.audio_channels =
                                    AudioChannelItem::default_for_count(detected_count);
                                backend.set_channel_routing(&task.audio_channels);
                            }
                        }
                    }
                }
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

        if let Some(active_id) = &self.active_task_id {
            let dur = self.get_duration_ms();
            if dur > 0 {
                if let Some(task) = self.store.get_mut(active_id) {
                    task.duration_ms = dur;
                    task.subtitles = self.subtitles.clone();
                }
                if self.last_save_instant.elapsed().as_secs() >= 5 {
                    self.last_save_instant = Instant::now();
                    let _ = self.store.save_to_dir(&self.storage_dir);
                }
            }
        }
    }

    pub fn get_time_ms(&self) -> i64 {
        self.backend.as_ref().map(|b| b.get_time_ms()).unwrap_or(0)
    }

    pub fn get_duration_ms(&self) -> i64 {
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

fn parse_srt_to_timeline(srt_content: &str) -> SubtitleTimeline {
    let mut timeline = SubtitleTimeline::new();
    let normalized = srt_content.replace("\r\n", "\n");
    let blocks = normalized.split("\n\n");
    for (idx, block) in blocks.enumerate() {
        let lines: Vec<&str> = block
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() >= 2 {
            let time_line = if lines[0].contains("-->") {
                lines[0]
            } else {
                lines[1]
            };
            let text_start = if lines[0].contains("-->") { 1 } else { 2 };
            let times: Vec<&str> = time_line.split("-->").collect();
            if times.len() == 2 && text_start < lines.len() {
                let start_ms = parse_srt_time(times[0].trim());
                let end_ms = parse_srt_time(times[1].trim());
                let text_lines = &lines[text_start..];

                let mut speaker_name = None;
                let mut first_line = text_lines[0].to_string();

                if first_line.starts_with('[') {
                    if let Some(close_bracket) = first_line.find(']') {
                        let speaker = first_line[1..close_bracket].trim().to_string();
                        if !speaker.is_empty() {
                            speaker_name = Some(speaker);
                            first_line = first_line[close_bracket + 1..].trim().to_string();
                        }
                    }
                }

                let (original_text, translated_text) = if text_lines.len() >= 2 {
                    (first_line, Some(text_lines[1].to_string()))
                } else {
                    (first_line, None)
                };

                timeline.add_cue_with_metadata(
                    SubtitleCue {
                        id: format!("srt_{}", idx),
                        start_ms,
                        end_ms,
                        speaker_name,
                        original_text,
                        translated_text,
                    },
                    SubtitleMetadata::authored(),
                );
            }
        }
    }
    timeline
}

fn parse_srt_time(time_str: &str) -> i64 {
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() == 3 {
        let h: f64 = parts[0].parse().unwrap_or(0.0);
        let m: f64 = parts[1].parse().unwrap_or(0.0);
        let s_part = parts[2].replace(',', ".");
        let s: f64 = s_part.parse().unwrap_or(0.0);
        ((h * 3600.0 + m * 60.0 + s) * 1000.0) as i64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_keeps_translation_cues_in_its_timeline() {
        let mut controller = VideoPlayerController::default();
        assert_eq!(controller.volume, 1.0);

        let task = VideoTask::new(
            "Test Video".into(),
            MediaSource::NetworkStream("http://test.com".into()),
            "en".into(),
            "zh".into(),
            VideoSubtitleMode::RealtimeTranslation,
            crate::client_settings::RecognitionSettings::default(),
        );
        let task_id = task.id.clone();
        controller.store.add_or_update(task);
        controller.active_task_id = Some(task_id);

        controller.subtitles.add_cue_with_metadata(
            SubtitleCue {
                id: "test_1".into(),
                start_ms: 500,
                end_ms: 2500,
                speaker_name: Some("Speaker".into()),
                original_text: "Hello".into(),
                translated_text: Some("你好".into()),
            },
            SubtitleMetadata::default(),
        );
        assert_eq!(controller.subtitles.cues().len(), 1);
    }

    #[test]
    fn test_parse_srt() {
        let srt = "1\n00:00:01,000 --> 00:00:03,500\nHello World\n\n2\n00:00:04,000 --> 00:00:06,000\nSecond line";
        let tl = parse_srt_to_timeline(srt);
        assert_eq!(tl.count(), 2);
        assert_eq!(tl.cues()[0].start_ms, 1000);
        assert_eq!(tl.cues()[0].end_ms, 3500);
    }

    #[test]
    fn test_controller_lifecycle_cleanup() {
        let mut controller = VideoPlayerController::default();
        let task = VideoTask::new(
            "Test Video".into(),
            MediaSource::LocalFile(PathBuf::from("test.mp4")),
            "ja".into(),
            "zh".into(),
            VideoSubtitleMode::RealtimeTranslation,
            crate::client_settings::RecognitionSettings {
                background_noise: 0.6,
                pause_tolerance: 1.0,
                continuous_recognition: false,
            },
        );
        let task_id = task.id.clone();
        controller.store.add_or_update(task);
        controller.active_task_id = Some(task_id.clone());
        controller.current_source = Some(MediaSource::LocalFile(PathBuf::from("test.mp4")));
        controller.route = VideoPlayerRoute::Player;

        // Returning to library should clear active task and source
        controller.open_library();
        assert_eq!(controller.route, VideoPlayerRoute::Library);
        assert!(controller.active_task_id.is_none());
        assert!(controller.current_source.is_none());

        // Re-activating and deleting task
        controller.active_task_id = Some(task_id.clone());
        controller.current_source = Some(MediaSource::LocalFile(PathBuf::from("test.mp4")));
        controller.delete_task(&task_id);
        assert!(controller.active_task_id.is_none());
        assert!(controller.current_source.is_none());
        assert!(controller.store.get(&task_id).is_none());
    }

    #[test]
    fn test_toggle_play_replay_when_stopped() {
        let mut controller = VideoPlayerController::default();
        controller.current_source = Some(MediaSource::LocalFile(PathBuf::from("test.mp4")));
        controller.toggle_play();
    }
}
