use std::collections::VecDeque;

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

/// Bounded, interleaved resampling with delay removed and exact duration at EOF.
/// Used by file decoding and recognition preprocessing so both preserve time.
pub struct StreamingResampler {
    inner: Option<Fft<f32>>,
    source_rate: u32,
    target_rate: u32,
    channels: usize,
    pending: VecDeque<f32>,
    trim_remaining: usize,
    input_frames: u64,
    output_frames: u64,
}

impl StreamingResampler {
    pub fn new(source_rate: u32, target_rate: u32, channels: usize) -> Result<Self, String> {
        if source_rate == 0 || target_rate == 0 || channels == 0 {
            return Err("sample rates and channel count must be positive".into());
        }
        let inner = if source_rate == target_rate {
            None
        } else {
            Some(
                Fft::new(
                    source_rate as usize,
                    target_rate as usize,
                    1024,
                    channels,
                    FixedSync::Input,
                )
                .map_err(|error| error.to_string())?,
            )
        };
        let trim_remaining = inner.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            inner,
            source_rate,
            target_rate,
            channels,
            pending: VecDeque::new(),
            trim_remaining,
            input_frames: 0,
            output_frames: 0,
        })
    }

    pub fn reset(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.reset();
        }
        self.trim_remaining = self.inner.as_ref().map_or(0, Resampler::output_delay);
        self.pending.clear();
        self.input_frames = 0;
        self.output_frames = 0;
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        if !samples.len().is_multiple_of(self.channels) {
            return Err("resampler input contains a partial channel frame".into());
        }
        self.input_frames += (samples.len() / self.channels) as u64;
        if self.inner.is_none() {
            self.output_frames += (samples.len() / self.channels) as u64;
            return Ok(samples.to_vec());
        }
        self.pending.extend(samples.iter().copied());
        let mut output = Vec::new();
        while self.pending.len() >= self.inner.as_ref().unwrap().input_frames_next() * self.channels
        {
            let count = self.inner.as_ref().unwrap().input_frames_next() * self.channels;
            let input = self.pending.drain(..count).collect::<Vec<_>>();
            self.process(&input, None, None, &mut output)?;
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<f32>, String> {
        if self.inner.is_none() {
            return Ok(Vec::new());
        }
        let limit = (self.input_frames as u128 * self.target_rate as u128)
            .div_ceil(self.source_rate as u128) as u64;
        let mut output = Vec::new();
        if !self.pending.is_empty() {
            let valid = self.pending.len() / self.channels;
            let mut input = self.pending.drain(..).collect::<Vec<_>>();
            input.resize(
                self.inner.as_ref().unwrap().input_frames_next() * self.channels,
                0.0,
            );
            self.process(&input, Some(valid), Some(limit), &mut output)?;
        }
        while self.output_frames < limit {
            let input = vec![0.0; self.inner.as_ref().unwrap().input_frames_next() * self.channels];
            let before = self.output_frames;
            self.process(&input, Some(0), Some(limit), &mut output)?;
            if self.output_frames == before {
                return Err("resampler could not flush its delayed output".into());
            }
        }
        Ok(output)
    }

    fn process(
        &mut self,
        input: &[f32],
        partial: Option<usize>,
        limit: Option<u64>,
        output: &mut Vec<f32>,
    ) -> Result<(), String> {
        let inner = self.inner.as_mut().unwrap();
        let input = InterleavedSlice::new(input, self.channels, input.len() / self.channels)
            .map_err(|error| error.to_string())?;
        let capacity = inner.output_frames_max();
        let mut samples = vec![0.0; capacity * self.channels];
        let mut target = InterleavedSlice::new_mut(&mut samples, self.channels, capacity)
            .map_err(|error| error.to_string())?;
        let indexing = partial.map(|partial_len| Indexing {
            partial_len: Some(partial_len),
            ..Default::default()
        });
        let (_, written) = inner
            .process_into_buffer(&input, &mut target, indexing.as_ref())
            .map_err(|error| error.to_string())?;
        let trim = self.trim_remaining.min(written);
        self.trim_remaining -= trim;
        let keep = limit.map_or(written - trim, |limit| {
            (written - trim).min(limit.saturating_sub(self.output_frames) as usize)
        });
        self.output_frames += keep as u64;
        output.extend_from_slice(&samples[trim * self.channels..(trim + keep) * self.channels]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_packets_preserve_duration_and_channel_identity() {
        for (source, target) in [(48_000, 44_100), (44_100, 16_000), (16_000, 16_000)] {
            let frames = 13_127;
            let signal = (0..frames)
                .flat_map(|i| {
                    let sample = (i as f32 * 0.03).sin() * 0.3;
                    [sample, -sample]
                })
                .collect::<Vec<_>>();
            let mut resampler = StreamingResampler::new(source, target, 2).unwrap();
            let mut output = Vec::new();
            for packet in signal.chunks(278) {
                output.extend(resampler.push(packet).unwrap());
            }
            output.extend(resampler.finish().unwrap());
            assert_eq!(
                output.len(),
                (frames as u64 * target as u64).div_ceil(source as u64) as usize * 2
            );
            assert!(
                output
                    .chunks_exact(2)
                    .all(|frame| (frame[0] + frame[1]).abs() < 1e-5)
            );
            assert!(resampler.finish().unwrap().is_empty());
            resampler.reset();
            let mut repeated = resampler.push(&signal).unwrap();
            repeated.extend(resampler.finish().unwrap());
            assert_eq!(output, repeated);
        }
    }

    #[test]
    fn short_tail_is_not_lost() {
        for length in [1, 17, 511, 1023] {
            let mut resampler = StreamingResampler::new(44_100, 16_000, 1).unwrap();
            let mut output = resampler.push(&vec![0.25; length]).unwrap();
            output.extend(resampler.finish().unwrap());
            assert_eq!(output.len(), (length * 16_000).div_ceil(44_100));
            assert!(output.iter().all(|sample| sample.is_finite()));
        }
    }
}
