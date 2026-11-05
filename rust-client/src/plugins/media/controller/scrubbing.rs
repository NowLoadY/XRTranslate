//! Timing drags share the subtitle editor's draft and commit path. Audio previews
//! use the existing player independently, so the video stays at the requested frame.
use super::*;
use std::time::Duration;

pub(super) struct ScrubAudio {
    player: Box<dyn MediaBackend>,
    pending: Option<i64>,
    sampled: Option<Instant>,
}

impl ScrubAudio {
    fn new(task: &MediaTask, volume: f32, muted: bool) -> Result<Self, String> {
        let mut player = super::super::backend::create()?;
        player.set_audio_only_mode(true);
        player.pause();
        player.set_volume(volume);
        player.set_mute(muted);
        match &task.source {
            MediaSource::LocalFile(path) => player.load_local_file(path.clone())?,
            MediaSource::NetworkStream(url) | MediaSource::OnlineFile(url) => {
                player.load_stream_url(url.clone())?;
            }
        }
        player.select_audio(task.playback_audio())?;
        player.set_channel_routing(if task.play_speech_track {
            &[]
        } else {
            &task.audio_channels
        });
        Ok(Self {
            player,
            pending: None,
            sampled: None,
        })
    }

    pub(super) fn tick(&mut self) {
        self.player.tick();
        if let Some(error) = self.player.take_error() {
            log::warn!("Timeline scrub audio failed: {error}");
            self.pending = None;
            self.player.pause();
        }
        // Coalesce pointer motion; do not queue a seek for every rendered frame.
        if self.pending.is_some()
            && self.player.get_duration_ms() > 0
            && self
                .sampled
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(80))
        {
            let time = self.pending.take().unwrap();
            self.player
                .seek(time.clamp(0, self.player.get_duration_ms().saturating_sub(1)));
            self.player.play();
            self.sampled = Some(Instant::now());
        } else if self
            .sampled
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(180))
        {
            self.player.pause();
            self.sampled = None;
        }
    }
}

impl MediaController {
    pub(in crate::plugins::media) fn subtitle_trim_bounds(
        &self,
        index: usize,
        edge: SubtitleField,
    ) -> std::ops::RangeInclusive<i64> {
        let cue = &self.subtitles.cues()[index];
        let bounds = self.subtitle_time_bounds(index);
        match edge {
            SubtitleField::Start => {
                let start = index
                    .checked_sub(1)
                    .map_or(0, |previous| self.subtitles.effective_end_at(previous));
                start
                    ..=self
                        .subtitles
                        .effective_end_at(index)
                        .saturating_sub(1)
                        .max(start)
            }
            _ => {
                cue.start_ms.saturating_add(1)..=(*bounds.end()).max(cue.start_ms.saturating_add(1))
            }
        }
    }

    pub(in crate::plugins::media) fn trim_subtitle(
        &mut self,
        cue: &str,
        edge: SubtitleField,
        time: i64,
        finished: bool,
    ) {
        let Some(index) = self.subtitles.cues().iter().position(|item| item.id == cue) else {
            return;
        };
        let bounds = self.subtitle_trim_bounds(index, edge);
        if self.subtitle_edit.is_none() {
            self.begin_subtitle_edit(cue, edge);
            let Some(edit) = &mut self.subtitle_edit else {
                return;
            };
            edit.dragging_time = true;
            if !finished && self.can_play() {
                let task = self
                    .store
                    .get(self.active_task_id.as_deref().unwrap())
                    .unwrap();
                match ScrubAudio::new(task, self.volume, self.muted) {
                    Ok(audio) => self.scrub_audio = Some(audio),
                    Err(error) => log::warn!("Subtitle scrub audio unavailable: {error}"),
                }
            }
        }
        let Some(edit) = self
            .subtitle_edit
            .as_mut()
            .filter(|edit| edit.dragging_time && edit.cue.id == cue)
        else {
            return;
        };
        let time = time.clamp(*bounds.start(), *bounds.end());
        match edge {
            SubtitleField::Start => edit.cue.start_ms = time,
            SubtitleField::End => edit.cue.end_ms = time,
            _ => return,
        }
        self.seek_to(time, false);
        if let Some(audio) = &mut self.scrub_audio {
            audio.pending = Some(time);
        }
        if finished {
            self.scrub_audio = None;
            self.save_subtitle_edit();
            // A failed save remains available in the normal editor for retry.
            if let Some(edit) = &mut self.subtitle_edit {
                edit.dragging_time = false;
            }
        }
    }

    pub(in crate::plugins::media) fn speech_move_bounds(
        &self,
        clip: &super::super::task::SpeechClip,
    ) -> std::ops::RangeInclusive<i64> {
        0..=self
            .get_duration_ms()
            .max(
                self.active_task_id
                    .as_deref()
                    .and_then(|id| self.store.get(id))
                    .map_or(0, |task| task.duration_ms),
            )
            .max(clip.end_ms())
            .saturating_sub(clip.duration_ms)
            .max(0)
    }

    pub(in crate::plugins::media) fn move_speech(
        &mut self,
        id: &str,
        start: i64,
        position: i64,
        finished: bool,
    ) {
        let Some(task_id) = self.active_task_id.clone() else {
            return;
        };
        let Some(task) = self.store.get(&task_id) else {
            return;
        };
        let Some(clip) = task
            .speech_track
            .as_ref()
            .and_then(|track| track.clips.iter().find(|clip| clip.id == id))
        else {
            return;
        };
        let bounds = self.speech_move_bounds(clip);
        let grab = position.saturating_sub(start).clamp(0, clip.duration_ms);
        let start = start.clamp(*bounds.start(), *bounds.end());
        if self.speech_edit.is_none() {
            if self.subtitle_edit.is_some() {
                return;
            }
            if let Some(backend) = &mut self.backend {
                backend.pause();
            }
            if !finished && self.can_play() {
                let mut reference = task.clone();
                reference.play_speech_track = false;
                match ScrubAudio::new(&reference, self.volume, self.muted) {
                    Ok(audio) => self.scrub_audio = Some(audio),
                    Err(error) => log::warn!("Timeline scrub audio unavailable: {error}"),
                }
            }
        }
        self.speech_edit = Some((id.to_owned(), start));
        self.seek_to(start + grab, false);
        if let Some(audio) = &mut self.scrub_audio {
            audio.pending = Some(start + grab);
        }
        if finished {
            self.speech_edit = None;
            self.scrub_audio = None;
            let task = self.store.get_mut(&task_id).unwrap();
            let track = task.speech_track.as_mut().unwrap();
            let old = track.clone();
            let clip = track.clips.iter_mut().find(|clip| clip.id == id).unwrap();
            if clip.start_ms == start {
                return;
            }
            clip.start_ms = start;
            track.clips.sort_by_key(|clip| clip.start_ms);
            track.rendered = false;
            if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
                self.store.get_mut(&task_id).unwrap().speech_track = Some(old);
                self.error = Some(error.to_string());
            } else {
                log::info!("Media speech clip moved: task={task_id} cue={id} start_ms={start}");
                if let Err(error) = self.queue_speech_render(&task_id, Vec::new()) {
                    self.error = Some(error);
                }
            }
        }
    }

    pub(in crate::plugins::media) fn audition_speech(&mut self, id: &str) {
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get_mut(id))
        else {
            return;
        };
        let Some(track) = &task.speech_track else {
            return;
        };
        if !track.rendered {
            return;
        }
        let Some(clip) = track.clips.iter().find(|clip| clip.id == id) else {
            return;
        };
        let (start, end) = (clip.start_ms, clip.end_ms());
        if self.speech_preview_end == Some(end)
            && self
                .backend
                .as_ref()
                .is_some_and(|b| b.get_status() == PlaybackStatus::Playing)
        {
            self.toggle_play();
            return;
        }
        task.play_speech_track = true;
        self.apply_audio_selection(false);
        self.seek_to(start, true);
        self.speech_preview_end = Some(end);
    }

    pub(in crate::plugins::media) fn cancel_subtitle_trim(&mut self) {
        self.speech_edit = None;
        self.speech_preview_end = None;
        self.scrub_audio = None;
        if self
            .subtitle_edit
            .as_ref()
            .is_some_and(|edit| edit.dragging_time)
        {
            self.subtitle_edit = None;
        }
    }
}

#[cfg(all(test, feature = "mpv"))]
#[test]
#[ignore = "requires the installed libmpv playback runtime"]
fn scrub_audio_plays_briefly_without_advancing_the_paused_player() {
    use crate::plugins::media::backend::mpv::tests::{player, silent_wav};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scrub.wav");
    silent_wav(&path);
    let mut main = player();
    main.load_local_file(path.clone()).unwrap();
    let mut audio = ScrubAudio {
        player: Box::new(player()),
        pending: Some(1_000),
        sampled: None,
    };
    audio.player.set_audio_only_mode(true);
    audio.player.load_local_file(path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while main.get_duration_ms() == 0 {
        main.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    main.seek(1_000);
    // Both forward and backward motion resume a short preview, then fall silent.
    for time in [1_000, 2_000, 500] {
        audio.pending = Some(time);
        let mut heard = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            audio.tick();
            main.tick();
            assert!(audio.player.take_error().is_none());
            heard |= audio.player.get_status() == PlaybackStatus::Playing
                && audio.player.get_time_ms() > time + 20;
            if heard && audio.player.get_status() == PlaybackStatus::Paused {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "audio preview did not play and pause"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!((time + 20..time + 600).contains(&audio.player.get_time_ms()));
        assert_eq!(main.get_status(), PlaybackStatus::Paused);
        assert!((main.get_time_ms() - 1_000).abs() < 2);
    }
}
