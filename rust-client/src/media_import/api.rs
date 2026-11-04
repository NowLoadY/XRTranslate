use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use crossbeam_channel::{Receiver, Sender, unbounded};

use super::{
    decode::run_import,
    types::{
        AudioImportError, AudioImportEvent, AudioImportOptions, AudioImportOutcome,
        AudioImportPacing,
    },
};

/// Owns a running import worker. Dropping the handle requests cancellation.
pub struct AudioImportHandle {
    stop_requested: Arc<AtomicBool>,
    events: Receiver<AudioImportEvent>,
    _worker: Option<JoinHandle<Result<AudioImportOutcome, AudioImportError>>>,
}

impl AudioImportHandle {
    pub fn stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    pub fn events(&self) -> &Receiver<AudioImportEvent> {
        &self.events
    }
}

impl Drop for AudioImportHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start decoding `path` on a dedicated worker thread.
///
/// The output sender should be the same `Sender<Vec<f32>>` consumed by the
/// recognition session. No frame is silently discarded. Realtime mode paces
/// output; fast mode is rejected unless `audio_tx` is bounded.
pub fn import_audio_file(
    path: impl AsRef<Path>,
    audio_tx: Sender<Vec<f32>>,
    options: AudioImportOptions,
) -> Result<AudioImportHandle, AudioImportError> {
    let path = path.as_ref().to_path_buf();
    spawn_import(
        audio_tx,
        options,
        move |audio_tx, options, stop, sent, events| {
            if options.range_ms.is_some() {
                #[cfg(feature = "mpv")]
                return super::mpv_stream::decode(
                    &path.to_string_lossy(),
                    "",
                    audio_tx,
                    options,
                    &stop,
                    sent,
                    events,
                );
                #[cfg(not(feature = "mpv"))]
                return Err(AudioImportError::Unsupported(
                    "Audio intervals require MPV".into(),
                ));
            }
            run_import(&path, audio_tx, options, &stop, sent, events)
        },
    )
}

/// Stream public online audio through the same decoding and recognition path.
pub fn import_audio_url(
    url: String,
    proxy: String,
    audio_tx: Sender<Vec<f32>>,
    options: AudioImportOptions,
) -> Result<AudioImportHandle, AudioImportError> {
    super::http::http_url(&url).map_err(AudioImportError::InvalidOptions)?;
    spawn_import(
        audio_tx,
        options,
        move |audio_tx, options, stop, sent, events| {
            // Only the URL path supplies a codec hint; signed query strings are not extensions.
            let parsed = super::http::http_url(&url).map_err(AudioImportError::InvalidOptions)?;
            let result = if options.range_ms.is_some()
                || options.audio_track.is_some()
                || parsed.path().ends_with(".m3u8")
            {
                Err(AudioImportError::Unsupported(
                    "Stream requires MPV decoding".into(),
                ))
            } else {
                let source = super::http::HttpAudioSource::open(&url, &proxy, Arc::clone(&stop))
                    .map_err(|error| AudioImportError::Io(std::io::Error::other(error)))?;
                super::decode::decode_source(
                    Path::new(parsed.path()),
                    Box::new(source),
                    &audio_tx,
                    &options,
                    &stop,
                    sent,
                    events,
                )
            };
            match result {
                // Never replay a prefix already sent to recognition when a decoder fails.
                Err(AudioImportError::Unsupported(_)) if sent.load(Ordering::Acquire) == 0 => {
                    #[cfg(feature = "mpv")]
                    {
                        super::mpv_stream::decode(
                            &url, &proxy, audio_tx, options, &stop, sent, events,
                        )
                    }
                    #[cfg(not(feature = "mpv"))]
                    {
                        Err(AudioImportError::Unsupported(
                            "This stream format requires the MPV feature".into(),
                        ))
                    }
                }
                result => result,
            }
        },
    )
}

fn spawn_import(
    audio_tx: Sender<Vec<f32>>,
    options: AudioImportOptions,
    run: impl FnOnce(
        Sender<Vec<f32>>,
        AudioImportOptions,
        Arc<AtomicBool>,
        &AtomicU64,
        &Sender<AudioImportEvent>,
    ) -> Result<u64, AudioImportError>
    + Send
    + 'static,
) -> Result<AudioImportHandle, AudioImportError> {
    validate_options(&audio_tx, &options)?;
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_requested);
    let (event_tx, events) = unbounded();
    let worker = thread::Builder::new()
        .name("audio-import".into())
        .spawn(move || {
            let sent_frames = AtomicU64::new(0);
            let result = run(
                audio_tx,
                options,
                Arc::clone(&worker_stop),
                &sent_frames,
                &event_tx,
            );
            let result = if worker_stop.load(Ordering::Acquire) {
                Err(AudioImportError::Cancelled)
            } else {
                result
            };
            let outcome = match result {
                Ok(output_frames) => AudioImportOutcome::Completed { output_frames },
                Err(AudioImportError::Cancelled) => AudioImportOutcome::Stopped {
                    output_frames: sent_frames.load(Ordering::Acquire),
                },
                Err(error @ AudioImportError::OutputClosed) => {
                    let _ = event_tx.send(AudioImportEvent::OutputClosed);
                    return Err(error);
                }
                Err(error) => {
                    let _ = event_tx.send(AudioImportEvent::Error(error.to_string()));
                    return Err(error);
                }
            };
            let event = match outcome {
                AudioImportOutcome::Completed { output_frames } => {
                    AudioImportEvent::Completed { output_frames }
                }
                AudioImportOutcome::Stopped { output_frames } => {
                    AudioImportEvent::Stopped { output_frames }
                }
            };
            let _ = event_tx.send(event);
            Ok(outcome)
        })
        .map_err(AudioImportError::Io)?;

    Ok(AudioImportHandle {
        stop_requested,
        events,
        _worker: Some(worker),
    })
}

fn validate_options(
    audio_tx: &Sender<Vec<f32>>,
    options: &AudioImportOptions,
) -> Result<(), AudioImportError> {
    if options.range_ms.is_some_and(|(start, end)| end <= start) {
        return Err(AudioImportError::InvalidOptions(
            "Audio interval must have positive duration".into(),
        ));
    }
    if !(1..=2).contains(&options.output_channels) {
        return Err(AudioImportError::InvalidOptions(
            "output must be mono or stereo".into(),
        ));
    }
    if options.output_sample_rate == 0 {
        return Err(AudioImportError::InvalidOptions(
            "output sample rate must be greater than zero".into(),
        ));
    }
    if options
        .gate_threshold_db
        .is_some_and(|db| !db.is_finite() || !(-80.0..=0.0).contains(&db))
    {
        return Err(AudioImportError::InvalidOptions(
            "noise gate threshold must be between -80 and 0 dBFS".into(),
        ));
    }
    if options.chunk_frames == 0 {
        return Err(AudioImportError::InvalidOptions(
            "chunk_frames must be greater than zero".into(),
        ));
    }
    if options.pacing == AudioImportPacing::AsFastAsPossible && audio_tx.capacity().is_none() {
        return Err(AudioImportError::InvalidOptions(
            "fast imports require a bounded audio channel".into(),
        ));
    }
    Ok(())
}
