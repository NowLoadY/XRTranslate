use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use audioadapter_buffers::direct::InterleavedSlice;
use crossbeam_channel::{SendTimeoutError, Sender};
use rubato::{Fft, FixedSync, Indexing, Resampler};

use super::types::{AudioImportError, AudioImportPacing};

const RESAMPLER_INPUT_FRAMES: usize = 1024;
const SEND_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct ChunkSink<'a> {
    sender: Sender<Vec<f32>>,
    chunk_frames: usize,
    sample_rate: u32,
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
        pacing: AudioImportPacing,
        stop_requested: &'a AtomicBool,
        sent_frames_counter: &'a AtomicU64,
    ) -> Self {
        Self {
            sender,
            chunk_frames,
            sample_rate,
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
        let chunk_frames = chunk.len() as u64;
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

pub(super) enum StreamingResampler {
    Passthrough,
    Fft {
        inner: Box<Fft<f32>>,
        source_rate: u32,
        target_rate: u32,
        pending: VecDeque<f32>,
        trim_remaining: usize,
        input_frames: u64,
        output_frames: u64,
    },
}

impl StreamingResampler {
    pub(super) fn new(source_rate: u32, target_rate: u32) -> Result<Self, AudioImportError> {
        if source_rate == target_rate {
            return Ok(Self::Passthrough);
        }
        let inner = Fft::<f32>::new(
            source_rate as usize,
            target_rate as usize,
            RESAMPLER_INPUT_FRAMES,
            1,
            FixedSync::Input,
        )
        .map_err(|error| AudioImportError::Resample(error.to_string()))?;
        let trim_remaining = inner.output_delay();
        Ok(Self::Fft {
            inner: Box::new(inner),
            source_rate,
            target_rate,
            pending: VecDeque::new(),
            trim_remaining,
            input_frames: 0,
            output_frames: 0,
        })
    }

    pub(super) fn push(
        &mut self,
        samples: &[f32],
        sink: &mut ChunkSink<'_>,
    ) -> Result<(), AudioImportError> {
        match self {
            Self::Passthrough => sink.push(samples),
            Self::Fft {
                inner,
                pending,
                trim_remaining,
                input_frames,
                output_frames,
                ..
            } => {
                *input_frames += samples.len() as u64;
                pending.extend(samples.iter().copied());
                while pending.len() >= inner.input_frames_next() {
                    let input_len = inner.input_frames_next();
                    let input: Vec<f32> = pending.drain(..input_len).collect();
                    let output = process_resampler(inner, &input, None)?;
                    emit_resampled(output, trim_remaining, output_frames, None, sink)?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn finish(&mut self, sink: &mut ChunkSink<'_>) -> Result<(), AudioImportError> {
        let Self::Fft {
            inner,
            source_rate,
            target_rate,
            pending,
            trim_remaining,
            input_frames,
            output_frames,
        } = self
        else {
            return Ok(());
        };

        let expected_output =
            ((*input_frames as u128 * *target_rate as u128).div_ceil(*source_rate as u128)) as u64;
        if !pending.is_empty() {
            let valid = pending.len();
            let required = inner.input_frames_next();
            let mut input: Vec<f32> = pending.drain(..).collect();
            input.resize(required, 0.0);
            let output = process_resampler(inner, &input, Some(valid))?;
            emit_resampled(
                output,
                trim_remaining,
                output_frames,
                Some(expected_output),
                sink,
            )?;
        }

        while *output_frames < expected_output {
            let input = vec![0.0; inner.input_frames_next()];
            let output = process_resampler(inner, &input, Some(0))?;
            let before = *output_frames;
            emit_resampled(
                output,
                trim_remaining,
                output_frames,
                Some(expected_output),
                sink,
            )?;
            if *output_frames == before {
                return Err(AudioImportError::Resample(
                    "resampler could not flush its delayed output".into(),
                ));
            }
        }
        Ok(())
    }
}

fn process_resampler(
    resampler: &mut Fft<f32>,
    input: &[f32],
    partial_len: Option<usize>,
) -> Result<Vec<f32>, AudioImportError> {
    let input_adapter = InterleavedSlice::new(input, 1, input.len())
        .map_err(|error| AudioImportError::Resample(error.to_string()))?;
    let output_capacity = resampler.output_frames_max();
    let mut output = vec![0.0; output_capacity];
    let mut output_adapter = InterleavedSlice::new_mut(&mut output, 1, output_capacity)
        .map_err(|error| AudioImportError::Resample(error.to_string()))?;
    let indexing = partial_len.map(|partial_len| Indexing {
        partial_len: Some(partial_len),
        ..Indexing::default()
    });
    let (_, written) = resampler
        .process_into_buffer(&input_adapter, &mut output_adapter, indexing.as_ref())
        .map_err(|error| AudioImportError::Resample(error.to_string()))?;
    output.truncate(written);
    Ok(output)
}

fn emit_resampled(
    mut samples: Vec<f32>,
    trim_remaining: &mut usize,
    output_frames: &mut u64,
    output_limit: Option<u64>,
    sink: &mut ChunkSink<'_>,
) -> Result<(), AudioImportError> {
    if *trim_remaining > 0 {
        let trim = (*trim_remaining).min(samples.len());
        samples.drain(..trim);
        *trim_remaining -= trim;
    }
    if let Some(limit) = output_limit {
        samples.truncate(limit.saturating_sub(*output_frames) as usize);
    }
    *output_frames += samples.len() as u64;
    sink.push(&samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;

    #[test]
    fn continuous_resampling_preserves_expected_duration() {
        let stop = AtomicBool::new(false);
        let sent_frames = AtomicU64::new(0);
        let (tx, rx) = bounded(32);
        let mut sink = ChunkSink::new(
            tx,
            160,
            16_000,
            AudioImportPacing::AsFastAsPossible,
            &stop,
            &sent_frames,
        );
        let mut resampler = StreamingResampler::new(48_000, 16_000).unwrap();
        let input: Vec<f32> = (0..4_800)
            .map(|frame| ((frame as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU).sin())
            .collect();
        for packet in input.chunks(317) {
            resampler.push(packet, &mut sink).unwrap();
        }
        resampler.finish(&mut sink).unwrap();
        sink.finish().unwrap();
        drop(sink);

        let output: Vec<f32> = rx.into_iter().flatten().collect();
        assert_eq!(output.len(), 1_600);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.01));
    }
}
