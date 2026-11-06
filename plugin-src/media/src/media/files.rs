use std::path::{Path, PathBuf};

use super::{
    MediaAction,
    backend::MediaSource,
    controller::MediaController,
    subtitles::SubtitleTimeline,
    task::{MediaType, SubtitleMode, detect_media_type},
};

pub(super) enum SubtitleChange {
    Import {
        path: PathBuf,
        timeline: SubtitleTimeline,
    },
    Action(MediaAction),
}

pub(super) fn read_subtitles(path: &Path) -> Result<SubtitleTimeline, String> {
    SubtitleTimeline::parse(&std::fs::read_to_string(path).map_err(|error| error.to_string())?)
}

impl MediaController {
    pub(super) fn open_files(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        let (media, subtitles): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .partition(|path| detect_media_type(path) != MediaType::Subtitles);
        if media.len() > 1 || subtitles.len() > 1 || media.is_empty() && subtitles.is_empty() {
            return Err("Choose one media file and one subtitle file.".into());
        }
        let imported = subtitles
            .first()
            .map(|path| read_subtitles(path))
            .transpose()?;
        let source = media.first().or(subtitles.first()).unwrap();
        self.open_create();
        self.draft_source = source.to_string_lossy().into_owned();
        self.start_draft_task()?;
        if !media.is_empty()
            && let (Some(path), Some(timeline)) = (subtitles.into_iter().next(), imported)
        {
            self.apply_subtitle_change(SubtitleChange::Import { path, timeline });
        }
        Ok(())
    }

    pub(super) fn can_change_files(&self) -> bool {
        self.subtitle_edit.is_none()
            && self.pending_subtitle_change.is_none()
            && self.video_export.is_none()
            && self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get(id))
                .is_some_and(|task| !task.is_task_running)
    }

    pub(super) fn import_subtitles(&mut self, path: PathBuf) -> Result<(), String> {
        if !self.can_change_files() {
            return Err("Finish the current task or edit first.".into());
        }
        let change = SubtitleChange::Import {
            timeline: read_subtitles(&path)?,
            path,
        };
        if self.subtitles.count() == 0 {
            self.apply_subtitle_change(change);
        } else {
            self.pending_subtitle_change = Some(change);
        }
        Ok(())
    }

    pub(super) fn apply_subtitle_change(&mut self, change: SubtitleChange) -> MediaAction {
        match change {
            SubtitleChange::Action(action) => action,
            SubtitleChange::Import { path, timeline } => {
                if let Some(task) = self
                    .active_task_id
                    .as_deref()
                    .and_then(|id| self.store.get_mut(id))
                {
                    task.subtitle_mode = SubtitleMode::ImportedSrt(path.clone());
                    if task.media_type == MediaType::Subtitles {
                        task.source = MediaSource::LocalFile(path.clone());
                        task.title = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        task.duration_ms = timeline
                            .cues()
                            .iter()
                            .map(|cue| cue.end_ms)
                            .max()
                            .unwrap_or(0);
                        self.current_source = Some(task.source.clone());
                    }
                }
                self.subtitles.replace(timeline);
                self.previews = Default::default();
                self.last_manual_scroll = None;
                self.last_auto_scrolled_cue_id = None;
                self.error = None;
                self.save_active();
                MediaAction::None
            }
        }
    }

    pub(super) fn attach_media(&mut self, path: PathBuf) -> Result<(), String> {
        if !self.is_subtitle_task() || !self.can_change_files() {
            return Err("Finish the current task or edit first.".into());
        }
        if !path.is_file() || detect_media_type(&path) == MediaType::Subtitles {
            return Err("Choose a video or audio file.".into());
        }
        let id = self.active_task_id.clone().ok_or("Task not found")?;
        let task = self.store.get_mut(&id).ok_or("Task not found")?;
        task.media_type = detect_media_type(&path);
        task.title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        task.source = MediaSource::LocalFile(path);
        task.subtitle_mode = SubtitleMode::TranslateExisting;
        task.duration_ms = 0;
        self.save_active();
        self.open_task(&id)
    }
}
