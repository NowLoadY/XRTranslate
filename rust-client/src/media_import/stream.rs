use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{SendTimeoutError, Sender};
pub(super) use xrtranslate_engine::audio::StreamingResampler;

use super::types::{AudioImportError, AudioImportPacing};

const SEND_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct ChunkSink<'a> {
    sender: Sender<Vec<f32>>,
    chunk_frames: usize,
    sample_rate: u32,
    channels: usize,
    pending: VecDeque<f32>,
    pacing: AudioImportPacing,
    pacing_started: Instant,
    stop_requested: &'a AtomicBool,
    sent_frames_counter: &'a AtomicU64,
    pub(super) sent_frames: u64,
}

impl<'a> ChunkSink<'a> {
    pub(super) fn new(
        sender: Sender<Vec<f32>>,
        chunk_frames: usize,
        sample_rate: u32,
        channels: usize,
        pacing: AudioImportPacing,
        stop_requested: &'a AtomicBool,
        sent_frames_counter: &'a AtomicU64,
    ) -> Self {
        Self {
            sender,
            chunk_frames: chunk_frames * channels,
            sample_rate,
            channels,
            pending: VecDeque::with_capacity(chunk_frames * 2),
            pacing,
            pacing_started: Instant::now(),
            stop_requested,
            sent_frames_counter,
            sent_frames: 0,
        }
    }

    pub(super) fn push(&mut self, samples: &[f32]) -> Result<(), AudioImportError> {
        self.pending.extend(samples.iter().copied());
        while self.pending.len() >= self.chunk_frames {
            let chunk = self.pending.drain(..self.chunk_frames).collect();
            self.send(chunk)?;
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), AudioImportError> {
        if !self.pending.is_empty() {
            let chunk = self.pending.drain(..).collect();
            self.send(chunk)?;
        }
        Ok(())
    }

    fn send(&mut self, mut chunk: Vec<f32>) -> Result<(), AudioImportError> {
        let chunk_frames = (chunk.len() / self.channels) as u64;
        loop {
            check_cancelled(self.stop_requested)?;
            match self.sender.send_timeout(chunk, SEND_POLL_INTERVAL) {
                Ok(()) => break,
                Err(SendTimeoutError::Timeout(returned)) => chunk = returned,
                Err(SendTimeoutError::Disconnected(_)) => {
                    return Err(AudioImportError::OutputClosed);
                }
            }
        }
        self.sent_frames += chunk_frames;
        self.sent_frames_counter
            .store(self.sent_frames, Ordering::Release);

        if self.pacing == AudioImportPacing::Realtime {
            let deadline = self.pacing_started
                + Duration::from_secs_f64(self.sent_frames as f64 / self.sample_rate as f64);
            while Instant::now() < deadline {
                check_cancelled(self.stop_requested)?;
                thread::sleep((deadline - Instant::now()).min(SEND_POLL_INTERVAL));
            }
        }
        Ok(())
    }
}

pub(super) fn duration_from_frames(frames: Option<u64>, sample_rate: u32) -> Option<Duration> {
    frames.map(|frames| Duration::from_secs_f64(frames as f64 / sample_rate as f64))
}

pub(super) fn check_cancelled(stop_requested: &AtomicBool) -> Result<(), AudioImportError> {
    if stop_requested.load(Ordering::Acquire) {
        Err(AudioImportError::Cancelled)
    } else {
        Ok(())
    }
}
