use super::store::{MeetingBundle, Segment, SegmentSource};
use crate::audio_playback::{AudioPlayer, PlaybackStatus};
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RecordingClip {
    pub run_id: String,
    pub source: SegmentSource,
    pub path: PathBuf,
    pub offset_ms: Option<i64>,
}

impl RecordingClip {
    fn matches(&self, segment: &Segment) -> bool {
        self.offset_ms.is_some()
            && self.run_id == segment.recognition_run_id
            && self.source == segment.source
    }
}

#[derive(Default)]
pub(super) struct MeetingPlayback {
    pub clips: Vec<RecordingClip>,
    pub selected: usize,
    pub follow: bool,
    pub last_followed: Option<String>,
    meeting_id: String,
    loaded_path: Option<PathBuf>,
    player: Option<AudioPlayer>,
}

impl MeetingPlayback {
    pub fn refresh(&mut self, bundle: &MeetingBundle, prepare: bool) -> Result<(), String> {
        if self.meeting_id != bundle.meeting.id {
            *self = Self {
                meeting_id: bundle.meeting.id.clone(),
                follow: true,
                ..Default::default()
            };
        }
        let previous = self.clips.get(self.selected).cloned();
        self.clips = recording_clips(bundle);
        self.selected = previous
            .as_ref()
            .and_then(|clip| self.clips.iter().position(|candidate| candidate == clip))
            .unwrap_or(0);
        if previous.is_some() && self.clips.get(self.selected) != previous.as_ref() {
            self.unload();
        }
        if prepare && !self.clips.is_empty() && !self.loaded() {
            self.load()?;
        }
        Ok(())
    }

    pub fn unload(&mut self) {
        self.player = None;
        self.loaded_path = None;
        self.last_followed = None;
    }

    pub fn pause(&mut self) {
        if self.playing() {
            self.player.as_mut().unwrap().pause();
        }
    }

    fn load(&mut self) -> Result<(), String> {
        let path = self
            .clips
            .get(self.selected)
            .ok_or("The retained audio file is no longer available")?
            .path
            .clone();
        if self.loaded_path.as_ref() != Some(&path) {
            self.unload();
            let mut player = AudioPlayer::new()?;
            player.load_local_file(path.clone())?;
            self.loaded_path = Some(path);
            self.player = Some(player);
        }
        Ok(())
    }

    pub fn select(&mut self, index: usize) -> Result<(), String> {
        self.pause();
        self.selected = index;
        self.follow = true;
        self.last_followed = None;
        self.load()
    }

    pub fn toggle(&mut self) -> Result<(), String> {
        self.load()?;
        let player = self.player.as_mut().unwrap();
        if player.get_status() == PlaybackStatus::Playing {
            player.pause();
        } else {
            player.play();
            self.follow = true;
            self.last_followed = None;
        }
        Ok(())
    }

    pub fn seek(&mut self, ms: i64) {
        if let Some(player) = &mut self.player {
            player.seek(ms);
        }
        self.follow = true;
        self.last_followed = None;
    }

    pub fn can_seek_segment(&self, segment: &Segment) -> bool {
        self.clips.iter().any(|clip| clip.matches(segment))
    }

    pub fn seek_segment(&mut self, segment: &Segment) -> Result<(), String> {
        let Some(index) = self.clips.iter().position(|clip| clip.matches(segment)) else {
            return Ok(());
        };
        let resume = self.playing();
        self.select(index)?;
        let position = segment
            .start_ms
            .saturating_sub(self.clips[index].offset_ms.unwrap());
        self.seek(position.max(0));
        if resume {
            self.player.as_mut().unwrap().play();
        }
        Ok(())
    }

    pub fn playing(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|player| player.get_status() == PlaybackStatus::Playing)
    }

    pub fn loaded(&self) -> bool {
        self.player.is_some()
    }
    pub fn position(&self) -> i64 {
        self.player.as_ref().map_or(0, AudioPlayer::get_time_ms)
    }
    pub fn duration(&self) -> i64 {
        self.player.as_ref().map_or(0, AudioPlayer::get_duration_ms)
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.player.as_mut().and_then(AudioPlayer::take_error)
    }

    pub fn current_segment<'a>(&self, segments: &'a [Segment]) -> Option<&'a Segment> {
        if !self.loaded() {
            return None;
        }
        let clip = self.clips.get(self.selected)?;
        let position = self.position().saturating_add(clip.offset_ms?);
        segments
            .iter()
            .filter(|segment| clip.matches(segment))
            .filter(|segment| segment.start_ms <= position && position < segment.end_ms)
            .max_by_key(|segment| segment.start_ms)
    }
}

fn recording_clips(bundle: &MeetingBundle) -> Vec<RecordingClip> {
    let mut clips = Vec::new();
    for run in &bundle.audio_runs {
        if let Some(path) = &run.audio_source_path {
            let path = PathBuf::from(path);
            if path.is_file() {
                clips.push(RecordingClip {
                    run_id: run.recognition_run_id.clone(),
                    source: SegmentSource::ImportedAudio,
                    path,
                    offset_ms: Some(run.timeline_offset_ms),
                });
            }
        }
    }
    if let Some(root) = &bundle.meeting.recording_path {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                let run_id = entry.file_name().to_string_lossy().into_owned();
                let offset_ms = bundle
                    .audio_runs
                    .iter()
                    .find(|run| run.recognition_run_id == run_id)
                    .map(|run| run.timeline_offset_ms);
                for (source, name) in [
                    (SegmentSource::Microphone, "microphone.wav"),
                    (SegmentSource::SystemAudio, "system-audio.wav"),
                ] {
                    let path = entry.path().join(name);
                    if has_recording(&path) {
                        clips.push(RecordingClip {
                            run_id: run_id.clone(),
                            source,
                            path,
                            offset_ms,
                        });
                    }
                }
            }
        }
    }
    // Imported files remain playable even when the meeting predates audio-run metadata.
    if let Some(path) = bundle.meeting.audio_source_path.as_ref().map(PathBuf::from) {
        if path.is_file() && !clips.iter().any(|clip| clip.path == path) {
            clips.push(RecordingClip {
                run_id: String::new(),
                source: SegmentSource::ImportedAudio,
                path,
                offset_ms: None,
            });
        }
    }
    clips.sort_by(|a, b| {
        a.offset_ms
            .cmp(&b.offset_ms)
            .then_with(|| a.run_id.cmp(&b.run_id))
            .then_with(|| a.source.as_str().cmp(b.source.as_str()))
    });
    clips
}

fn has_recording(path: &Path) -> bool {
    // Saved tracks use a 44-byte PCM WAV header; empty source tracks are omitted.
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 44)
}
