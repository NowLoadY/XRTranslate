//! Bounded, sample-aligned vocal extraction. The caller owns decoding,
//! resampling, cancellation between pushes, and the managed ONNX Runtime.

#![forbid(unsafe_code)]

mod transform;

use std::{error::Error, fmt, path::Path};

use ort::{
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};
use transform::Transform;

pub const SAMPLE_RATE_HZ: u32 = 44_100;

// Original UVR profile 0ddfc0eb5792638ad5dc27850236c246 (primary stem: Vocals):
// https://github.com/Anjok07/ultimatevocalremovergui/blob/master/models/MDX_Net_Models/model_data/model_data.json
// The release's added metadata infers n_fft from the retained bin count, but
// this model was trained with a larger FFT and a cropped frequency range.
const MODEL_FFT_SIZE: usize = 6144;
const MODEL_BINS: usize = 2048;
const MODEL_FRAMES: usize = 256;
const MODEL_HOP: usize = 1024;
const VOCAL_GAIN: f32 = 1.035;

#[derive(Debug)]
pub enum SeparationError {
    Inference(ort::Error),
    InvalidModel(&'static str),
    InvalidAudio(&'static str),
}

impl fmt::Display for SeparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inference(error) => write!(f, "Vocal extraction failed: {error}"),
            Self::InvalidModel(message) => write!(f, "Invalid vocal extraction model: {message}"),
            Self::InvalidAudio(message) => write!(f, "Invalid vocal extraction audio: {message}"),
        }
    }
}

impl Error for SeparationError {}

impl From<ort::Error> for SeparationError {
    fn from(error: ort::Error) -> Self {
        Self::Inference(error)
    }
}

/// UVR-MDX stereo inference on one worker, with bounded lookahead and smooth
/// overlapping block boundaries. Inputs and outputs are interleaved stereo at
/// [`SAMPLE_RATE_HZ`]. `finish` emits the remainder without extending the timeline.
pub struct VocalSeparator {
    session: Session,
    transform: Transform,
    buffer: WindowBuffer,
}

impl VocalSeparator {
    /// The host must initialize its managed runtime before loading a model.
    pub fn load(path: &Path, threads: usize) -> Result<Self, SeparationError> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| ort::Error::new(error.to_string()))?
            .with_intra_threads(threads.max(1))
            .map_err(|error| ort::Error::new(error.to_string()))?
            .with_inter_threads(1)
            .map_err(|error| ort::Error::new(error.to_string()))?
            .with_intra_op_spinning(false)
            .map_err(|error| ort::Error::new(error.to_string()))?
            .with_inter_op_spinning(false)
            .map_err(|error| ort::Error::new(error.to_string()))?
            .commit_from_file(path)?;
        let metadata = session.metadata()?;
        let number = |name| {
            metadata
                .custom(name)
                .and_then(|value| value.parse::<usize>().ok())
        };
        if metadata.custom("model_type").as_deref() != Some("UVR")
            || metadata.custom("model_name").as_deref() != Some("UVR_MDXNET_9482")
            || number("sample_rate") != Some(SAMPLE_RATE_HZ as usize)
            || number("stems") != Some(2)
            || number("dim_c") != Some(4)
            || number("dim_f") != Some(MODEL_BINS)
            || number("dim_t") != Some(MODEL_FRAMES)
            || number("hop_length") != Some(MODEL_HOP)
            || number("center") != Some(1)
            || metadata.custom("window_type").as_deref() != Some("hann")
        {
            return Err(SeparationError::InvalidModel(
                "expected the UVR_MDXNET_9482 vocal model",
            ));
        }
        let samples = MODEL_HOP * (MODEL_FRAMES - 1);
        let overlap = SAMPLE_RATE_HZ as usize / 2;
        drop(metadata);
        Ok(Self {
            session,
            transform: Transform::new(MODEL_FFT_SIZE, MODEL_HOP, MODEL_FRAMES, MODEL_BINS),
            buffer: WindowBuffer::new(samples, MODEL_FFT_SIZE / 2, overlap),
        })
    }

    /// Feed decoded packets; check cancellation between calls. At most one
    /// model window is retained, irrespective of media duration.
    pub fn push(&mut self, stereo: &[f32]) -> Result<Vec<f32>, SeparationError> {
        let Self {
            session,
            transform,
            buffer,
        } = self;
        buffer.push(stereo, |window| infer(session, transform, window))
    }

    pub fn finish(&mut self) -> Result<Vec<f32>, SeparationError> {
        let Self {
            session,
            transform,
            buffer,
        } = self;
        buffer.finish(|window| infer(session, transform, window))
    }

    /// Start a fresh timeline while retaining the loaded model session.
    pub fn reset(&mut self) {
        self.buffer = WindowBuffer::new(self.buffer.samples, self.buffer.trim, self.buffer.overlap);
    }
}

fn infer(
    session: &mut Session,
    transform: &Transform,
    stereo: &[f32],
) -> Result<Vec<f32>, SeparationError> {
    let spectrum = model_input(transform, stereo);
    let input = Tensor::from_array(([1, 4, transform.bins(), transform.frames()], spectrum))?;
    let outputs = session.run(ort::inputs![input])?;
    let (shape, spectrum) = outputs[0].try_extract_tensor::<f32>()?;
    if shape.as_ref() != [1, 4, transform.bins() as i64, transform.frames() as i64]
        || spectrum.iter().any(|value| !value.is_finite())
    {
        return Err(SeparationError::InvalidModel(
            "unexpected vocal spectrum output",
        ));
    }
    Ok(transform
        .synthesize(spectrum)
        .into_iter()
        .map(|sample| sample * VOCAL_GAIN)
        .collect())
}

fn model_input(transform: &Transform, stereo: &[f32]) -> Vec<f32> {
    let mut spectrum = transform.analyze(stereo);
    // Match UVR's suppression of DC and the two lowest frequency bins before
    // inference, independently for both real/imaginary stereo channel planes.
    for plane in spectrum.chunks_exact_mut(transform.bins() * transform.frames()) {
        plane[..3 * transform.frames()].fill(0.0);
    }
    spectrum
}

struct WindowBuffer {
    samples: usize,
    trim: usize,
    overlap: usize,
    pending: Vec<f32>,
    tail: Vec<f32>,
    input_frames: u64,
    output_frames: u64,
    finished: bool,
}

impl WindowBuffer {
    fn new(samples: usize, trim: usize, overlap: usize) -> Self {
        let mut pending = Vec::with_capacity(samples * 2);
        pending.resize(trim * 2, 0.0);
        Self {
            samples,
            trim,
            overlap,
            pending,
            tail: Vec::new(),
            input_frames: 0,
            output_frames: 0,
            finished: false,
        }
    }

    fn step(&self) -> usize {
        self.samples - self.trim * 2 - self.overlap
    }

    fn push(
        &mut self,
        mut stereo: &[f32],
        mut infer: impl FnMut(&[f32]) -> Result<Vec<f32>, SeparationError>,
    ) -> Result<Vec<f32>, SeparationError> {
        if self.finished || stereo.len() % 2 != 0 || stereo.iter().any(|sample| !sample.is_finite())
        {
            return Err(SeparationError::InvalidAudio(
                "expected finite stereo frames in an open stream",
            ));
        }
        self.input_frames += stereo.len() as u64 / 2;
        let mut output = Vec::new();
        while !stereo.is_empty() {
            let count = (self.samples * 2 - self.pending.len()).min(stereo.len());
            self.pending.extend_from_slice(&stereo[..count]);
            stereo = &stereo[count..];
            if self.pending.len() == self.samples * 2 {
                self.process(&mut infer, self.step(), &mut output)?;
            }
        }
        Ok(output)
    }

    fn process(
        &mut self,
        infer: &mut impl FnMut(&[f32]) -> Result<Vec<f32>, SeparationError>,
        frames: usize,
        output: &mut Vec<f32>,
    ) -> Result<(), SeparationError> {
        self.pending.resize(self.samples * 2, 0.0);
        let separated = infer(&self.pending)?;
        if separated.len() != self.samples * 2 || separated.iter().any(|sample| !sample.is_finite())
        {
            return Err(SeparationError::InvalidModel("invalid reconstructed audio"));
        }
        let mut core = separated[self.trim * 2..(self.samples - self.trim) * 2].to_vec();
        for (i, (sample, previous)) in core.iter_mut().zip(&self.tail).enumerate() {
            let weight = (i / 2) as f32 / (self.overlap - 1).max(1) as f32;
            *sample = previous * (1.0 - weight) + *sample * weight;
        }
        self.tail.clear();
        self.tail.extend_from_slice(&core[self.step() * 2..]);
        output.extend_from_slice(&core[..frames * 2]);
        self.output_frames += frames as u64;
        self.pending.drain(..frames * 2);
        Ok(())
    }

    fn finish(
        &mut self,
        mut infer: impl FnMut(&[f32]) -> Result<Vec<f32>, SeparationError>,
    ) -> Result<Vec<f32>, SeparationError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let mut output = Vec::new();
        while self.output_frames < self.input_frames {
            let remaining = (self.input_frames - self.output_frames) as usize;
            if remaining <= self.tail.len() / 2 {
                output.extend_from_slice(&self.tail[..remaining * 2]);
                self.output_frames = self.input_frames;
            } else {
                self.process(&mut infer, remaining.min(self.step()), &mut output)?;
            }
        }
        self.pending.clear();
        self.tail.clear();
        self.finished = true;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_input_uses_original_frequency_spacing_and_removes_dc() {
        let transform = Transform::new(MODEL_FFT_SIZE, MODEL_HOP, MODEL_FRAMES, MODEL_BINS);
        let input = (0..MODEL_HOP * (MODEL_FRAMES - 1))
            .flat_map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / 6144.0;
                [
                    0.1 + (phase * 128.0).sin() * 0.5,
                    (phase * 256.0).sin() * 0.25,
                ]
            })
            .collect::<Vec<_>>();
        let spectrum = model_input(&transform, &input);
        let plane = MODEL_BINS * MODEL_FRAMES;
        for part in spectrum.chunks_exact(plane) {
            assert!(part[..3 * MODEL_FRAMES].iter().all(|&value| value == 0.0));
        }
        // At 44.1 kHz these tones are 918.75 and 1837.5 Hz. Cropping the
        // spectrum must not change their original 6144-point frequency grid.
        for (channel, expected_bin, expected_magnitude) in [(0, 128, 768.0), (1, 256, 384.0)] {
            let magnitude = |bin: usize| {
                let index = channel * 2 * plane + bin * MODEL_FRAMES + 80;
                spectrum[index].hypot(spectrum[index + plane])
            };
            let peak = (3..MODEL_BINS)
                .max_by(|&a, &b| magnitude(a).total_cmp(&magnitude(b)))
                .unwrap();
            assert_eq!(peak, expected_bin);
            assert!((magnitude(peak) - expected_magnitude).abs() < 0.1);
        }
    }

    #[test]
    fn chunk_boundaries_and_short_tail_preserve_every_stereo_frame() {
        for frames in [0, 1, 4, 29, 30, 31, 32, 39, 61, 65, 193] {
            let input: Vec<_> = (0..frames).flat_map(|i| [i as f32, -(i as f32)]).collect();
            for packet_frames in [1, 7, 19, 300] {
                let mut buffer = WindowBuffer::new(40, 4, 8);
                let mut output = Vec::new();
                for packet in input.chunks(packet_frames * 2) {
                    output.extend(buffer.push(packet, |x| Ok(x.to_vec())).unwrap());
                    assert!(buffer.pending.len() <= 80);
                }
                output.extend(buffer.finish(|x| Ok(x.to_vec())).unwrap());
                assert_eq!(
                    output.len(),
                    input.len(),
                    "frames={frames}, packet={packet_frames}"
                );
                assert!(
                    output
                        .iter()
                        .zip(&input)
                        .all(|(a, b)| (a - b).abs() < 0.00002)
                );
                assert!(
                    buffer
                        .finish(|_| panic!("already finished"))
                        .unwrap()
                        .is_empty()
                );
                assert!(
                    buffer
                        .push(&[0.0, 0.0], |_| panic!("already finished"))
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn reject_invalid_audio_before_changing_the_timeline() {
        let mut buffer = WindowBuffer::new(40, 4, 8);
        assert!(buffer.push(&[0.0], |_| unreachable!()).is_err());
        assert!(buffer.push(&[f32::NAN, 0.0], |_| unreachable!()).is_err());
        assert_eq!(buffer.input_frames, 0);
        assert!(buffer.finish(|_| unreachable!()).unwrap().is_empty());
    }

    #[test]
    fn overlapping_estimates_blend_without_jumps() {
        let mut buffer = WindowBuffer::new(40, 4, 8);
        let mut pass = 0;
        let output = buffer
            .push(&vec![0.0; 100 * 2], |x| {
                pass += 1;
                Ok(vec![pass as f32; x.len()])
            })
            .unwrap();
        let step = buffer.step();
        assert_eq!(output[step * 2], 1.0);
        assert_eq!(output[(step + 7) * 2], 2.0);
        assert!(
            output
                .chunks_exact(2)
                .map(|x| x[0])
                .collect::<Vec<_>>()
                .windows(2)
                .all(|x| (x[1] - x[0]).abs() < 0.15)
        );
    }

    #[test]
    #[ignore = "requires the managed UVR model and ONNX Runtime; set XRT_VOCAL_MODEL and XRT_ORT_LIBRARY"]
    fn managed_model_preserves_stereo_duration() {
        let model = std::env::var_os("XRT_VOCAL_MODEL").expect("XRT_VOCAL_MODEL");
        #[cfg(feature = "managed-ort")]
        {
            let runtime = std::env::var_os("XRT_ORT_LIBRARY").expect("XRT_ORT_LIBRARY");
            assert!(ort::init_from(runtime).unwrap().commit());
        }
        let mut separator = VocalSeparator::load(Path::new(&model), 2).unwrap();
        let input: Vec<_> = (0..SAMPLE_RATE_HZ as usize * 7 + 137)
            .flat_map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / SAMPLE_RATE_HZ as f32;
                [(phase * 220.0).sin() * 0.1, (phase * 440.0).sin() * 0.05]
            })
            .collect();
        let mut output = Vec::new();
        for packet in input.chunks(4096) {
            output.extend(separator.push(packet).unwrap());
        }
        output.extend(separator.finish().unwrap());
        assert_eq!(input.len(), output.len());
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}
