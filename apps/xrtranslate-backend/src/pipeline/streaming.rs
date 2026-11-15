//! Bounded handoff from the audio/VAD owner to the existing inference worker.
//! One live turn retains exactly the same PCM/timeline as an ordinary utterance.

use tokio::sync::{mpsc, oneshot};
use xrtranslate_vad::{EndpointConfig, FRAME_SAMPLES, SAMPLE_RATE_HZ};

use super::TimedUtterance;

#[derive(Debug)]
pub(crate) struct LiveUtterance {
    pub(crate) audio: mpsc::Receiver<Vec<u8>>,
    /// None completes an input rejected by the shared speech check; cancellation
    /// drops the sender instead. Neither may commit a recognition result.
    pub(crate) finished: oneshot::Receiver<Option<TimedUtterance>>,
    pub(crate) source_start_ms: f64,
}

pub(super) struct LiveCapture {
    audio: mpsc::Sender<Vec<u8>>,
    finished: oneshot::Sender<Option<TimedUtterance>>,
}

impl LiveCapture {
    pub(super) fn begin(
        config: EndpointConfig,
        onset: &[i16],
        processed_samples: u64,
    ) -> Result<(Self, LiveUtterance), String> {
        // The detector bounds each turn. Even while waiting for a model slot,
        // its complete turn fits without blocking microphone/control intake.
        let capacity = config.max_active_frames
            + config.pre_roll_frames
            + config.max_active_overlap_frames
            + config.opening_window_frames
            + 2;
        let (audio, receiver) = mpsc::channel(capacity);
        let (finished, completion) = oneshot::channel();
        let capture = Self { audio, finished };
        capture.push(onset)?;
        Ok((
            capture,
            LiveUtterance {
                audio: receiver,
                finished: completion,
                source_start_ms: processed_samples.saturating_sub(onset.len() as u64) as f64
                    * 1000.0
                    / f64::from(SAMPLE_RATE_HZ),
            },
        ))
    }

    pub(super) fn push(&self, samples: &[i16]) -> Result<(), String> {
        // Split the onset as well, so queue capacity is measured in VAD frames.
        for frame in samples.chunks(FRAME_SAMPLES) {
            match self.audio.try_send(
                frame
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect(),
            ) {
                Ok(()) => {}
                // A failed request drops its receiver. Finish recording this
                // turn normally so the next utterance gets a fresh request.
                Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                Err(error) => return Err(format!("Live ASR audio delivery failed: {error}")),
            }
        }
        Ok(())
    }

    pub(super) fn finish(self, utterance: Option<TimedUtterance>) {
        let _ = self.finished.send(utterance);
        // Dropping audio signals EOF after every queued frame. Reset/cancel
        // also drops completion, which cannot be mistaken for a successful end.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_recognition_does_not_stop_capture_of_the_next_turn() {
        let frame = vec![1; FRAME_SAMPLES];
        let (capture, turn) = LiveCapture::begin(EndpointConfig::default(), &frame, 0).unwrap();
        drop(turn);
        capture.push(&frame).unwrap();
        capture.finish(None);

        let (capture, mut next) = LiveCapture::begin(EndpointConfig::default(), &frame, 0).unwrap();
        capture.push(&frame).unwrap();
        assert_eq!(next.audio.try_recv().unwrap().len(), FRAME_SAMPLES * 2);
        assert_eq!(next.audio.try_recv().unwrap().len(), FRAME_SAMPLES * 2);
        capture.finish(None);
        assert!(next.finished.try_recv().unwrap().is_none());
    }
}
