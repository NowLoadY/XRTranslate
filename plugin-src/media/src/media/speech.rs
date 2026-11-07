//! Synthesize translated clips independently; keep completed work as it arrives.
mod reference;
pub(super) mod track;
use super::{
    controller::MediaController,
    task::{MediaTask, SpeechClip, SpeechTrack},
};
use eframe::egui;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufWriter, Cursor},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use xrtranslate_plugin::{Client, Request};
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
    preparing: Option<Request>,
    result: Option<crossbeam_channel::Receiver<SpeechEvent>>,
    cancelled: Arc<AtomicBool>,
    progress: Arc<SpeechProgress>,
    total: usize,
    input_key: u64,
    replace_track: bool,
    cue_id: Option<String>,
}

enum SpeechEvent {
    Clip(SpeechClip, tempfile::TempPath),
    Finished(Result<(), String>),
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
        client: &Client,
        directory: &Path,
        ctx: &egui::Context,
    ) -> Option<SpeechEvent> {
        ctx.request_repaint_after(Duration::from_millis(100));
        if let Some(result) = &self.result {
            return match result.try_recv() {
                Ok(result) => Some(result),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(_) => Some(SpeechEvent::Finished(Err(
                    "Speech generation stopped unexpectedly".into(),
                ))),
            };
        }
        if self.cancelled.load(Ordering::Relaxed) {
            return Some(SpeechEvent::Finished(Ok(())));
        }
        let request = self
            .preparing
            .get_or_insert_with(|| client.request("backend.prepare", serde_json::json!({})));
        let server = match request.poll()? {
            Ok(value) => match value["server"].as_str() {
                Some(server) => server.to_owned(),
                None => {
                    return Some(SpeechEvent::Finished(Err(
                        "Invalid speech service response".into(),
                    )));
                }
            },
            Err(error) => return Some(SpeechEvent::Finished(Err(error))),
        };
        self.preparing = None;
        let task = self.pending.take()?;
        let cue_id = self.cue_id.clone();
        let (send, receive) = crossbeam_channel::unbounded();
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
                            } => Ok(()),
                            output = generate(&task, cue_id.as_deref(), &server, &directory, &cancelled, &progress, &ctx, &send) => output,
                        }
                    })
            })();
            match &result {
                Ok(_) if cancelled.load(Ordering::Relaxed) => log::info!("Media speech cancelled: task={} completed={}", task.id, progress.completed.load(Ordering::Relaxed)),
                Ok(_) => log::info!("Media speech completed: task={} language={} cues={}", task.id, task.target_language, task.subtitles.count()),
                Err(error) => log::warn!("Media speech stopped: task={} language={} completed={} error={error}", task.id, task.target_language, progress.completed.load(Ordering::Relaxed)),
            }
            let _ = send.send(SpeechEvent::Finished(result));
            ctx.request_repaint();
        });
        if let Err(error) = worker {
            return Some(SpeechEvent::Finished(Err(error.to_string())));
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

fn validate(task: &MediaTask, cue_id: Option<&str>) -> Result<(), String> {
    let target = xrtranslate_engine::language::SupportedLanguage::parse(&task.target_language)
        .ok_or("Choose a target language first.")?;
    if task.is_task_running || task.subtitles.count() == 0 {
        return Err("Finish translating the subtitles first.".into());
    }
    if cue_id.is_some_and(|id| !task.subtitles.cues().iter().any(|cue| cue.id == id)) {
        return Err("Subtitle no longer exists.".into());
    }
    for (index, cue) in task.subtitles.cues().iter().enumerate() {
        if cue_id.is_some_and(|id| cue.id != id) {
            continue;
        }
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
    task.speech_use_source_voice.hash(&mut key);
    task.speech_reference_range.hash(&mut key);
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
    cue_id: Option<&str>,
    server: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    progress: &SpeechProgress,
    ctx: &egui::Context,
    send: &crossbeam_channel::Sender<SpeechEvent>,
) -> Result<(), String> {
    validate(task, cue_id)?;
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
    if task.speech_use_source_voice && !status.supports_reference_audio {
        return Err("The selected TTS model does not support reference audio.".into());
    }
    if !task.speech_use_source_voice && !status.ready {
        return Err("Choose an available voice in the TTS Center first.".into());
    }
    url.set_path("/tts/synthesize");
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let existing = task.speech_track.as_ref();
    for (completed, (index, cue)) in task
        .subtitles
        .cues()
        .iter()
        .enumerate()
        .filter(|(_, cue)| cue_id.is_none_or(|id| cue.id == id))
        .enumerate()
    {
        if existing.is_some_and(|track| track.overlaps(cue.start_ms, cue.end_ms)) {
            progress.completed.store(completed + 1, Ordering::Relaxed);
            continue;
        }
        let start = cue.start_ms as u64 * u64::from(SAMPLE_RATE) / 1000;
        // Natural speech may use the following pause, but must not displace the next cue.
        let mut end_ms = task
            .subtitles
            .cues()
            .get(index + 1)
            .map_or(task.duration_ms.max(cue.end_ms), |next| next.start_ms);
        if cue_id.is_some() {
            end_ms = end_ms.min(cue.end_ms);
        }
        if let Some(next) = existing
            .into_iter()
            .flat_map(|track| &track.clips)
            .filter(|clip| clip.start_ms > cue.start_ms)
            .map(|clip| clip.start_ms)
            .min()
        {
            end_ms = end_ms.min(next);
        }
        let end = end_ms as u64 * u64::from(SAMPLE_RATE) / 1000;
        let available = usize::try_from(end - start).map_err(|e| e.to_string())?;
        let audio = async {
            let reference_audio = task
                .speech_use_source_voice
                .then(|| reference::audio(task, cue.start_ms, cue.end_ms))
                .transpose()?;
            let body = response(
                client.post(url.clone()).json(&SpeechRequest {
                    text: cue.translated_text.clone().unwrap(),
                    language: task.target_language.clone(),
                    voice: Some(status.selection.clone()),
                    reference_audio,
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
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let output = tempfile::Builder::new()
            .suffix(".wav")
            .tempfile_in(directory)
            .map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(
            BufWriter::new(output.reopen().map_err(|e| e.to_string())?),
            SPEC,
        )
        .map_err(|e| e.to_string())?;
        let duration_ms = (audio.len() as u64 * 1000).div_ceil(u64::from(SAMPLE_RATE)) as i64;
        let peak = audio
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let peaks = audio
            .chunks(audio.len().div_ceil(48))
            .map(|chunk| {
                (chunk.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / peak * 255.0)
                    as u8
            })
            .collect();
        for sample in audio {
            writer.write_sample(sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        let clip = SpeechClip {
            id: uuid::Uuid::new_v4().to_string(),
            path: directory.join(format!("{}.wav", uuid::Uuid::new_v4())),
            start_ms: cue.start_ms,
            duration_ms,
            text: cue.translated_text.clone().unwrap(),
            peaks,
        };
        send.send(SpeechEvent::Clip(clip, output.into_temp_path()))
            .map_err(|_| "Speech generation cancelled")?;
        progress.completed.store(completed + 1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    Ok(())
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
            // Cancelling drops this wait; previously published clips remain available.
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
    pub(crate) fn start_speech(&mut self, restart: bool) {
        self.start_speech_selection(restart, None);
    }

    pub(crate) fn start_cue_speech(&mut self, cue_id: String) {
        self.start_speech_selection(false, Some(cue_id));
    }

    fn start_speech_selection(&mut self, restart: bool, cue_id: Option<String>) {
        if self.speech_job.is_some() || self.subtitle_edit.is_some() || self.speech_edit.is_some() {
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
        if let Err(error) = validate(task, cue_id.as_deref()) {
            self.error = Some(error);
            return;
        }
        if let Some(cue) = task
            .subtitles
            .cues()
            .iter()
            .find(|cue| Some(&cue.id) == cue_id.as_ref())
            && task
                .speech_track
                .as_ref()
                .is_some_and(|track| track.overlaps(cue.start_ms, cue.end_ms))
        {
            self.error = Some("This time range already contains speech.".into());
            return;
        }
        self.error = None;
        let mut pending = task.clone();
        // Keep the old track until the first new clip is saved. A fresh run must
        // not mix the previously selected voice with its replacement.
        let replace_track = restart
            || (cue_id.is_none()
                && pending
                    .speech_track
                    .as_ref()
                    .is_some_and(|track| track.completed(task) == task.subtitles.count()));
        if replace_track {
            pending.speech_track = None;
        }
        self.speech_job = Some(SpeechJob {
            task_id: task.id.clone(),
            language: task.target_language.clone(),
            total: if cue_id.is_some() {
                1
            } else {
                task.subtitles.count()
            },
            input_key: input_key(task),
            replace_track,
            pending: Some(pending),
            preparing: None,
            result: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(SpeechProgress::default()),
            cue_id,
        });
    }

    pub(crate) fn cancel_speech(&mut self) {
        if let Some(job) = &self.speech_job {
            job.cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub(in crate::media) fn delete_speech(&mut self, id: &str) {
        if self.subtitle_edit.is_some() || self.speech_edit.is_some() {
            return;
        }
        let Some(task_id) = self.active_task_id.clone() else {
            return;
        };
        let Some(task) = self.store.get_mut(&task_id) else {
            return;
        };
        let Some(track) = &mut task.speech_track else {
            return;
        };
        let Some(index) = track.clips.iter().position(|clip| clip.id == id) else {
            return;
        };
        let old = track.clone();
        let removed = track.clips.remove(index);
        let retired = if track.clips.iter().any(|clip| clip.path == removed.path) {
            Vec::new()
        } else {
            vec![removed.path]
        };
        track.rendered = false;
        if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
            self.store.get_mut(&task_id).unwrap().speech_track = Some(old);
            self.error = Some(error.to_string());
            return;
        }
        self.cancel_subtitle_trim();
        if self
            .store
            .get(&task_id)
            .is_some_and(|task| task.play_speech_track)
            && let Some(backend) = &mut self.backend
        {
            backend.pause();
        }
        if let Err(error) = self.queue_speech_render(&task_id, retired) {
            self.error = Some(error);
        }
    }

    pub(crate) fn poll_speech(&mut self, client: &Client, ctx: &egui::Context) {
        while let Some(event) = self
            .speech_job
            .as_mut()
            .and_then(|job| job.poll(client, &self.storage_dir.join("media-speech"), ctx))
        {
            match event {
                SpeechEvent::Finished(result) => {
                    self.speech_job = None;
                    if let Err(error) = result {
                        self.error = Some(error);
                    }
                }
                SpeechEvent::Clip(clip, temporary) => {
                    let job = self.speech_job.as_ref().unwrap();
                    let (task_id, language, key, replace_track) = (
                        job.task_id.clone(),
                        job.language.clone(),
                        job.input_key,
                        job.replace_track,
                    );
                    self.save_active();
                    let result = (|| -> Result<(), String> {
                        let task = self
                            .store
                            .get_mut(&task_id)
                            .ok_or("Media project no longer exists")?;
                        if input_key(task) != key {
                            return Err("Subtitles changed during speech generation. Generate the track again.".into());
                        }
                        temporary.persist(&clip.path).map_err(|e| e.to_string())?;
                        let old = task.speech_track.clone();
                        let was_selected = task.play_speech_track;
                        let track = task.speech_track.get_or_insert_with(|| SpeechTrack {
                            path: self
                                .storage_dir
                                .join("media-speech")
                                .join(format!("{}.wav", uuid::Uuid::new_v4())),
                            language: language.clone(),
                            clips: Vec::new(),
                            rendered: false,
                        });
                        if replace_track {
                            track.language = language;
                            track.clips.clear();
                        }
                        if track.overlaps(clip.start_ms, clip.end_ms()) {
                            self.remove_speech_file(&clip.path);
                            return Err("This time range already contains speech.".into());
                        }
                        track.clips.push(clip.clone());
                        track.clips.sort_by_key(|clip| clip.start_ms);
                        track.rendered = false;
                        task.play_speech_track = was_selected || old.is_none();
                        if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
                            let task = self.store.get_mut(&task_id).unwrap();
                            task.speech_track = old;
                            task.play_speech_track = was_selected;
                            self.remove_speech_file(&clip.path);
                            return Err(error.to_string());
                        }
                        self.speech_job.as_mut().unwrap().replace_track = false;
                        let current = self
                            .store
                            .get(&task_id)
                            .unwrap()
                            .speech_track
                            .as_ref()
                            .unwrap();
                        let retired = old
                            .into_iter()
                            .flat_map(|track| track.clips)
                            .filter(|clip| !current.clips.iter().any(|new| new.path == clip.path))
                            .map(|clip| clip.path)
                            .collect();
                        self.queue_speech_render(&task_id, retired)?;
                        log::info!(
                            "Media speech clip saved: task={task_id} cue={} start_ms={} duration_ms={}",
                            clip.id,
                            clip.start_ms,
                            clip.duration_ms
                        );
                        // Retired assets are removed after mixing, so queued renders
                        // may still finish reading the preceding revision.
                        Ok(())
                    })();
                    if let Err(error) = result {
                        log::warn!("Media speech not saved: task={task_id} error={error}");
                        self.error = Some(error);
                        self.speech_job = None;
                    }
                }
            }
        }
    }

    pub(super) fn queue_speech_render(
        &mut self,
        task_id: &str,
        retired: Vec<std::path::PathBuf>,
    ) -> Result<(), String> {
        if self.speech_render.is_none() {
            self.speech_render = Some(track::RenderQueue::new(self.context.clone())?);
        }
        self.speech_render.as_ref().unwrap().push(
            self.store
                .get(task_id)
                .ok_or("Media project no longer exists")?,
            retired,
        )
    }

    pub(super) fn prepare_speech_track(&mut self, task_id: &str) {
        let Some(task) = self.store.get_mut(task_id) else {
            return;
        };
        let Some(track) = &mut task.speech_track else {
            return;
        };
        // Existing mixed tracks remain usable as one movable clip; new synthesis
        // publishes individual clips without trying to infer their old boundaries.
        if track.clips.is_empty()
            && track.rendered
            && let Ok(reader) = hound::WavReader::open(&track.path)
        {
            track.clips.push(SpeechClip {
                id: uuid::Uuid::new_v4().to_string(),
                path: track.path.clone(),
                start_ms: 0,
                duration_ms: i64::from(reader.duration()) * 1000
                    / i64::from(reader.spec().sample_rate),
                text: String::new(),
                peaks: Vec::new(),
            });
            track.path = track.path.with_file_name(format!(
                "{}-mix.wav",
                track.path.file_stem().unwrap_or_default().to_string_lossy()
            ));
            track.rendered = false;
        }
        if !track.rendered || !track.path.is_file() {
            track.rendered = false;
            if let Err(error) = self
                .store
                .save_to_dir(&self.storage_dir)
                .map_err(|e| e.to_string())
                .and_then(|_| self.queue_speech_render(task_id, Vec::new()))
            {
                self.error = Some(error);
            }
        }
    }

    pub(super) fn poll_speech_render(&mut self) {
        while let Some(result) = self
            .speech_render
            .as_ref()
            .and_then(|queue| queue.receive.try_recv().ok())
        {
            let Some(task) = self.store.get_mut(&result.task_id) else {
                self.remove_speech_file(&result.path);
                continue;
            };
            let Some(track) = &mut task.speech_track else {
                continue;
            };
            if track.key(task.duration_ms) != result.key {
                continue;
            }
            match result.result {
                Ok(()) => {
                    track.rendered = true;
                    let selected = task.play_speech_track;
                    let empty = track.clips.is_empty();
                    if empty {
                        task.speech_track = None;
                        task.play_speech_track = false;
                    }
                    if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
                        self.error = Some(error.to_string());
                    }
                    if selected
                        && !self.previewing_vocals
                        && self.active_task_id.as_deref() == Some(&result.task_id)
                    {
                        self.apply_audio_selection(false);
                        if let Some(backend) = &mut self.backend
                            && let Err(error) = backend.refresh_audio()
                        {
                            self.error = Some(error);
                        }
                    }
                    if empty {
                        self.remove_speech_file(&result.path);
                    }
                }
                Err(error) => {
                    log::warn!(
                        "Media speech mixing failed: task={} error={error}",
                        result.task_id
                    );
                    self.error = Some(error);
                }
            }
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
    use std::time::Instant;

    #[test]
    fn a_single_cue_creates_an_independent_clip_and_respects_occupied_time() {
        use crate::media::{backend::MediaSource, subtitles::SubtitleTimeline, task::SubtitleMode};
        use std::io::BufRead;
        let directory = tempfile::tempdir().unwrap();
        let mut task = MediaTask::new(
            "single".into(),
            MediaSource::LocalFile("source.mp4".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::TranslateExisting,
            Default::default(),
        );
        task.subtitles = SubtitleTimeline::parse("1\n00:00:01,000 --> 00:00:03,000\nHello\n\n2\n00:00:04,000 --> 00:00:06,000\nUntranslated\n").unwrap();
        let cue = task.subtitles.cues()[0].clone();
        task.subtitles
            .set_translation(&cue.id, "你好".into(), "en", "zh");
        assert!(validate(&task, None).is_err());
        validate(&task, Some(&cue.id)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for synthesis in [false, true] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(&mut socket);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(if synthesis {
                    "POST /tts/synthesize "
                } else {
                    "GET /tts/voice "
                }));
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let response = if synthesis {
                    let request: SpeechRequest = serde_json::from_slice(&body).unwrap();
                    assert_eq!(request.text, "你好");
                    assert!(request.reference_audio.is_none());
                    let mut output = Cursor::new(Vec::new());
                    let mut writer = hound::WavWriter::new(&mut output, SPEC).unwrap();
                    for _ in 0..SAMPLE_RATE {
                        writer.write_sample(100_i16).unwrap();
                    }
                    writer.finalize().unwrap();
                    output.into_inner()
                } else {
                    serde_json::to_vec(&VoiceStatus {
                        ready: true,
                        ..Default::default()
                    })
                    .unwrap()
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                socket.write_all(&response).unwrap();
            }
        });
        let (send, receive) = crossbeam_channel::unbounded();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(generate(
                &task,
                Some(&cue.id),
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &SpeechProgress::default(),
                &Default::default(),
                &send,
            ))
            .unwrap();
        worker.join().unwrap();
        let SpeechEvent::Clip(clip, temporary) = receive.try_recv().unwrap() else {
            panic!("missing clip")
        };
        temporary.persist(&clip.path).unwrap();
        assert_ne!(clip.id, cue.id);
        assert_eq!(clip.start_ms, cue.start_ms);
        assert!(clip.end_ms() <= cue.end_ms);
        task.speech_track = Some(SpeechTrack {
            path: directory.path().join("mix.wav"),
            language: "zh".into(),
            clips: vec![clip.clone()],
            rendered: true,
        });
        let mut controller = MediaController::open(directory.path().into(), None);
        controller.backend = None;
        controller.active_task_id = Some(task.id.clone());
        controller.subtitles = task.subtitles.clone();
        controller.store.tasks = vec![task];
        controller.start_cue_speech(cue.id.clone());
        assert!(controller.speech_job.is_none());
        assert_eq!(
            controller.error.as_deref(),
            Some("This time range already contains speech.")
        );
        controller.store.tasks[0]
            .speech_track
            .as_mut()
            .unwrap()
            .clips[0]
            .start_ms = 7000;
        controller.start_cue_speech(cue.id);
        assert_eq!(controller.speech_job.as_ref().unwrap().total, 1);
        assert!(!controller.speech_job.as_ref().unwrap().replace_track);
        assert_eq!(
            controller.store.tasks[0]
                .speech_track
                .as_ref()
                .unwrap()
                .clips[0]
                .start_ms,
            7000
        );
    }

    #[test]
    fn restarting_partial_speech_keeps_old_audio_until_a_replacement_arrives() {
        use super::super::{backend::MediaSource, subtitles::SubtitleTimeline, task::SubtitleMode};
        let directory = tempfile::tempdir().unwrap();
        let mut task = MediaTask::new(
            "voice-change".into(),
            MediaSource::LocalFile("source.mp4".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::TranslateExisting,
            Default::default(),
        );
        task.subtitles = SubtitleTimeline::parse(
            "1\n00:00:01,000 --> 00:00:03,000\nHello.\n\n2\n00:00:04,000 --> 00:00:06,000\nGoodbye.\n"
        ).unwrap();
        let ids = task
            .subtitles
            .cues()
            .iter()
            .map(|cue| cue.id.clone())
            .collect::<Vec<_>>();
        task.subtitles
            .set_translation(&ids[0], "你好。".into(), "en", "zh");
        task.subtitles
            .set_translation(&ids[1], "再见。".into(), "en", "zh");
        let clip = SpeechClip {
            id: ids[0].clone(),
            path: directory.path().join("old.wav"),
            start_ms: 1000,
            duration_ms: 2000,
            text: "你好。".into(),
            peaks: Vec::new(),
        };
        std::fs::write(&clip.path, b"previous audio").unwrap();
        task.speech_track = Some(SpeechTrack {
            path: directory.path().join("track.wav"),
            language: "zh".into(),
            clips: vec![clip.clone()],
            rendered: true,
        });
        let mut controller = MediaController::open(directory.path().into(), None);
        controller.backend = None;
        controller.active_task_id = Some(task.id.clone());
        controller.subtitles = task.subtitles.clone();
        controller.store.tasks = vec![task];
        for restart in [false, true] {
            controller.start_speech(restart);
            let job = controller.speech_job.as_ref().unwrap();
            assert_eq!(job.replace_track, restart);
            assert_eq!(
                job.pending.as_ref().unwrap().speech_track.is_none(),
                restart
            );
            assert_eq!(
                controller.store.tasks[0]
                    .speech_track
                    .as_ref()
                    .unwrap()
                    .clips,
                [clip.clone()]
            );
            assert_eq!(std::fs::read(&clip.path).unwrap(), b"previous audio");
            controller.speech_job = None;
        }
    }

    #[test]
    #[ignore = "requires the installed media playback runtime for pitch-preserving alignment"]
    fn translated_speech_is_aligned_without_changing_recognition_input() {
        #[cfg(windows)]
        if let Some(host) = std::env::var_os("XRT_TEST_HOST_EXECUTABLE") {
            xrtranslate_client_core::playback::configure_host_runtime(Path::new(&host));
        }
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
        validate(&task, None).unwrap();
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
                Some(("你好。", 3)),
                Some(("再见。", 0)),
                None,
                Some(("再见。", 1)),
                None,
                Some(("你好。", 3)),
                Some(("再见。", -2)),
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
        let (send, receive) = crossbeam_channel::unbounded();
        runtime
            .block_on(generate(
                &task,
                None,
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &progress,
                &egui::Context::default(),
                &send,
            ))
            .unwrap();
        let collect = |track: &mut SpeechTrack| {
            for event in receive.try_iter() {
                if let SpeechEvent::Clip(clip, temporary) = event {
                    temporary.persist(&clip.path).unwrap();
                    track.clips.push(clip);
                }
            }
        };
        let mut track = SpeechTrack {
            path: directory.path().join("mix.wav"),
            language: "zh".into(),
            clips: Vec::new(),
            rendered: false,
        };
        collect(&mut track);
        assert_eq!(track.clips.len(), 2);
        track::render(&track, task.duration_ms, None).unwrap();
        assert_eq!(progress.completed.load(Ordering::Relaxed), 2);
        assert!(!progress.waiting.load(Ordering::Relaxed));
        let samples = decode(&std::fs::read(&track.path).unwrap()).unwrap();
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
        let mut partial = SpeechTrack {
            path: directory.path().join("partial.wav"),
            language: "zh".into(),
            clips: Vec::new(),
            rendered: false,
        };
        let partial_progress = SpeechProgress::default();
        let error = runtime
            .block_on(generate(
                &task,
                None,
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &partial_progress,
                &egui::Context::default(),
                &send,
            ))
            .unwrap_err();
        assert!(
            error.contains("Subtitle 2 (3000–6000 ms)") && error.contains("503"),
            "{error}"
        );
        collect(&mut partial);
        assert_eq!(
            partial.clips.len(),
            1,
            "a later error must not discard completed speech"
        );
        assert_eq!(partial_progress.completed.load(Ordering::Relaxed), 1);
        track::render(&partial, task.duration_ms, None).unwrap();
        let partial_samples = decode(&std::fs::read(&partial.path).unwrap()).unwrap();
        assert_eq!(
            &partial_samples[..3 * SAMPLE_RATE as usize],
            &samples[..3 * SAMPLE_RATE as usize]
        );
        assert!(
            partial_samples[3 * SAMPLE_RATE as usize..]
                .iter()
                .all(|&s| s == 0)
        );
        // Resume requests only the missing translation and preserves an independently moved clip.
        partial.clips[0].start_ms = 500;
        task.speech_track = Some(partial.clone());
        runtime
            .block_on(generate(
                &task,
                None,
                &server,
                directory.path(),
                &AtomicBool::new(false),
                &SpeechProgress::default(),
                &egui::Context::default(),
                &send,
            ))
            .unwrap();
        collect(&mut partial);
        assert_eq!(partial.clips.len(), 2);
        assert_eq!(partial.clips[0].start_ms, 500);
        task.speech_track = None;
        let mut cancelled_track = SpeechTrack {
            path: directory.path().join("cancelled.wav"),
            language: "zh".into(),
            clips: Vec::new(),
            rendered: false,
        };
        runtime.block_on(async {
            let progress = SpeechProgress::default();
            let cancelled = AtomicBool::new(false);
            let ctx = egui::Context::default();
            let generation = generate(&task, None, &server, directory.path(), &cancelled, &progress, &ctx, &send);
            tokio::pin!(generation);
            tokio::select! {
                result = &mut generation => panic!("should still be waiting: {result:?}"),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    assert!(progress.waiting.load(Ordering::Relaxed));
                    assert_eq!(progress.completed.load(Ordering::Relaxed), 1);
                    collect(&mut cancelled_track);
                    assert_eq!(cancelled_track.clips.len(), 1, "completed clips are available before generation finishes");
                }
            }
        });
        mock.join().unwrap();
        assert!(cancelled_track.clips[0].path.is_file());
        track::render(&cancelled_track, task.duration_ms, None).unwrap();
        assert_eq!(
            decode(&std::fs::read(&cancelled_track.path).unwrap()).unwrap(),
            partial_samples
        );
        // Incremental movement clears the old location, mixes overlap, and matches a fresh export.
        let old = track.clone();
        track.clips[1].start_ms = 1_500;
        track::render(&track, task.duration_ms, Some(&old)).unwrap();
        let moved = decode(&std::fs::read(&track.path).unwrap()).unwrap();
        assert!(moved[3 * SAMPLE_RATE as usize..].iter().all(|&s| s == 0));
        track::render(&track, task.duration_ms, None).unwrap();
        assert_eq!(decode(&std::fs::read(&track.path).unwrap()).unwrap(), moved);
        assert_eq!(task.subtitles.cues()[1].start_ms, 3000);
        assert!(
            fit(
                vec![100; 3 * SAMPLE_RATE as usize],
                2 * SAMPLE_RATE as usize,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        let original = task.translation_request();
        task.speech_track = Some(track);
        task.play_speech_track = true;
        assert_eq!(task.translation_request(), original);
        assert!(matches!(
            task.playback_audio(),
            super::super::backend::PlaybackAudio::External(_)
        ));
        let fingerprint = input_key(&task);
        task.target_language = "ja".into();
        assert_ne!(input_key(&task), fingerprint);
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tmp")),
            "unfinished clip files must be cleaned up"
        );
    }
}
