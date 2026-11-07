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
        player.set_channel_routing(task.playback_channels());
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
    pub(in crate::media) fn subtitle_trim_bounds(
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

    pub(in crate::media) fn trim_subtitle(
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

    pub(in crate::media) fn speech_move_bounds(
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

    pub(in crate::media) fn move_speech(
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
                if reference.playback_sources().is_empty() {
                    reference.playback_tracks = None;
                }
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

    pub(in crate::media) fn audition_speech(&mut self, id: &str) {
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

    pub(in crate::media) fn cancel_subtitle_trim(&mut self) {
        self.finish_vocal_preview();
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

    pub(in crate::media) fn set_vocal_reference(&mut self, range: Option<[i64; 2]>) {
        if self.speech_job.is_some() {
            return;
        }
        if range.is_some_and(|[start, end]| start < 0 || end - start < 5000 || end - start > 20_000)
        {
            self.error = Some("Select 5–20 seconds of clear speech from one speaker.".into());
            return;
        }
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get_mut(id))
        else {
            return;
        };
        task.speech_reference_range = range;
        if range.is_some() {
            task.speech_use_source_voice = true;
        }
        self.save_active();
    }

    pub(in crate::media) fn audition_vocals(&mut self, start: i64, end: i64) {
        if self.previewing_vocals && self.speech_preview_end == Some(end) {
            self.finish_vocal_preview();
            return;
        }
        self.finish_vocal_preview();
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
        else {
            return;
        };
        let Some(stems) = task
            .separated_audio
            .as_ref()
            .filter(|stems| stems.vocals.is_file())
        else {
            return;
        };
        let audio = super::super::backend::PlaybackAudio::External(stems.vocals.clone()).adjusted(
            xrtranslate_client_core::playback::AudioMixSettings {
                volume: task
                    .audio_mix
                    .volume(super::super::task::PlaybackTrack::Vocals),
                ..Default::default()
            },
        );
        if let Some(backend) = &mut self.backend {
            backend.pause();
            if let Err(error) = backend.select_audio(audio) {
                self.error = Some(error);
                return;
            }
            backend.set_channel_routing(&[]);
            backend.seek(start.max(0));
            backend.play();
            self.previewing_vocals = true;
            self.speech_preview_end = Some(end);
        }
    }

    pub(super) fn finish_vocal_preview(&mut self) {
        if !std::mem::take(&mut self.previewing_vocals) {
            return;
        }
        self.speech_preview_end = None;
        if let Some(backend) = &mut self.backend {
            backend.pause();
            if let Some(task) = self
                .active_task_id
                .as_deref()
                .and_then(|id| self.store.get(id))
            {
                backend.set_channel_routing(task.playback_channels());
                if let Err(error) = backend.select_audio(task.playback_audio()) {
                    self.error = Some(error);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "mpv", any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the installed media playback runtime"]
    fn vocal_audition_restores_the_mix_without_changing_recognition_channels() {
        use crate::media::{
            backend::{MediaBackend, mpv::MpvBackend},
            task::SubtitleMode,
        };
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.wav");
        let vocals = directory.path().join("vocals.wav");
        for (path, channels) in [(&original, 2), (&vocals, 1)] {
            let mut writer = hound::WavWriter::create(
                path,
                hound::WavSpec {
                    channels,
                    sample_rate: 24_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..48_000 * usize::from(channels) {
                writer.write_sample(0_i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        let mut backend = MpvBackend::new().unwrap();
        backend.set_mute(true);
        let mut controller =
            MediaController::open(directory.path().into(), Some(Box::new(backend)));
        let mut task = MediaTask::new(
            "Audition".into(),
            MediaSource::LocalFile(original),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            Default::default(),
        );
        task.duration_ms = 2000;
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals,
            background: directory.path().join("unused.wav"),
        });
        let id = task.id.clone();
        controller.store.add_or_update(task);
        controller.open_task(&id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while controller.store.get(&id).unwrap().audio_channels.len() != 2 {
            assert!(
                Instant::now() < deadline,
                "original audio should load: {:?}",
                controller.error
            );
            controller.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mix = controller.store.get(&id).unwrap().playback_audio();
        controller.audition_vocals(0, 300);
        assert!(controller.previewing_vocals);
        while controller.previewing_vocals {
            assert!(
                Instant::now() < deadline,
                "audition should end: {:?}",
                controller.error
            );
            controller.tick();
            assert_eq!(controller.store.get(&id).unwrap().audio_channels.len(), 2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(controller.store.get(&id).unwrap().playback_audio(), mix);
        while !controller
            .backend
            .as_ref()
            .unwrap()
            .audio_tracks()
            .iter()
            .any(|track| track.selected && track.external.is_none())
        {
            assert!(
                Instant::now() < deadline,
                "the original playback mix should return"
            );
            controller.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(controller.get_status(), PlaybackStatus::Paused);
    }
}
