//! ONNX graph contract and signal processing for OpenVoice.

use std::{path::Path, sync::Arc};

use half::f16;
use ndarray::{Array1, Array2, Array3, arr0};
use ort::{session::Session, value::Value};
use rustfft::{FftPlanner, num_complex::Complex32};
use serde::Deserialize;

use crate::{
    InferenceError, SynthesizedPcm,
    tts::{
        audio::{resample, resample_pcm16},
        onnx_runtime::{ActiveOnnxDevice, OnnxExecutionDevice, build_session_group},
    },
};

use super::{
    OpenVoiceBaseVoice,
    frontend::{EnglishFrontend, EnglishInputs},
};

const BASE_SAMPLE_RATE: u32 = 44_100;
pub(super) const OUTPUT_SAMPLE_RATE: u32 = 22_050;
const REFERENCE_FFT_SIZE: usize = 1024;
const REFERENCE_HOP_SIZE: usize = 256;
const REFERENCE_PAD: usize = (REFERENCE_FFT_SIZE - REFERENCE_HOP_SIZE) / 2;
const SPEAKER_EMBEDDING_SIZE: usize = 256;

#[derive(Deserialize)]
struct ModelConfig {
    symbols: Vec<String>,
}

pub(super) struct OpenVoiceRuntime {
    frontend: EnglishFrontend,
    bert: Session,
    base: Session,
    converter: Session,
    reference_encoder: Session,
    source_embedding: Vec<f16>,
    base_voice: OpenVoiceBaseVoice,
    active_device: ActiveOnnxDevice,
}

impl OpenVoiceRuntime {
    pub(super) fn load(
        model_dir: &Path,
        device: OnnxExecutionDevice,
        threads: usize,
        base_voice: OpenVoiceBaseVoice,
    ) -> Result<Self, InferenceError> {
        let config: ModelConfig = serde_json::from_slice(
            &std::fs::read(model_dir.join("model_config.json"))
                .map_err(|error| model_error(error.to_string()))?,
        )
        .map_err(|error| model_error(error.to_string()))?;
        let frontend = EnglishFrontend::load(model_dir, &config.symbols)?;
        let bert_path = model_dir.join("models/bert.onnx");
        let base_path = model_dir.join("models/melo.onnx");
        let converter_path = model_dir.join("models/converter.onnx");
        let reference_path = model_dir.join("models/reference_encoder.onnx");
        let (mut sessions, active_device) = build_session_group(
            &[&bert_path, &base_path, &converter_path, &reference_path],
            device,
            threads,
            "openvoice",
        )?;
        let reference_encoder = sessions.pop().expect("four sessions were requested");
        let converter = sessions.pop().expect("four sessions were requested");
        let base = sessions.pop().expect("four sessions were requested");
        let bert = sessions.pop().expect("four sessions were requested");
        let source_embedding =
            read_source_embedding(&model_dir.join(base_voice.source_embedding()))?;
        Ok(Self {
            frontend,
            bert,
            base,
            converter,
            reference_encoder,
            source_embedding,
            base_voice,
            active_device,
        })
    }

    pub(super) const fn active_device(&self) -> OnnxExecutionDevice {
        self.active_device.execution_device()
    }

    pub(super) fn encode_reference(
        &mut self,
        pcm16: &[u8],
        sample_rate: u32,
    ) -> Result<Vec<f16>, InferenceError> {
        let samples = resample_pcm16(pcm16, sample_rate, OUTPUT_SAMPLE_RATE)?;
        if samples.len() < OUTPUT_SAMPLE_RATE as usize / 2 {
            return Err(model_error(
                "OpenVoice reference audio must contain at least 0.5 seconds",
            ));
        }
        let spectrum = reference_spectrum(&samples)?;
        let frames = spectrum.len() / (REFERENCE_FFT_SIZE / 2 + 1);
        let input = Array3::from_shape_vec((1, frames, REFERENCE_FFT_SIZE / 2 + 1), spectrum)
            .map_err(|error| model_error(error.to_string()))?;
        let outputs = self
            .reference_encoder
            .run(ort::inputs![
                "spec" => Value::from_array(input).map_err(ort_error)?
            ])
            .map_err(ort_error)?;
        let (shape, embedding) = outputs["tone_embedding"]
            .try_extract_tensor::<f32>()
            .map_err(ort_error)?;
        if shape.as_ref() != [1, SPEAKER_EMBEDDING_SIZE as i64]
            || embedding.iter().any(|value| !value.is_finite())
        {
            return Err(model_error(format!(
                "unexpected OpenVoice reference embedding {shape:?}"
            )));
        }
        Ok(embedding.iter().copied().map(f16::from_f32).collect())
    }

    pub(super) fn synthesize(
        &mut self,
        text: &str,
        target_embedding: &[f16],
        speed: f32,
    ) -> Result<SynthesizedPcm, InferenceError> {
        let inputs = self.frontend.encode(&mut self.bert, text)?;
        let base_audio = self.run_base(inputs, speed, self.base_voice.speaker_id())?;
        let base_audio = resample(&base_audio, BASE_SAMPLE_RATE, OUTPUT_SAMPLE_RATE)?
            .into_iter()
            .map(f16::from_f32)
            .collect::<Vec<_>>();
        let audio = Array2::from_shape_vec((1, base_audio.len()), base_audio)
            .map_err(|error| model_error(error.to_string()))?;
        let source = Array3::from_shape_vec(
            (1, SPEAKER_EMBEDDING_SIZE, 1),
            self.source_embedding.clone(),
        )
        .map_err(|error| model_error(error.to_string()))?;
        let target =
            Array3::from_shape_vec((1, SPEAKER_EMBEDDING_SIZE, 1), target_embedding.to_vec())
                .map_err(|error| model_error(error.to_string()))?;
        let outputs = self
            .converter
            .run(ort::inputs![
                "audio_base" => Value::from_array(audio).map_err(ort_error)?,
                "se_src" => Value::from_array(source).map_err(ort_error)?,
                "se_target" => Value::from_array(target).map_err(ort_error)?,
            ])
            .map_err(ort_error)?;
        let (shape, values) = outputs["output"]
            .try_extract_tensor::<f16>()
            .map_err(ort_error)?;
        if shape.len() != 3 || shape[0] != 1 || shape[1] != 1 || values.is_empty() {
            return Err(model_error(format!(
                "unexpected OpenVoice converter output {shape:?}"
            )));
        }
        let mut peak = 0.0_f32;
        let bytes = values
            .iter()
            .flat_map(|value| {
                let value = value.to_f32();
                peak = peak.max(value.abs());
                ((value.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16).to_le_bytes()
            })
            .collect::<Vec<_>>();
        if !peak.is_finite() || peak < 1.0e-5 {
            return Err(model_error(
                "OpenVoice converter returned silent or non-finite audio",
            ));
        }
        Ok(SynthesizedPcm {
            bytes,
            sample_rate: OUTPUT_SAMPLE_RATE,
        })
    }

    fn run_base(
        &mut self,
        inputs: EnglishInputs,
        speed: f32,
        speaker_id: i32,
    ) -> Result<Vec<f32>, InferenceError> {
        let tokens = inputs.phone_ids.len();
        let phone_ids = Array2::from_shape_vec((1, tokens), inputs.phone_ids)
            .map_err(|error| model_error(error.to_string()))?;
        let tones = Array2::from_shape_vec((1, tokens), inputs.tones)
            .map_err(|error| model_error(error.to_string()))?;
        let languages = Array2::from_shape_vec((1, tokens), inputs.language_ids)
            .map_err(|error| model_error(error.to_string()))?;
        let bert = Array3::from_shape_vec((1, 768, tokens), inputs.bert)
            .map_err(|error| model_error(error.to_string()))?;
        let lengths = Array1::from_vec(vec![tokens as i32]);
        let speakers = Array1::from_vec(vec![speaker_id]);
        let length_scale = arr0(f16::from_f32(1.0 / speed));
        let outputs = self
            .base
            .run(ort::inputs![
                "x_tst" => Value::from_array(phone_ids).map_err(ort_error)?,
                "x_tst_lenghts" => Value::from_array(lengths).map_err(ort_error)?,
                "speakers" => Value::from_array(speakers).map_err(ort_error)?,
                "tones" => Value::from_array(tones).map_err(ort_error)?,
                "lang_ids" => Value::from_array(languages).map_err(ort_error)?,
                "ja_bert" => Value::from_array(bert).map_err(ort_error)?,
                "length_scale" => Value::from_array(length_scale).map_err(ort_error)?,
            ])
            .map_err(ort_error)?;
        let (shape, audio) = outputs["output"]
            .try_extract_tensor::<f16>()
            .map_err(ort_error)?;
        if shape.len() != 3 || shape[0] != 1 || shape[1] != 1 || audio.is_empty() {
            return Err(model_error(format!("unexpected MeloTTS output {shape:?}")));
        }
        let audio = audio.iter().map(|value| value.to_f32()).collect::<Vec<_>>();
        if audio.iter().any(|value| !value.is_finite()) {
            return Err(model_error("MeloTTS returned non-finite audio"));
        }
        Ok(audio)
    }
}

fn read_source_embedding(path: &Path) -> Result<Vec<f16>, InferenceError> {
    let bytes = std::fs::read(path).map_err(|error| model_error(error.to_string()))?;
    if bytes.len() != SPEAKER_EMBEDDING_SIZE * size_of::<f32>() {
        return Err(model_error(format!(
            "invalid source speaker embedding size {}",
            bytes.len()
        )));
    }
    let embedding = bytes
        .chunks_exact(4)
        .map(|bytes| f16::from_f32(f32::from_le_bytes(bytes.try_into().unwrap())))
        .collect::<Vec<_>>();
    Ok(embedding)
}

fn reference_spectrum(samples: &[f32]) -> Result<Vec<f32>, InferenceError> {
    if samples.len() <= REFERENCE_PAD {
        return Err(model_error(
            "reference audio is too short for OpenVoice STFT",
        ));
    }
    let mut padded = Vec::with_capacity(samples.len() + REFERENCE_PAD * 2);
    for index in (1..=REFERENCE_PAD).rev() {
        padded.push(samples[index]);
    }
    padded.extend_from_slice(samples);
    for index in 0..REFERENCE_PAD {
        padded.push(samples[samples.len() - 2 - index]);
    }
    if padded.len() < REFERENCE_FFT_SIZE {
        return Err(model_error(
            "reference audio is too short for OpenVoice STFT",
        ));
    }
    let frames = (padded.len() - REFERENCE_FFT_SIZE) / REFERENCE_HOP_SIZE + 1;
    let window = (0..REFERENCE_FFT_SIZE)
        .map(|index| {
            0.5 - 0.5
                * (2.0 * std::f32::consts::PI * index as f32 / REFERENCE_FFT_SIZE as f32).cos()
        })
        .collect::<Vec<_>>();
    let mut planner = FftPlanner::<f32>::new();
    let fft: Arc<dyn rustfft::Fft<f32>> = planner.plan_fft_forward(REFERENCE_FFT_SIZE);
    let bins = REFERENCE_FFT_SIZE / 2 + 1;
    let mut spectrum = Vec::with_capacity(frames * bins);
    let mut buffer = vec![Complex32::default(); REFERENCE_FFT_SIZE];
    for frame in 0..frames {
        let begin = frame * REFERENCE_HOP_SIZE;
        for index in 0..REFERENCE_FFT_SIZE {
            buffer[index] = Complex32::new(padded[begin + index] * window[index], 0.0);
        }
        fft.process(&mut buffer);
        spectrum.extend(
            buffer[..bins]
                .iter()
                .map(|value| (value.norm_sqr() + 1.0e-6).sqrt()),
        );
    }
    Ok(spectrum)
}

fn ort_error(error: ort::Error) -> InferenceError {
    model_error(error.to_string())
}

fn model_error(message: impl Into<String>) -> InferenceError {
    InferenceError::InvalidConfiguration {
        field: "tts.providers.openvoice.onnx",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_spectrum_uses_the_official_513_bins() {
        let samples = vec![0.0; OUTPUT_SAMPLE_RATE as usize];
        let spectrum = reference_spectrum(&samples).unwrap();
        assert_eq!(spectrum.len() % 513, 0);
        assert!(spectrum.iter().all(|value| value.is_finite()));
    }
}
