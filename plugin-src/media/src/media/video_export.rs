use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExportSettings {
    pub use_speech: bool,
    pub keep_background: bool,
    pub burn_subtitles: bool,
}

impl ExportSettings {
    pub fn for_task(task: &super::task::MediaTask) -> Self {
        Self {
            use_speech: task.speech_track.is_some(),
            keep_background: task
                .separated_audio
                .as_ref()
                .is_some_and(|stems| stems.background.is_file()),
            burn_subtitles: task.subtitles.count() > 0,
        }
    }

    pub fn problem(&self, task: &super::task::MediaTask) -> Option<&'static str> {
        if task.is_task_running {
            return Some("Finish the current task or edit first.");
        }
        if self.use_speech {
            let Some(track) = &task.speech_track else {
                return Some("Generate a speech track first.");
            };
            if !track.rendered || !track.path.is_file() {
                return Some("The speech track is not ready yet.");
            }
            if track.language != task.target_language
                || (track.clips.iter().any(|clip| !clip.text.is_empty())
                    && track.completed(task) < task.subtitles.count())
            {
                return Some("Finish generating the speech track first.");
            }
            if self.keep_background
                && task
                    .separated_audio
                    .as_ref()
                    .is_none_or(|stems| !stems.background.is_file())
            {
                return Some(
                    "Use vocal separation during recognition to prepare background audio.",
                );
            }
        }
        if self.burn_subtitles && task.subtitles.count() == 0 {
            return Some("No subtitles to export.");
        }
        None
    }
}

pub(super) struct VideoExport {
    progress: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
    result: crossbeam_channel::Receiver<Result<PathBuf, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl VideoExport {
    pub fn supported(task: &super::task::MediaTask) -> bool {
        cfg!(all(feature = "mpv", any(windows, target_os = "linux")))
            && task.media_type == super::task::MediaType::Video
            && matches!(&task.source, super::backend::MediaSource::LocalFile(path) if path.is_file())
    }

    #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
    pub fn start(
        task: &super::task::MediaTask,
        settings: ExportSettings,
        output: Option<PathBuf>,
        ctx: eframe::egui::Context,
    ) -> Result<Self, String> {
        if let Some(problem) = settings.problem(task) {
            return Err(problem.into());
        }
        let super::backend::MediaSource::LocalFile(input) = &task.source else {
            return Err("A local video is required".into());
        };
        let title = std::path::Path::new(&task.title)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let suffix = if settings.use_speech {
            "dubbed"
        } else if settings.burn_subtitles {
            "subtitled"
        } else {
            "export"
        };
        let input = input.clone();
        let subtitles = task.subtitles.export_playback_srt();
        let style = task.subtitle_style.clone();
        let original_audio = task.original_audio_track;
        let speech = settings
            .use_speech
            .then(|| task.speech_track.as_ref().unwrap().path.clone());
        let background = (settings.use_speech && settings.keep_background)
            .then(|| task.separated_audio.as_ref().unwrap().background.clone());
        let progress = Arc::new(AtomicU32::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, result) = crossbeam_channel::bounded(1);
        let (worker_progress, worker_cancelled) = (progress.clone(), cancelled.clone());
        let worker = std::thread::Builder::new()
            .name("media-video-export".into())
            .spawn(move || {
                let encoded = (|| -> Result<PathBuf, String> {
                    let output = output
                        .or_else(|| {
                            crate::export::choose_path(
                                &format!("{title}-{suffix}"),
                                crate::export::Format::Mp4,
                            )
                        })
                        .ok_or("")?;
                    if output.canonicalize().ok().as_ref()
                        == Some(&input.canonicalize().map_err(|e| e.to_string())?)
                    {
                        return Err(
                            "Choose a different file to preserve the original video.".into()
                        );
                    }
                    let subtitle_file = tempfile::Builder::new()
                        .suffix(".srt")
                        .tempfile()
                        .map_err(|e| e.to_string())?;
                    std::fs::write(subtitle_file.path(), subtitles).map_err(|e| e.to_string())?;
                    let temporary = tempfile::Builder::new()
                        .prefix(".xrtranslate-")
                        .suffix(".mp4")
                        .tempfile_in(output.parent().ok_or("Invalid output directory")?)
                        .map_err(|e| e.to_string())?
                        .into_temp_path();
                    super::backend::mpv::encode_video(
                        &input,
                        &temporary,
                        super::backend::mpv::VideoEncoding {
                            subtitles: settings.burn_subtitles.then_some(subtitle_file.path()),
                            style: &style,
                            audio: match speech.as_deref() {
                                Some(path) => super::backend::mpv::VideoAudio::Speech {
                                    path,
                                    background: background.as_deref(),
                                },
                                None => super::backend::mpv::VideoAudio::Original(original_audio),
                            },
                        },
                        &worker_progress,
                        &worker_cancelled,
                    )?;
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Err("Video export cancelled".into());
                    }
                    temporary.persist(&output).map_err(|e| e.to_string())?;
                    Ok(output)
                })();
                let _ = tx.send(encoded);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            progress,
            cancelled,
            result,
            worker: Some(worker),
        })
    }

    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.result.try_recv() {
            Ok(Err(_)) if self.cancelled.load(Ordering::Relaxed) => Some(Err(String::new())),
            Ok(result) => Some(result),
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Video export stopped unexpectedly".into())),
        }
    }
}

impl Drop for VideoExport {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl super::controller::MediaController {
    pub(super) fn start_video_export(
        &mut self,
        id: &str,
        settings: ExportSettings,
        ctx: &eframe::egui::Context,
    ) {
        if self.video_export.is_some() {
            return;
        }
        self.save_active();
        if self.subtitle_edit.is_some()
            || self.speech_edit.is_some()
            || self
                .speech_job
                .as_ref()
                .is_some_and(|job| job.task_id == id)
        {
            self.error = Some("Finish the current task or edit first.".into());
            return;
        }
        #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
        if let Some(task) = self
            .store
            .get(id)
            .filter(|task| VideoExport::supported(task))
        {
            self.exported_video = None;
            match VideoExport::start(task, settings, None, ctx.clone()) {
                Ok(job) => self.video_export = Some(job),
                Err(error) => self.error = Some(error),
            }
        }
        let _ = (id, settings, ctx);
    }
}

#[cfg(all(test, feature = "mpv", any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use crate::media::{
        backend::{
            MediaSource,
            mpv::{VideoAudio, VideoEncoding, encode_video},
        },
        subtitles::SubtitleTimeline,
        task::{MediaTask, SubtitleMode},
    };

    #[test]
    #[ignore = "requires the installed video playback runtime and encoders"]
    fn dubbed_export_replaces_original_audio_and_optionally_keeps_stereo_background() {
        use super::super::backend::mpv::encode_audio;
        use super::super::task::SpeechTrack;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.mp4");
        let speech = directory.path().join("配音, track.wav");
        let background = directory.path().join("背景, track.wav");
        let style = Default::default();
        encode_video(
            std::path::Path::new("av://lavfi:color=c=black:s=320x180:r=25:d=1[out0];sine=frequency=440:duration=1[out1]"),
            &source,
            VideoEncoding { subtitles: None, style: &style, audio: VideoAudio::Original(None) },
            &AtomicU32::new(0), &AtomicBool::new(false),
        ).unwrap();
        let mut writer = hound::WavWriter::create(
            &speech,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        // Longer than the video, as can happen after moving a speech clip.
        for i in 0..96_000 {
            writer
                .write_sample(
                    (0.2 * (std::f64::consts::TAU * 880.0 * i as f64 / 48_000.0).sin()
                        * i16::MAX as f64) as i16,
                )
                .unwrap();
        }
        writer.finalize().unwrap();
        let mut writer = hound::WavWriter::create(
            &background,
            hound::WavSpec {
                channels: 2,
                sample_rate: 44_100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for i in 0..44_100 {
            for frequency in [1760.0, 2200.0] {
                writer
                    .write_sample(
                        (0.1 * (std::f64::consts::TAU * frequency * i as f64 / 44_100.0).sin())
                            as f32,
                    )
                    .unwrap();
            }
        }
        writer.finalize().unwrap();
        let mut task = MediaTask::new(
            "Video".into(),
            MediaSource::LocalFile(source),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            Default::default(),
        );
        task.speech_track = Some(SpeechTrack {
            path: speech,
            language: "zh".into(),
            clips: Vec::new(),
            rendered: true,
        });
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals: directory.path().join("unused-vocals.wav"),
            background,
        });
        task.subtitles =
            SubtitleTimeline::parse("1\n00:00:00,000 --> 00:00:01,000\nHello.\n你好。").unwrap();
        for (keep_background, burn_subtitles) in [(false, false), (true, false), (true, true)] {
            let output = directory
                .path()
                .join(format!("dubbed-{keep_background}-{burn_subtitles}.mp4"));
            let job = VideoExport::start(
                &task,
                ExportSettings {
                    use_speech: true,
                    keep_background,
                    burn_subtitles,
                },
                Some(output.clone()),
                Default::default(),
            )
            .unwrap();
            assert_eq!(
                job.result
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap()
                    .unwrap(),
                output
            );
            let decoded = directory.path().join("decoded.wav");
            encode_audio(&output, &decoded, 1.0, &AtomicBool::new(false)).unwrap();
            let mut reader = hound::WavReader::open(decoded).unwrap();
            let spec = reader.spec();
            let duration = reader.duration() as f64 / spec.sample_rate as f64;
            // Allow codec/filter padding; the two-second speech must end with the one-second video.
            assert!(
                (0.9..1.2).contains(&duration),
                "Speech extended the video: {duration}"
            );
            let samples: Vec<_> = reader.samples::<i16>().map(Result::unwrap).collect();
            let amplitude = |channel: usize, frequency: f64| {
                let frames: Vec<_> = samples
                    .chunks_exact(spec.channels as usize)
                    .map(|frame| frame[channel] as f64 / i16::MAX as f64)
                    .skip(spec.sample_rate as usize / 10)
                    .take(spec.sample_rate as usize / 2)
                    .collect();
                let mut re = 0.0;
                let mut im = 0.0;
                for (i, value) in frames.iter().enumerate() {
                    let phase =
                        std::f64::consts::TAU * frequency * i as f64 / spec.sample_rate as f64;
                    re += value * phase.cos();
                    im += value * phase.sin();
                }
                2.0 * re.hypot(im) / frames.len() as f64
            };
            assert!(amplitude(0, 880.0) > 0.05, "Speech is missing");
            assert!(
                amplitude(0, 440.0) < 0.003,
                "Original voices leaked into the export"
            );
            if keep_background {
                assert_eq!(spec.channels, 2);
                assert!(
                    amplitude(0, 1760.0) > 0.03 && amplitude(1, 2200.0) > 0.03,
                    "Stereo background is missing"
                );
                assert!(
                    amplitude(0, 2200.0) < 0.003 && amplitude(1, 1760.0) < 0.003,
                    "Background stereo was lost"
                );
            } else {
                assert!(
                    amplitude(0, 1760.0) < 0.003,
                    "Background was included when disabled"
                );
            }
        }
        std::fs::remove_file(&task.separated_audio.as_ref().unwrap().background).unwrap();
        assert!(
            VideoExport::start(
                &task,
                ExportSettings {
                    use_speech: true,
                    keep_background: true,
                    burn_subtitles: false
                },
                None,
                Default::default()
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires the installed video playback runtime and encoders"]
    fn exports_video_without_overwriting_source_or_publishing_cancelled_work() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp4");
        let srt = directory.path().join("source.srt");
        std::fs::write(&srt, "1\n00:00:02,000 --> 00:00:03,000\nFixture\n").unwrap();
        encode_video(std::path::Path::new("av://lavfi:color=c=black:s=640x360:r=25:d=1[out0];sine=frequency=440:duration=1[out1]"), &source, VideoEncoding { subtitles: Some(&srt), style: &Default::default(), audio: VideoAudio::Original(None) }, &AtomicU32::new(0), &AtomicBool::new(false)).unwrap();
        let mut task = MediaTask::new(
            "test.mp4".into(),
            MediaSource::LocalFile(source.clone()),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            Default::default(),
        );
        task.subtitles = SubtitleTimeline::parse(
            "1\n00:00:00,000 --> 00:00:01,000\nA moment worth keeping.\n值得留住的一刻。",
        )
        .unwrap();
        task.subtitle_style.size = 40.0;
        task.subtitle_style.color = [32, 210, 80];
        let original = std::fs::read(&source).unwrap();
        assert!(
            VideoExport::start(
                &task,
                ExportSettings::for_task(&task),
                Some(source.clone()),
                Default::default()
            )
            .unwrap()
            .result
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .is_err()
        );
        let output = directory.path().join("subtitled.mp4");
        let job = VideoExport::start(
            &task,
            ExportSettings::for_task(&task),
            Some(output.clone()),
            Default::default(),
        )
        .unwrap();
        assert_eq!(
            job.result
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
                .unwrap(),
            output
        );
        assert!(std::fs::metadata(&output).unwrap().len() > 1000);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        if let Some(preview) = std::env::var_os("XRTRANSLATE_EXPORT_PREVIEW") {
            std::fs::copy(&output, preview).unwrap();
        }
        let cancelled = directory.path().join("cancelled.mp4");
        std::fs::write(&cancelled, b"existing destination").unwrap();
        let job = VideoExport::start(
            &task,
            ExportSettings::for_task(&task),
            Some(cancelled.clone()),
            Default::default(),
        )
        .unwrap();
        job.cancel();
        assert!(
            job.result
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
                .is_err()
        );
        assert_eq!(std::fs::read(cancelled).unwrap(), b"existing destination");
    }
}
