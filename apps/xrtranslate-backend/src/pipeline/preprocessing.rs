//! Optional input preprocessing for the shared recognition pipeline. Each
//! adapter emits sample-aligned mono 16 kHz PCM before ordinary endpointing.

use std::path::Path;

use xrtranslate_denoise::GtcrnDenoiser;
use xrtranslate_engine::audio::{StreamingResampler, f32_to_pcm16le};
use xrtranslate_separation::{SAMPLE_RATE_HZ as VOCAL_SAMPLE_RATE_HZ, VocalSeparator};

#[derive(Default)]
pub(super) enum AudioPreprocessor {
    #[default]
    None,
    Gtcrn(GtcrnDenoiser),
    Vocals(VocalPreprocessor),
}

impl AudioPreprocessor {
    pub(super) fn gtcrn(path: &Path, threads: usize) -> Result<Self, String> {
        GtcrnDenoiser::from_file(path, threads)
            .map(Self::Gtcrn)
            .map_err(|error| error.to_string())
    }

    pub(super) fn vocals(path: &Path, threads: usize) -> Result<Self, String> {
        Ok(Self::Vocals(VocalPreprocessor {
            separator: VocalSeparator::load(path, threads).map_err(|error| error.to_string())?,
            resampler: StreamingResampler::new(VOCAL_SAMPLE_RATE_HZ, 16_000, 1)?,
        }))
    }

    /// The first output is recognition PCM. The optional second output is
    /// delayed detection-only PCM; GTCRN keeps the original audio for ASR.
    pub(super) fn push(&mut self, pcm: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        if !pcm.len().is_multiple_of(2) {
            return Err("PCM16LE audio must contain whole samples".into());
        }
        match self {
            Self::None => Ok((pcm.to_vec(), None)),
            Self::Gtcrn(denoiser) => Ok((
                pcm.to_vec(),
                Some(
                    denoiser
                        .process_pcm16le(pcm)
                        .map_err(|error| error.to_string())?,
                ),
            )),
            Self::Vocals(vocals) => {
                if !pcm.len().is_multiple_of(4) {
                    return Err("Vocal extraction requires whole stereo PCM16LE frames".into());
                }
                let stereo = pcm
                    .chunks_exact(2)
                    .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
                    .collect::<Vec<_>>();
                let separated = vocals
                    .separator
                    .push(&stereo)
                    .map_err(|error| error.to_string())?;
                Ok((vocals.resample(&separated)?, None))
            }
        }
    }

    pub(super) fn finish(&mut self) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        match self {
            Self::None => Ok((Vec::new(), None)),
            Self::Gtcrn(denoiser) => {
                // Padding drains delayed detection audio only. AudioBuffer
                // trims it to the number of real recognition samples.
                let tail = denoiser
                    .process_pcm16le(&[0; xrtranslate_denoise::FFT_SIZE * 2])
                    .map_err(|error| error.to_string())?;
                Ok((Vec::new(), Some(tail)))
            }
            Self::Vocals(vocals) => {
                let tail = vocals
                    .separator
                    .finish()
                    .map_err(|error| error.to_string())?;
                let mut pcm = vocals.resample(&tail)?;
                pcm.extend(f32_to_pcm16le(vocals.resampler.finish()?));
                Ok((pcm, None))
            }
        }
    }

    pub(super) fn reset(&mut self) {
        match self {
            Self::None => {}
            Self::Gtcrn(denoiser) => denoiser.reset(),
            Self::Vocals(vocals) => {
                vocals.separator.reset();
                vocals.resampler.reset();
            }
        }
    }

    pub(super) fn is_vocal(&self) -> bool {
        matches!(self, Self::Vocals(_))
    }
}

pub(super) struct VocalPreprocessor {
    separator: VocalSeparator,
    resampler: StreamingResampler,
}

impl VocalPreprocessor {
    fn resample(&mut self, stereo: &[f32]) -> Result<Vec<u8>, String> {
        let mono = stereo
            .chunks_exact(2)
            .map(|frame| (frame[0] + frame[1]) * 0.5)
            .collect::<Vec<_>>();
        self.resampler.push(&mono).map(f32_to_pcm16le)
    }
}

/// Candidate activity on the separated vocal track, not a probability that
/// the signal contains words. Endpointing and minimum-duration admission remain
/// shared with speech; residual instruments can still require an empty ASR result.
#[derive(Default)]
pub(super) struct VocalActivity {
    active: bool,
}

impl VocalActivity {
    pub(super) fn evidence(&mut self, samples: &[i16], speech_probability: f32) -> f32 {
        if samples.is_empty() {
            self.reset();
            return 0.0;
        }
        // Centered RMS ignores DC offset without suppressing sustained vowels.
        let mean = samples.iter().map(|&sample| sample as f64).sum::<f64>() / samples.len() as f64;
        let power = samples
            .iter()
            .map(|&sample| (sample as f64 - mean).powi(2))
            .sum::<f64>()
            / samples.len() as f64
            / 32768.0_f64.powi(2);
        // -48 dB onset, -54 dB continuation, -60 dB floor for speech evidence.
        // No added hangover: the existing endpoint detector owns pause timing.
        self.active = power
            >= if self.active {
                0.0000039810717
            } else {
                0.000015848932
            };
        if self.active {
            1.0
        } else if power >= 0.000001 && speech_probability.is_finite() {
            speech_probability.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub(super) fn reset(&mut self) {
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_vad::{EndpointConfig, EndpointDetector, EndpointEvent, FRAME_SAMPLES};

    fn tone(amplitude: f32) -> [i16; FRAME_SAMPLES] {
        std::array::from_fn(|i| {
            ((i as f32 * std::f32::consts::TAU / 32.0).sin() * amplitude * 32768.0) as i16
        })
    }

    #[test]
    fn singing_activity_survives_zero_speech_probability_and_weak_tails() {
        let mut activity = VocalActivity::default();
        assert_eq!(activity.evidence(&tone(0.01), 0.0), 1.0);
        assert_eq!(activity.evidence(&tone(0.004), 0.0), 1.0);
        assert_eq!(activity.evidence(&[0; FRAME_SAMPLES], 1.0), 0.0);
        assert_eq!(activity.evidence(&tone(0.004), 0.0), 0.0);
        assert_eq!(activity.evidence(&[1000; FRAME_SAMPLES], 1.0), 0.0);
        assert_eq!(activity.evidence(&tone(0.002), 0.9), 0.9);
        activity.reset();
        assert_eq!(activity.evidence(&tone(0.004), 0.0), 0.0);
    }

    #[test]
    fn isolated_impulse_cannot_open_the_shared_endpoint() {
        let mut activity = VocalActivity::default();
        let mut endpoint = EndpointDetector::new(EndpointConfig::default()).unwrap();
        let mut impulse = [0; FRAME_SAMPLES];
        impulse[100] = i16::MAX;
        for samples in std::iter::once(impulse).chain(std::iter::repeat_n([0; FRAME_SAMPLES], 30)) {
            assert!(matches!(
                endpoint
                    .push(&samples, activity.evidence(&samples, 0.0))
                    .unwrap(),
                EndpointEvent::Listening
            ));
        }
        assert!(endpoint.flush().is_none());
    }

    #[test]
    fn short_pause_and_eof_keep_one_complete_singing_segment() {
        let mut activity = VocalActivity::default();
        let mut endpoint = EndpointDetector::new(EndpointConfig::default()).unwrap();
        let mut gate = super::super::audio::SpeechGate::new(100);
        let mut started = 0;
        for samples in std::iter::repeat_n(tone(0.02), 20)
            .chain(std::iter::repeat_n([0; FRAME_SAMPLES], 8))
            .chain(std::iter::repeat_n(tone(0.01), 20))
        {
            let evidence = activity.evidence(&samples, 0.0);
            gate.observe(evidence, samples.len());
            match endpoint.push(&samples, evidence).unwrap() {
                EndpointEvent::SpeechStarted => started += 1,
                EndpointEvent::Finalized(_) => panic!("short pause split a singing phrase"),
                _ => {}
            }
        }
        let utterance = endpoint.flush().expect("EOF retains active singing");
        assert_eq!(started, 1);
        assert!(gate.accept(&utterance, 0.5));
        assert!(utterance.samples.len() >= 46 * FRAME_SAMPLES);
    }

    #[test]
    fn bypass_keeps_original_pcm_and_rejects_partial_samples() {
        let mut processor = AudioPreprocessor::None;
        let input = [0, 0, 255, 127, 0, 128];
        assert_eq!(processor.push(&input).unwrap(), (input.to_vec(), None));
        assert!(processor.push(&[0]).is_err());
        assert_eq!(processor.finish().unwrap(), (Vec::new(), None));
        processor.reset();
        assert!(!processor.is_vocal());
    }

    #[test]
    #[ignore = "requires managed UVR resources; set XRT_VOCAL_MODEL and XRT_ORT_LIBRARY"]
    fn managed_vocals_flush_and_reset_preserve_the_recognition_timeline() {
        let model = std::env::var_os("XRT_VOCAL_MODEL").expect("XRT_VOCAL_MODEL");
        let runtime = std::env::var_os("XRT_ORT_LIBRARY").expect("XRT_ORT_LIBRARY");
        xrtranslate_inference::initialize_onnx_runtime(Path::new(&runtime)).unwrap();
        let mut processor = AudioPreprocessor::vocals(Path::new(&model), 2).unwrap();
        assert!(processor.is_vocal());
        assert!(processor.push(&[0, 0]).is_err());
        assert!(processor.push(&[0; 100]).unwrap().0.is_empty());
        processor.reset();
        assert_eq!(processor.finish().unwrap(), (Vec::new(), None));
        processor.reset();
        let frames = VOCAL_SAMPLE_RATE_HZ as usize * 7 + 137;
        let stereo = (0..frames)
            .flat_map(|i| {
                let sample = ((i as f32 * 0.031).sin() * 8192.0) as i16;
                [sample.to_le_bytes(), sample.to_le_bytes()].concat()
            })
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        for packet in stereo.chunks(4092) {
            let (pcm, detection) = processor.push(packet).unwrap();
            assert!(detection.is_none());
            output.extend(pcm);
        }
        output.extend(processor.finish().unwrap().0);
        assert_eq!(
            output.len(),
            (frames * 16_000).div_ceil(VOCAL_SAMPLE_RATE_HZ as usize) * 2
        );
        assert!(processor.finish().unwrap().0.is_empty());

        if let Some(model) = std::env::var_os("XRT_GTCRN_MODEL") {
            use super::super::audio::AudioBuffer;
            let mut processor = AudioPreprocessor::gtcrn(Path::new(&model), 2).unwrap();
            let mut buffer = AudioBuffer::default();
            let pcm = (0..13_127)
                .flat_map(|i| (((i as f32 * 0.04).sin() * 8192.0) as i16).to_le_bytes())
                .collect::<Vec<_>>();
            let mut recognition = Vec::new();
            for packet in pcm.chunks(1000) {
                let (original, detection) = processor.push(packet).unwrap();
                assert_eq!(original, packet);
                buffer.push(&original, detection.as_deref());
                recognition.extend(buffer.take_ready().raw);
            }
            let (source, tail) = processor.finish().unwrap();
            assert!(source.is_empty());
            let final_frames = buffer.finish(tail.as_deref());
            recognition.extend_from_slice(&final_frames.raw[..final_frames.real_samples * 2]);
            assert_eq!(
                recognition, pcm,
                "GTCRN must retain every original ASR sample"
            );
        }
    }
}
