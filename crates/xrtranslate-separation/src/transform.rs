use std::{f32::consts::PI, sync::Arc};

use rustfft::{Fft, FftPlanner, num_complex::Complex32};

/// UVR's centered periodic-Hann STFT: separate real/imaginary planes for each
/// stereo channel, frequency-major layout, and a cropped model frequency range.
pub(super) struct Transform {
    size: usize,
    hop: usize,
    frames: usize,
    bins: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

impl Transform {
    pub(super) fn new(size: usize, hop: usize, frames: usize, bins: usize) -> Self {
        let mut planner = FftPlanner::new();
        Self {
            size,
            hop,
            frames,
            bins,
            window: (0..size)
                .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / size as f32).cos())
                .collect(),
            forward: planner.plan_fft_forward(size),
            inverse: planner.plan_fft_inverse(size),
        }
    }

    pub(super) fn bins(&self) -> usize {
        self.bins
    }
    pub(super) fn frames(&self) -> usize {
        self.frames
    }
    fn samples(&self) -> usize {
        self.hop * (self.frames - 1)
    }

    pub(super) fn analyze(&self, stereo: &[f32]) -> Vec<f32> {
        let bins = self.bins();
        let plane = bins * self.frames;
        let mut output = vec![0.0; 4 * plane];
        let mut buffer = vec![Complex32::default(); self.size];
        let mut scratch = vec![Complex32::default(); self.forward.get_inplace_scratch_len()];
        for channel in 0..2 {
            for frame in 0..self.frames {
                let start = frame as isize * self.hop as isize - (self.size / 2) as isize;
                for (i, value) in buffer.iter_mut().enumerate() {
                    let index = start + i as isize;
                    // Center padding matches the reference reflect-mode STFT.
                    let index = if index < 0 {
                        -index
                    } else if index >= self.samples() as isize {
                        self.samples() as isize * 2 - 2 - index
                    } else {
                        index
                    } as usize;
                    *value = Complex32::new(stereo[index * 2 + channel] * self.window[i], 0.0);
                }
                self.forward.process_with_scratch(&mut buffer, &mut scratch);
                for (bin, value) in buffer.iter().take(bins).enumerate() {
                    let index = channel * 2 * plane + bin * self.frames + frame;
                    output[index] = value.re;
                    output[index + plane] = value.im;
                }
            }
        }
        output
    }

    pub(super) fn synthesize(&self, spectrum: &[f32]) -> Vec<f32> {
        let bins = self.bins();
        let plane = bins * self.frames;
        let samples = self.samples();
        let mut output = vec![0.0; samples * 2];
        let mut weights = vec![0.0; samples];
        let mut buffer = vec![Complex32::default(); self.size];
        let mut scratch = vec![Complex32::default(); self.inverse.get_inplace_scratch_len()];
        for channel in 0..2 {
            for frame in 0..self.frames {
                // The model returns only its retained low-frequency bins.
                // Clear all omitted bins, including the Nyquist frequency.
                buffer.fill(Complex32::default());
                for bin in 0..bins {
                    let index = channel * 2 * plane + bin * self.frames + frame;
                    buffer[bin] = Complex32::new(spectrum[index], spectrum[index + plane]);
                    if bin > 0 {
                        buffer[self.size - bin] = buffer[bin].conj();
                    }
                }
                self.inverse.process_with_scratch(&mut buffer, &mut scratch);
                let start = frame as isize * self.hop as isize - (self.size / 2) as isize;
                for (i, value) in buffer.iter().enumerate() {
                    let position = start + i as isize;
                    if position >= 0 && position < samples as isize {
                        let position = position as usize;
                        output[position * 2 + channel] +=
                            value.re * self.window[i] / self.size as f32;
                        if channel == 0 {
                            weights[position] += self.window[i].powi(2);
                        }
                    }
                }
            }
        }
        for (frame, weight) in output.chunks_exact_mut(2).zip(weights) {
            if weight > f32::EPSILON {
                frame[0] /= weight;
                frame[1] /= weight;
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_roundtrip_preserves_stereo_gain_and_time() {
        let transform = Transform::new(6144, 1024, 256, 2048);
        let input: Vec<_> = (0..transform.samples())
            .flat_map(|i| {
                let phase = 2.0 * PI * i as f32 / 44100.0;
                [(phase * 220.0).sin() * 0.7, (phase * 440.0).sin() * 0.2]
            })
            .collect();
        let output = transform.synthesize(&transform.analyze(&input));
        assert_eq!(input.len(), output.len());
        let rms = (input
            .iter()
            .zip(&output)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / input.len() as f32)
            .sqrt();
        assert!(rms < 0.0001, "roundtrip RMS error: {rms}");
        assert!(output.iter().all(|x| x.is_finite()));
    }
}
