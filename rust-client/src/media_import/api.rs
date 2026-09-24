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
    validate_options(&audio_tx, &options)?;

    let path = path.as_ref().to_path_buf();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_requested);
    let (event_tx, events) = unbounded();
    let worker = thread::Builder::new()
        .name("audio-import".into())
        .spawn(move || {
            let sent_frames = AtomicU64::new(0);
            let result = run_import(
                &path,
                audio_tx,
                options,
                &worker_stop,
                &sent_frames,
                &event_tx,
            );
            let outcome = match result {
                Ok(output_frames) => AudioImportOutcome::Completed { output_frames },
                Err(AudioImportError::Cancelled) => AudioImportOutcome::Stopped {
                    output_frames: sent_frames.load(Ordering::Acquire),
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_mode_rejects_unbounded_output() {
        let (tx, _rx) = unbounded();
        let options = AudioImportOptions {
            pacing: AudioImportPacing::AsFastAsPossible,
            ..AudioImportOptions::default()
        };
        assert!(matches!(
            validate_options(&tx, &options),
            Err(AudioImportError::InvalidOptions(_))
        ));
    }
}
