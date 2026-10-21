use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

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
        output: PathBuf,
        ctx: eframe::egui::Context,
    ) -> Result<Self, String> {
        let super::backend::MediaSource::LocalFile(input) = &task.source else {
            return Err("A local video is required".into());
        };
        if output.canonicalize().ok().as_ref()
            == Some(&input.canonicalize().map_err(|e| e.to_string())?)
        {
            return Err("Choose a different file to preserve the original video.".into());
        }
        let input = input.clone();
        let subtitles = task.subtitles.export_playback_srt();
        let style = task.subtitle_style.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, result) = crossbeam_channel::bounded(1);
        let (worker_progress, worker_cancelled) = (progress.clone(), cancelled.clone());
        let worker = std::thread::Builder::new()
            .name("subtitle-video-export".into())
            .spawn(move || {
                let encoded = (|| -> Result<PathBuf, String> {
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
                        subtitle_file.path(),
                        &style,
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
    pub(super) fn start_video_export(&mut self, id: &str, ctx: &eframe::egui::Context) {
        if self.video_export.is_some() {
            return;
        }
        self.save_active();
        #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
        if let Some(task) = self
            .store
            .get(id)
            .filter(|task| VideoExport::supported(task))
        {
            let stem = std::path::Path::new(&task.title)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            if let Some(path) =
                crate::export::choose_path(&format!("{stem}-subtitled"), crate::export::Format::Mp4)
            {
                self.exported_video = None;
                match VideoExport::start(task, path, ctx.clone()) {
                    Ok(job) => self.video_export = Some(job),
                    Err(error) => self.error = Some(error),
                }
            }
        }
        let _ = (id, ctx);
    }
}

#[cfg(all(test, feature = "mpv", any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use crate::plugins::media::{
        backend::{MediaSource, mpv::encode_video},
        subtitles::SubtitleTimeline,
        task::{MediaTask, SubtitleMode},
    };

    #[test]
    #[ignore = "requires the installed video playback runtime and encoders"]
    fn exports_video_without_overwriting_source_or_publishing_cancelled_work() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp4");
        let srt = directory.path().join("source.srt");
        std::fs::write(&srt, "1\n00:00:02,000 --> 00:00:03,000\nFixture\n").unwrap();
        encode_video(std::path::Path::new("av://lavfi:color=c=black:s=640x360:r=25:d=1[out0];sine=frequency=440:duration=1[out1]"), &source, &srt, &Default::default(), &AtomicU32::new(0), &AtomicBool::new(false)).unwrap();
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
        assert!(VideoExport::start(&task, source.clone(), Default::default()).is_err());
        let output = directory.path().join("subtitled.mp4");
        let job = VideoExport::start(&task, output.clone(), Default::default()).unwrap();
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
        let job = VideoExport::start(&task, cancelled.clone(), Default::default()).unwrap();
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
