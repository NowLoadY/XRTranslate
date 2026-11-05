//! Translate once, then synthesize a separate, time-aligned playback track.
use super::{
    controller::MediaController,
    task::{MediaTask, SpeechTrack},
};
use crate::backend::{BackendManager, BackendStart, BackendStatus};
use eframe::egui;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufWriter, Cursor},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use xrtranslate_protocol::tts::{SpeechRequest, VoiceStatus};

const SAMPLE_RATE: u32 = 48_000;
const SPEC: hound::WavSpec = hound::WavSpec {
    channels: 1,
    sample_rate: SAMPLE_RATE,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
};

pub(super) struct SpeechJob {
    pub task_id: String,
    language: String,
    pending: Option<MediaTask>,
    preparing: Option<Instant>,
    next_poll: Instant,
    result: Option<crossbeam_channel::Receiver<Result<tempfile::TempPath, String>>>,
    cancelled: Arc<AtomicBool>,
    progress: Arc<SpeechProgress>,
    total: usize,
    input_key: u64,
}

#[derive(Default)]
struct SpeechProgress {
    completed: AtomicUsize,
    waiting: AtomicBool,
}

impl SpeechJob {
    pub fn progress(&self) -> f32 {
        self.progress.completed.load(Ordering::Relaxed) as f32 / self.total as f32
    }

    pub fn is_waiting(&self) -> bool {
        self.progress.waiting.load(Ordering::Relaxed)
    }

    fn poll(
        &mut self,
        backend: &mut BackendManager,
        server: &str,
        directory: &Path,
        ctx: &egui::Context,
    ) -> Option<Result<tempfile::TempPath, String>> {
        ctx.request_repaint_after(Duration::from_millis(100));
        if let Some(result) = &self.result {
            return match result.try_recv() {
                Ok(result) => Some(result),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(_) => Some(Err("Speech generation stopped unexpectedly".into())),
            };
        }
        if Instant::now() < self.next_poll {
            return None;
        }
        self.next_poll = Instant::now() + Duration::from_millis(250);
        let ready = if let Some(start) = self.preparing {
            match backend.status(server) {
                BackendStatus::Ready => Ok(true),
                BackendStatus::Starting(_) if start.elapsed() < Duration::from_secs(180) => {
                    Ok(false)
                }
                BackendStatus::Starting(_) => Err("Speech service startup timed out".into()),
                BackendStatus::Failed(error) => Err(error),
            }
        } else {
            self.preparing = Some(Instant::now());
            backend
                .prepare(server)
                .map(|status| matches!(status, BackendStart::Ready))
        };
        match ready {
            Ok(false) => return None,
            Err(error) => return Some(Err(error)),
            Ok(true) => {}
        }
        let task = self.pending.take()?;
        let (send, receive) = crossbeam_channel::bounded(1);
        let (server, directory, ctx) = (server.to_owned(), directory.to_owned(), ctx.clone());
        let (cancelled, progress) = (self.cancelled.clone(), self.progress.clone());
        let worker = std::thread::Builder::new().name("media-speech".into()).spawn(move || {
            let result = (|| {
                tokio::runtime::Builder::new_current_thread().enable_all().build()
                    .map_err(|e| e.to_string())?.block_on(async {
                        tokio::select! {
                            biased;
                            _ = async {
                                while !cancelled.load(Ordering::Relaxed) {
                                    tokio::time::sleep(Duration::from_millis(50)).await;
                                }
                            } => Err("Speech generation cancelled".into()),
                            output = generate(&task, &server, &directory, &cancelled, &progress, &ctx) => output,
                        }
                    })
            })();
            match &result {
                Ok(_) => log::info!("Media speech completed: task={} language={} cues={}", task.id, task.target_language, task.subtitles.count()),
                Err(error) => log::warn!("Media speech stopped: task={} language={} completed={} error={error}", task.id, task.target_language, progress.completed.load(Ordering::Relaxed)),
            }
            let _ = send.send(result);
            ctx.request_repaint();
        });
        if let Err(error) = worker {
            return Some(Err(error.to_string()));
        }
        self.result = Some(receive);
        None
    }
}

impl Drop for SpeechJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn validate(task: &MediaTask) -> Result<(), String> {
    let target = xrtranslate_engine::language::SupportedLanguage::parse(&task.target_language)
        .ok_or("Choose a target language first.")?;
    if task.is_task_running || task.subtitles.count() == 0 {
        return Err("Finish translating the subtitles first.".into());
    }
    for (index, cue) in task.subtitles.cues().iter().enumerate() {
        if cue
            .translated_text
            .as_ref()
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(format!("Subtitle {} has no translation", index + 1));
        }
        if !cue.target_language.is_empty()
            && xrtranslate_engine::language::SupportedLanguage::parse(&cue.target_language)
                != Some(target)
        {
            return Err(format!(
                "Subtitle {} is not translated into {}",
                index + 1,
                task.target_language
            ));
        }
        if cue.start_ms < 0
            || cue.end_ms <= cue.start_ms
            || task
                .subtitles
                .cues()
                .get(index + 1)
                .is_some_and(|next| next.start_ms <= cue.start_ms)
        {
            return Err(format!(
                "Subtitle {} has invalid or simultaneous timing",
                index + 1
            ));
        }
    }
    Ok(())
}

fn input_key(task: &MediaTask) -> u64 {
    let mut key = DefaultHasher::new();
    task.target_language.hash(&mut key);
    task.duration_ms.hash(&mut key);
    for cue in task.subtitles.cues() {
        (
            cue.start_ms,
            cue.end_ms,
            &cue.translated_text,
            &cue.target_language,
        )
            .hash(&mut key);
    }
    key.finish()
}

async fn generate(
    task: &MediaTask,
    server: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    progress: &SpeechProgress,
    ctx: &egui::Context,
) -> Result<tempfile::TempPath, String> {
    validate(task)?;
    let mut url = reqwest::Url::parse(server).map_err(|e| e.to_string())?;
    url.set_scheme(if matches!(url.scheme(), "wss" | "https") {
        "https"
    } else {
        "http"
    })
    .map_err(|_| "Invalid speech service URL")?;
    url.set_query(None);
    url.set_fragment(None);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    url.set_path("/tts/voice");
    let status: VoiceStatus = response(client.get(url.clone()), progress)
        .await?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if !status.ready {
        return Err("Choose an available voice in the TTS Center first.".into());
    }
    url.set_path("/tts/synthesize");
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let output = tempfile::Builder::new()
        .suffix(".wav")
        .tempfile_in(directory)
        .map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(
        BufWriter::new(output.reopen().map_err(|e| e.to_string())?),
        SPEC,
    )
    .map_err(|e| e.to_string())?;
    let mut written = 0u64;
    for (index, cue) in task.subtitles.cues().iter().enumerate() {
        let start = cue.start_ms as u64 * u64::from(SAMPLE_RATE) / 1000;
        // Natural speech may use the following pause, but must not displace the next cue.
        let end_ms = task
            .subtitles
            .cues()
            .get(index + 1)
            .map_or(task.duration_ms.max(cue.end_ms), |next| next.start_ms);
        let end = end_ms as u64 * u64::from(SAMPLE_RATE) / 1000;
        let available = usize::try_from(end - start).map_err(|e| e.to_string())?;
        let audio = async {
            let body = response(
                client.post(url.clone()).json(&SpeechRequest {
                    text: cue.translated_text.clone().unwrap(),
                    language: task.target_language.clone(),
                    voice: Some(status.selection.clone()),
                }),
                progress,
            )
            .await?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
            let samples = decode(&body)?;
            fit(samples, available, cancelled)
        }
        .await
        .map_err(|error: String| {
            format!(
                "Subtitle {} ({}–{} ms): {error}",
                index + 1,
                cue.start_ms,
                cue.end_ms
            )
        })?;
        while written < start {
            writer.write_sample(0i16).map_err(|e| e.to_string())?;
            written += 1;
            if written.is_multiple_of(u64::from(SAMPLE_RATE)) && cancelled.load(Ordering::Relaxed) {
                return Err("Speech generation cancelled".into());
            }
        }
        for sample in audio {
            writer.write_sample(sample).map_err(|e| e.to_string())?;
            written += 1;
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("Speech generation cancelled".into());
        }
        progress.completed.store(index + 1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    let duration = task
        .duration_ms
        .max(task.subtitles.cues().last().unwrap().end_ms) as u64
        * u64::from(SAMPLE_RATE)
        / 1000;
    while written < duration {
        writer.write_sample(0i16).map_err(|e| e.to_string())?;
        written += 1;
        if written.is_multiple_of(u64::from(SAMPLE_RATE)) && cancelled.load(Ordering::Relaxed) {
            return Err("Speech generation cancelled".into());
        }
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(output.into_temp_path())
}

async fn response(
    request: reqwest::RequestBuilder,
    progress: &SpeechProgress,
) -> Result<reqwest::Response, String> {
    let mut retries = 0u32;
    loop {
        let response = request
            .try_clone()
            .ok_or("Speech request cannot be retried")?
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        if status.is_success() {
            if retries > 0 {
                log::info!(
                    "Media speech resumed: subtitle={} retries={retries}",
                    progress.completed.load(Ordering::Relaxed) + 1
                );
            }
            return Ok(response);
        }
        let delay = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
            .map(Duration::from_secs);
        let error = response.text().await.map_err(|e| e.to_string())?;
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay = delay
                .unwrap_or_else(|| Duration::from_secs((60 * (1 << retries.min(3))).min(300)))
                .max(Duration::from_secs(1));
            let until = tokio::time::Instant::now()
                .checked_add(delay)
                .ok_or("Speech service returned an invalid retry delay")?;
            retries = retries.saturating_add(1);
            log::warn!(
                "Media speech waiting: subtitle={} retry={retries} delay_s={} error={error}",
                progress.completed.load(Ordering::Relaxed) + 1,
                delay.as_secs()
            );
            progress.waiting.store(true, Ordering::Relaxed);
            // The job's cancellation branch drops this wait and the partial WAV.
            tokio::time::sleep_until(until).await;
            progress.waiting.store(false, Ordering::Relaxed);
            continue;
        }
        return Err(format!("Speech service ({status}): {error}"));
    }
}

fn decode(wav: &[u8]) -> Result<Vec<i16>, String> {
    let mut reader = hound::WavReader::new(Cursor::new(wav)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err("Speech service returned unsupported audio".into());
    }
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if samples.is_empty() {
        return Err("Speech service returned empty audio".into());
    }
    if spec.sample_rate == SAMPLE_RATE {
        return Ok(samples);
    }
    let input: Vec<_> = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
    let mut resampler =
        xrtranslate_engine::audio::StreamingResampler::new(spec.sample_rate, SAMPLE_RATE, 1)?;
    let mut output = resampler.push(&input)?;
    output.extend(resampler.finish()?);
    Ok(output
        .into_iter()
        .map(|s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect())
}

fn fit(samples: Vec<i16>, available: usize, cancelled: &AtomicBool) -> Result<Vec<i16>, String> {
    if samples.len() <= available {
        return Ok(samples);
    }
    #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
    {
        let source = tempfile::Builder::new()
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(
            BufWriter::new(source.reopen().map_err(|e| e.to_string())?),
            SPEC,
        )
        .map_err(|e| e.to_string())?;
        let tempo = samples.len() as f64 / available.max(1) as f64 * 1.02;
        for sample in samples {
            writer.write_sample(sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        let output = tempfile::Builder::new()
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        super::backend::mpv::encode_audio(source.path(), output.path(), tempo, cancelled)?;
        let mut fitted = decode(&std::fs::read(output.path()).map_err(|e| e.to_string())?)?;
        if fitted.len() > available {
            // MPV pads the final PCM encoder frame with zero samples. Remove
            // only that padding past the cue boundary, never speech samples.
            if fitted[available..].iter().any(|&sample| sample != 0) {
                return Err("Speech could not fit its subtitle timing".into());
            }
            fitted.truncate(available);
        }
        log::info!("Media speech timing fitted: tempo={tempo:.3} available_samples={available}");
        return Ok(fitted);
    }
    #[cfg(not(all(feature = "mpv", any(windows, target_os = "linux"))))]
    {
        let _ = cancelled;
        Err("The media playback runtime is required to fit speech to subtitle timing".into())
    }
}

impl MediaController {
    pub(crate) fn start_speech(&mut self) {
        if self.speech_job.is_some() || self.subtitle_edit.is_some() {
            return;
        }
        self.save_active();
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
        else {
            return;
        };
        if let Err(error) = validate(task) {
            self.error = Some(error);
            return;
        }
        self.error = None;
        self.speech_job = Some(SpeechJob {
            task_id: task.id.clone(),
            language: task.target_language.clone(),
            total: task.subtitles.count(),
            input_key: input_key(task),
            pending: Some(task.clone()),
            preparing: None,
            next_poll: Instant::now(),
            result: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(SpeechProgress::default()),
        });
    }

    pub(crate) fn poll_speech(
        &mut self,
        backend: &mut BackendManager,
        server: &str,
        ctx: &egui::Context,
    ) {
        let Some(result) = self
            .speech_job
            .as_mut()
            .and_then(|job| job.poll(backend, server, &self.storage_dir.join("media-speech"), ctx))
        else {
            return;
        };
        let job = self.speech_job.take().unwrap();
        self.save_active();
        let result = result.and_then(|temporary| {
            let Some(task) = self.store.get_mut(&job.task_id) else {
                return Ok(());
            };
            if input_key(task) != job.input_key {
                return Err(
                    "Subtitles changed during speech generation. Generate the track again.".into(),
                );
            }
            let path = temporary.with_file_name(format!("{}.wav", uuid::Uuid::new_v4()));
            temporary.persist(&path).map_err(|e| e.to_string())?;
            let old = task.speech_track.replace(SpeechTrack {
                path: path.clone(),
                language: job.language.clone(),
            });
            let was_playing = std::mem::replace(&mut task.play_speech_track, true);
            if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
                let task = self.store.get_mut(&job.task_id).unwrap();
                task.speech_track = old;
                task.play_speech_track = was_playing;
                self.remove_speech_file(&path);
                return Err(error.to_string());
            }
            if self.active_task_id.as_deref() == Some(&job.task_id) {
                self.apply_audio_selection(false);
            }
            if let Some(old) = old {
                self.remove_speech_file(&old.path);
            }
            Ok(())
        });
        if let Err(error) = result {
            log::warn!("Media speech not saved: task={} error={error}", job.task_id);
            self.error = Some(error);
        }
    }

    pub(super) fn remove_speech_file(&self, path: &Path) {
        if let Ok(root) = self.storage_dir.join("media-speech").canonicalize()
            && path
                .canonicalize()
                .is_ok_and(|path| path.parent() == Some(root.as_path()))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(all(test, feature = "mpv", any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    #[ignore = "requires the installed media playback runtime for pitch-preserving alignment"]
    fn translated_speech_is_aligned_without_changing_recognition_input() {
        let directory = tempfile::tempdir().unwrap();
        let mut task = MediaTask::new(
            "speech-test".into(),
            super::super::backend::MediaSource::LocalFile("source.mp4".into()),
            "en".into(),
            "zh".into(),
            super::super::task::SubtitleMode::TranslateExisting,
            Default::default(),
        );
        task.duration_ms = 6000;
        for (start_ms, end_ms, original, translated) in [
            (1000, 2000, "Hello.", "你好。"),
            (3000, 6000, "Goodbye.", "再见。"),
        ] {
            task.subtitles.add_cue_with_metadata(
                super::super::subtitles::SubtitleCue {
                    id: start_ms.to_string(),
                    start_ms,
                    end_ms,
                    speaker_name: None,
                    original_text: original.into(),
                    translated_text: Some(translated.into()),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                },
                Default::default(),
            );
        }
        validate(&task).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let progress = Arc::new(SpeechProgress::default());
        let observed = progress.clone();
        let mock = std::thread::spawn(move || {
            let mut limited_at: Option<Instant> = None;
            for expected in [
                None,
                Some(("你好。", 3)),
                Some(("再见。", -1)),
                Some(("再见。", -1)),
                Some(("再见。", 1)),
                None,
                Some(("你好。", 0)),
                None,
                Some(("你好。", -2)),
            ] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break request.len();
                    }
                };
                let header = String::from_utf8_lossy(&request).to_ascii_lowercase();
                let len = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |len| len.parse::<usize>().unwrap());
                request.resize(header_end + len, 0);
                socket.read_exact(&mut request[header_end..]).unwrap();
                let body = if let Some((text, seconds)) = expected {
                    assert!(header.starts_with("post /tts/synthesize "));
                    let input: SpeechRequest =
                        serde_json::from_slice(&request[header_end..]).unwrap();
                    assert_eq!(input.text, text);
                    assert_eq!(input.language, "zh");
                    assert_eq!(input.voice.unwrap().preset.as_deref(), Some("fixture"));
                    if let Some(start) = limited_at.take() {
                        assert!(start.elapsed() >= Duration::from_secs(1));
                        assert_eq!(observed.completed.load(Ordering::Relaxed), 1);
                    }
                    if seconds < 0 {
                        let delay = if seconds == -1 { 1 } else { 3600 };
                        write!(socket, "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {delay}\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbusy").unwrap();
                        limited_at = Some(Instant::now());
                        continue;
                    }
                    if seconds == 0 {
                        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbusy").unwrap();
                        continue;
                    }
                    let mut buffer = Cursor::new(Vec::new());
                    let mut writer = hound::WavWriter::new(
                        &mut buffer,
                        hound::WavSpec {
                            sample_rate: 24_000,
                            ..SPEC
                        },
                    )
                    .unwrap();
                    for index in 0..24_000 * seconds {
                        writer
                            .write_sample(
                                (8000.0
                                    * (index as f32 * std::f32::consts::TAU * 440.0 / 24_000.0)
                                        .sin()) as i16,
                            )
                            .unwrap();
                    }
                    writer.finalize().unwrap();
                    buffer.into_inner()
                } else {
                    assert!(header.starts_with("get /tts/voice "));
                    serde_json::to_vec(&VoiceStatus {
                        ready: true,
                        selection: xrtranslate_protocol::tts::VoiceSelection {
                            preset: Some("fixture".into()),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .unwrap()
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let output = runtime
            .block_on(generate(
                &task,
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &progress,
                &egui::Context::default(),
            ))
            .unwrap();
        assert_eq!(progress.completed.load(Ordering::Relaxed), 2);
        assert!(!progress.waiting.load(Ordering::Relaxed));
        let samples = decode(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(samples.len(), 6 * SAMPLE_RATE as usize);
        assert!(samples[..SAMPLE_RATE as usize].iter().all(|&s| s == 0));
        for second in [1, 3] {
            let region =
                &samples[second * SAMPLE_RATE as usize..(second + 1) * SAMPLE_RATE as usize];
            let frequency = region
                .windows(2)
                .filter(|pair| pair[0] <= 0 && pair[1] > 0)
                .count();
            assert!(
                (430..450).contains(&frequency),
                "pitch changed: {frequency} Hz"
            );
        }
        assert!(samples[5 * SAMPLE_RATE as usize..].iter().all(|&s| s == 0));
        let error = runtime
            .block_on(generate(
                &task,
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &SpeechProgress::default(),
                &egui::Context::default(),
            ))
            .unwrap_err();
        assert!(
            error.contains("Subtitle 1 (1000–2000 ms)") && error.contains("503"),
            "{error}"
        );
        runtime.block_on(async {
            let progress = SpeechProgress::default();
            let cancelled = AtomicBool::new(false);
            let ctx = egui::Context::default();
            // Dropping the generation future is also how the job cancels a long wait.
            let generation = generate(
                &task,
                &server,
                directory.path(),
                &cancelled,
                &progress,
                &ctx,
            );
            tokio::pin!(generation);
            tokio::select! {
                result = &mut generation => panic!("should still be waiting: {result:?}"),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    assert!(progress.waiting.load(Ordering::Relaxed));
                    assert_eq!(progress.completed.load(Ordering::Relaxed), 0);
                }
            }
        });
        mock.join().unwrap();
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "failed synthesis must remove its partial output"
        );
        assert!(
            fit(
                vec![100; 3 * SAMPLE_RATE as usize],
                2 * SAMPLE_RATE as usize,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        let original = task.translation_request();
        task.speech_track = Some(SpeechTrack {
            path: output.to_path_buf(),
            language: "zh".into(),
        });
        task.play_speech_track = true;
        assert_eq!(task.translation_request(), original);
        assert!(matches!(
            task.playback_audio(),
            super::super::backend::PlaybackAudio::External(_)
        ));
        let fingerprint = input_key(&task);
        task.target_language = "ja".into();
        assert_ne!(input_key(&task), fingerprint);
        drop(output);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
