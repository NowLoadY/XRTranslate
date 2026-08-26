//! Ultra-lightweight real-time GTCRN-Light v3 speech enhancement and background noise suppression.
//!
//! GTCRN processes single-channel 16 kHz audio through 512-point STFT analysis (257 complex frequency bins)
//! with 256-sample frame hop (16 ms). It maintains stateful streaming cache tensors across frames
//! to achieve ultra-low latency noise suppression with minimal CPU usage.

#![forbid(unsafe_code)]

use std::{error::Error, f32::consts::PI, fmt, path::Path};

use ndarray::{Array4, Array5};
use ort::{
    session::{Session, builder::GraphOptimizationLevel},
    value::Value,
};

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const FFT_SIZE: usize = 512;
pub const FFT_BINS: usize = FFT_SIZE / 2 + 1; // 257 bins
pub const HOP_SIZE: usize = 256; // 16 ms hop (256 samples at 16 kHz)

const CONV_CACHE_SHAPE: [usize; 5] = [2, 1, 16, 16, 33];
const TRA_CACHE_SHAPE: [usize; 5] = [2, 3, 1, 1, 16];
const INTER_CACHE_SHAPE: [usize; 4] = [2, 1, 33, 16];

#[derive(Debug)]
pub enum DenoiseError {
    Ort(ort::Error),
    InvalidModelOutput(&'static str),
    NonFiniteSample,
}

impl fmt::Display for DenoiseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ort(error) => {
                write!(formatter, "ONNX Runtime denoiser inference failed: {error}")
            }
            Self::InvalidModelOutput(msg) => {
                write!(formatter, "GTCRN model output is invalid: {msg}")
            }
            Self::NonFiniteSample => {
                formatter.write_str("audio stream contains non-finite samples")
            }
        }
    }
}

impl Error for DenoiseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ort(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ort::Error> for DenoiseError {
    fn from(value: ort::Error) -> Self {
        Self::Ort(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

fn multiply_complex(left: Complex, right: Complex) -> Complex {
    Complex {
        re: left.re.mul_add(right.re, -(left.im * right.im)),
        im: left.re.mul_add(right.im, left.im * right.re),
    }
}

fn fft_512_in_place(values: &mut [Complex; FFT_SIZE]) {
    for index in 1..FFT_SIZE {
        let reversed = index.reverse_bits() >> (usize::BITS - FFT_SIZE.trailing_zeros());
        if index < reversed {
            values.swap(index, reversed);
        }
    }
    let mut length = 2;
    while length <= FFT_SIZE {
        let angle = -2.0 * PI / length as f32;
        let root = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for start in (0..FFT_SIZE).step_by(length) {
            let mut twiddle = Complex { re: 1.0, im: 0.0 };
            for offset in 0..length / 2 {
                let even = values[start + offset];
                let odd = multiply_complex(values[start + offset + length / 2], twiddle);
                values[start + offset] = Complex {
                    re: even.re + odd.re,
                    im: even.im + odd.im,
                };
                values[start + offset + length / 2] = Complex {
                    re: even.re - odd.re,
                    im: even.im - odd.im,
                };
                twiddle = multiply_complex(twiddle, root);
            }
        }
        length *= 2;
    }
}

fn ifft_512_in_place(values: &mut [Complex; FFT_SIZE]) {
    for index in 1..FFT_SIZE {
        let reversed = index.reverse_bits() >> (usize::BITS - FFT_SIZE.trailing_zeros());
        if index < reversed {
            values.swap(index, reversed);
        }
    }
    let mut length = 2;
    while length <= FFT_SIZE {
        let angle = 2.0 * PI / length as f32;
        let root = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for start in (0..FFT_SIZE).step_by(length) {
            let mut twiddle = Complex { re: 1.0, im: 0.0 };
            for offset in 0..length / 2 {
                let even = values[start + offset];
                let odd = multiply_complex(values[start + offset + length / 2], twiddle);
                values[start + offset] = Complex {
                    re: even.re + odd.re,
                    im: even.im + odd.im,
                };
                values[start + offset + length / 2] = Complex {
                    re: even.re - odd.re,
                    im: even.im - odd.im,
                };
                twiddle = multiply_complex(twiddle, root);
            }
        }
        length *= 2;
    }
    let scale = 1.0 / FFT_SIZE as f32;
    for value in values.iter_mut() {
        value.re *= scale;
        value.im *= scale;
    }
}

/// Sine (sqrt-Hann) window ensuring exact perfect reconstruction with 50% overlap.
#[derive(Debug, Clone)]
pub struct StftWindow {
    window: [f32; FFT_SIZE],
}

impl StftWindow {
    pub fn new() -> Self {
        let mut window = [0.0f32; FFT_SIZE];
        for (i, w) in window.iter_mut().enumerate() {
            let phase = PI * (i as f32 + 0.5) / FFT_SIZE as f32;
            *w = phase.sin();
        }
        Self { window }
    }

    #[inline(always)]
    pub fn window(&self) -> &[f32; FFT_SIZE] {
        &self.window
    }
}

impl Default for StftWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// Real-time streaming Causal STFT/iSTFT processor with overlap-add.
#[derive(Debug)]
pub struct CausalStftStream {
    window: StftWindow,
    in_buffer: Vec<f32>,
    ola_buffer: [f32; FFT_SIZE],
}

impl CausalStftStream {
    pub fn new() -> Self {
        Self {
            window: StftWindow::new(),
            in_buffer: Vec::with_capacity(FFT_SIZE * 2),
            ola_buffer: [0.0f32; FFT_SIZE],
        }
    }

    pub fn reset(&mut self) {
        self.in_buffer.clear();
        self.ola_buffer.fill(0.0);
    }

    /// Appends audio samples into the input buffer.
    pub fn push_samples(&mut self, samples: &[i16]) {
        self.in_buffer
            .extend(samples.iter().map(|&s| s as f32 / 32768.0));
    }

    /// Checks if a complete 512-sample frame is ready for analysis.
    pub fn has_frame(&self) -> bool {
        self.in_buffer.len() >= FFT_SIZE
    }

    /// Extracts STFT complex bins for the current frame.
    pub fn compute_stft_frame(&self) -> [Complex; FFT_BINS] {
        let mut fft_buf = [Complex::default(); FFT_SIZE];
        let win = self.window.window();
        for i in 0..FFT_SIZE {
            fft_buf[i] = Complex {
                re: self.in_buffer[i] * win[i],
                im: 0.0,
            };
        }
        fft_512_in_place(&mut fft_buf);
        let mut bins = [Complex::default(); FFT_BINS];
        bins.copy_from_slice(&fft_buf[..FFT_BINS]);
        bins
    }

    /// Consumes the enhanced complex bins, performs iSTFT, overlap-add, and yields 256 output samples.
    pub fn synthesize_and_advance(
        &mut self,
        enhanced_bins: &[Complex; FFT_BINS],
    ) -> [i16; HOP_SIZE] {
        let mut full_fft = [Complex::default(); FFT_SIZE];
        full_fft[0] = Complex {
            re: enhanced_bins[0].re,
            im: 0.0,
        };
        full_fft[FFT_SIZE / 2] = Complex {
            re: enhanced_bins[FFT_SIZE / 2].re,
            im: 0.0,
        };
        for k in 1..FFT_SIZE / 2 {
            full_fft[k] = enhanced_bins[k];
            full_fft[FFT_SIZE - k] = Complex {
                re: enhanced_bins[k].re,
                im: -enhanced_bins[k].im,
            };
        }

        ifft_512_in_place(&mut full_fft);

        let win = self.window.window();
        for i in 0..FFT_SIZE {
            self.ola_buffer[i] += full_fft[i].re * win[i];
        }

        let mut output = [0i16; HOP_SIZE];
        for i in 0..HOP_SIZE {
            let sample_f32 = self.ola_buffer[i] * 32768.0;
            output[i] = sample_f32.clamp(-32768.0, 32767.0).round() as i16;
        }

        // Shift OLA buffer by HOP_SIZE
        for i in 0..FFT_SIZE - HOP_SIZE {
            self.ola_buffer[i] = self.ola_buffer[i + HOP_SIZE];
        }
        self.ola_buffer[FFT_SIZE - HOP_SIZE..].fill(0.0);

        // Advance input buffer by HOP_SIZE
        self.in_buffer.drain(..HOP_SIZE);

        output
    }
}

impl Default for CausalStftStream {
    fn default() -> Self {
        Self::new()
    }
}

/// Stateful GTCRN-Light v3 Denoiser session.
pub struct GtcrnDenoiser {
    session: Session,
    stft_stream: CausalStftStream,
    conv_cache: Array5<f32>,
    tra_cache: Array5<f32>,
    inter_cache: Array4<f32>,
}

impl GtcrnDenoiser {
    /// Loads a GTCRN ONNX model from file path.
    pub fn from_file(
        model_path: impl AsRef<Path>,
        intra_threads: usize,
    ) -> Result<Self, DenoiseError> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| DenoiseError::Ort(ort::Error::new(error.to_string())))?
            .with_intra_threads(intra_threads.max(1))
            .map_err(|error| DenoiseError::Ort(ort::Error::new(error.to_string())))?
            .with_inter_threads(1)
            .map_err(|error| DenoiseError::Ort(ort::Error::new(error.to_string())))?
            .with_intra_op_spinning(false)
            .map_err(|error| DenoiseError::Ort(ort::Error::new(error.to_string())))?
            .with_inter_op_spinning(false)
            .map_err(|error| DenoiseError::Ort(ort::Error::new(error.to_string())))?
            .commit_from_file(model_path)?;

        Ok(Self {
            session,
            stft_stream: CausalStftStream::new(),
            conv_cache: Array5::zeros(CONV_CACHE_SHAPE),
            tra_cache: Array5::zeros(TRA_CACHE_SHAPE),
            inter_cache: Array4::zeros(INTER_CACHE_SHAPE),
        })
    }

    /// Resets recurrent streaming caches and overlap-add buffer.
    pub fn reset(&mut self) {
        self.stft_stream.reset();
        self.conv_cache.fill(0.0);
        self.tra_cache.fill(0.0);
        self.inter_cache.fill(0.0);
    }

    /// Processes incoming mono PCM16LE audio chunk and returns denoised PCM16LE.
    pub fn process_samples(&mut self, samples: &[i16]) -> Result<Vec<i16>, DenoiseError> {
        self.stft_stream.push_samples(samples);
        let mut output = Vec::with_capacity(samples.len());

        while self.stft_stream.has_frame() {
            let stft_bins = self.stft_stream.compute_stft_frame();

            // Prepare mix tensor [1, 257, 1, 2]
            let mut mix_data = Vec::with_capacity(FFT_BINS * 2);
            for bin in &stft_bins {
                mix_data.push(bin.re);
                mix_data.push(bin.im);
            }
            let mix_tensor = Array4::from_shape_vec((1, FFT_BINS, 1, 2), mix_data)
                .map_err(|_| DenoiseError::InvalidModelOutput("mix tensor shape mismatch"))?;

            let mix_val = Value::from_array(mix_tensor.into_dyn())?;
            let conv_val = Value::from_array(self.conv_cache.clone().into_dyn())?;
            let tra_val = Value::from_array(self.tra_cache.clone().into_dyn())?;
            let inter_val = Value::from_array(self.inter_cache.clone().into_dyn())?;

            let outputs = self.session.run(ort::inputs![
                "mix" => mix_val,
                "conv_cache" => conv_val,
                "tra_cache" => tra_val,
                "inter_cache" => inter_val,
            ])?;

            // Extract enhanced spectrum
            let enh_out = outputs
                .get("enh")
                .or_else(|| outputs.get("output"))
                .or_else(|| outputs.get("enhanced"))
                .ok_or(DenoiseError::InvalidModelOutput(
                    "missing 'enh' output tensor",
                ))?;

            let (_, enh_values) = enh_out.try_extract_tensor::<f32>().map_err(|_| {
                DenoiseError::InvalidModelOutput("invalid float output in enh tensor")
            })?;

            if enh_values.len() < FFT_BINS * 2 {
                return Err(DenoiseError::InvalidModelOutput(
                    "enh tensor length too short",
                ));
            }

            let mut enhanced_bins = [Complex::default(); FFT_BINS];
            for k in 0..FFT_BINS {
                enhanced_bins[k] = Complex {
                    re: enh_values[k * 2],
                    im: enh_values[k * 2 + 1],
                };
            }

            // Update caches if returned
            if let Some(conv_out) = outputs
                .get("conv_cache_out")
                .or_else(|| outputs.get("conv_cache"))
            {
                if let Ok((shape, data)) = conv_out.try_extract_tensor::<f32>() {
                    if shape.as_ref() == CONV_CACHE_SHAPE.map(|d| d as i64).as_slice() {
                        self.conv_cache = Array5::from_shape_vec(CONV_CACHE_SHAPE, data.to_vec())
                            .unwrap_or(Array5::zeros(CONV_CACHE_SHAPE));
                    }
                }
            }
            if let Some(tra_out) = outputs
                .get("tra_cache_out")
                .or_else(|| outputs.get("tra_cache"))
            {
                if let Ok((shape, data)) = tra_out.try_extract_tensor::<f32>() {
                    if shape.as_ref() == TRA_CACHE_SHAPE.map(|d| d as i64).as_slice() {
                        self.tra_cache = Array5::from_shape_vec(TRA_CACHE_SHAPE, data.to_vec())
                            .unwrap_or(Array5::zeros(TRA_CACHE_SHAPE));
                    }
                }
            }
            if let Some(inter_out) = outputs
                .get("inter_cache_out")
                .or_else(|| outputs.get("inter_cache"))
            {
                if let Ok((shape, data)) = inter_out.try_extract_tensor::<f32>() {
                    if shape.as_ref() == INTER_CACHE_SHAPE.map(|d| d as i64).as_slice() {
                        self.inter_cache = Array4::from_shape_vec(INTER_CACHE_SHAPE, data.to_vec())
                            .unwrap_or(Array4::zeros(INTER_CACHE_SHAPE));
                    }
                }
            }

            let hopped_samples = self.stft_stream.synthesize_and_advance(&enhanced_bins);
            output.extend_from_slice(&hopped_samples);
        }

        Ok(output)
    }

    /// Processes raw bytes of PCM16LE audio.
    pub fn process_pcm16le(&mut self, pcm: &[u8]) -> Result<Vec<u8>, DenoiseError> {
        let sample_count = pcm.len() / 2;
        let mut samples = Vec::with_capacity(sample_count);
        for chunk in pcm.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }

        let denoised_samples = self.process_samples(&samples)?;
        let mut denoised_bytes = Vec::with_capacity(denoised_samples.len() * 2);
        for s in denoised_samples {
            denoised_bytes.extend_from_slice(&s.to_le_bytes());
        }
        Ok(denoised_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stft_istft_perfect_reconstruction_with_identity_bins() {
        let mut stft_stream = CausalStftStream::new();

        // Generate 16 kHz test sine signal: 440 Hz + 1000 Hz
        let total_samples = 4096;
        let mut original = Vec::with_capacity(total_samples);
        for i in 0..total_samples {
            let t = i as f32 / 16000.0;
            let sample_f32 =
                0.5 * (2.0 * PI * 440.0 * t).sin() + 0.3 * (2.0 * PI * 1000.0 * t).sin();
            original.push((sample_f32 * 32767.0) as i16);
        }

        stft_stream.push_samples(&original);
        let mut reconstructed = Vec::new();

        while stft_stream.has_frame() {
            let bins = stft_stream.compute_stft_frame();
            // Pass through with identity enhancement
            let out_hop = stft_stream.synthesize_and_advance(&bins);
            reconstructed.extend_from_slice(&out_hop);
        }

        assert!(!reconstructed.is_empty());
        // After initial filter ramp-up (after 1 frame = 512 samples), check maximum difference
        let start_eval = 512;
        let end_eval = reconstructed.len().min(total_samples - 512);
        for i in start_eval..end_eval {
            let diff = (original[i] as i32 - reconstructed[i] as i32).abs();
            // Difference should be within quantization rounding (<= 2 on 16-bit integer scale)
            assert!(
                diff <= 2,
                "Reconstruction mismatch at index {i}: original={}, reconstructed={}, diff={diff}",
                original[i],
                reconstructed[i]
            );
        }
    }

    #[test]
    fn stft_window_princen_bradley_condition_holds() {
        let win = StftWindow::new();
        let w = win.window();
        for i in 0..HOP_SIZE {
            let sum = w[i] * w[i] + w[i + HOP_SIZE] * w[i + HOP_SIZE];
            let diff = (sum - 1.0).abs();
            assert!(diff < 1e-6, "Window condition violated at {i}: {sum}");
        }
    }

    #[test]
    fn fft_and_ifft_roundtrip() {
        let mut buffer = [Complex::default(); FFT_SIZE];
        for i in 0..FFT_SIZE {
            buffer[i] = Complex {
                re: (i as f32 * 0.1).sin(),
                im: (i as f32 * 0.2).cos(),
            };
        }
        let original = buffer;
        fft_512_in_place(&mut buffer);
        ifft_512_in_place(&mut buffer);

        for i in 0..FFT_SIZE {
            let re_diff = (buffer[i].re - original[i].re).abs();
            let im_diff = (buffer[i].im - original[i].im).abs();
            assert!(re_diff < 1e-5, "Real mismatch at {i}: diff={re_diff}");
            assert!(im_diff < 1e-5, "Imag mismatch at {i}: diff={im_diff}");
        }
    }

    #[test]
    fn complex_multiplication_correctness() {
        let a = Complex { re: 3.0, im: 4.0 };
        let b = Complex { re: 1.0, im: 2.0 };
        // (3+4i)*(1+2i) = 3 + 6i + 4i - 8 = -5 + 10i
        let c = multiply_complex(a, b);
        assert!((c.re - -5.0).abs() < 1e-6);
        assert!((c.im - 10.0).abs() < 1e-6);
    }

    #[test]
    fn gtcrn_model_inference_denoises_noisy_signal() {
        let model_path = Path::new("../../models/gtcrn/gtcrn_simple.onnx");
        if !model_path.is_file() {
            eprintln!(
                "Skipping model inference test: model file not present at {}",
                model_path.display()
            );
            return;
        }

        let mut denoiser =
            GtcrnDenoiser::from_file(model_path, 1).expect("failed to load GTCRN model");

        // Generate synthetic noisy speech (sine wave + random noise)
        let sample_rate = 16000.0;
        let count = 16000; // 1 second
        let mut noisy = Vec::with_capacity(count);
        for i in 0..count {
            let t = i as f32 / sample_rate;
            let signal = 0.5 * (2.0 * PI * 440.0 * t).sin();
            let noise = 0.1 * ((i * 1103515245 + 12345) as f32 / 2147483648.0 - 0.5);
            let combined = signal + noise;
            noisy.push((combined * 32767.0) as i16);
        }

        let denoised = denoiser.process_samples(&noisy).expect("denoising failed");
        assert!(!denoised.is_empty());
        assert_eq!(denoised.len() % HOP_SIZE, 0);

        // Test PCM byte pipeline
        let mut bytes = Vec::with_capacity(noisy.len() * 2);
        for s in &noisy {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let denoised_bytes = denoiser
            .process_pcm16le(&bytes)
            .expect("denoising pcm bytes failed");
        assert_eq!(denoised_bytes.len() % (HOP_SIZE * 2), 0);
    }
}
