//! Provider-neutral TTS result types and concrete provider adapters.

mod audio;
mod onnx_runtime;
mod providers;

use crate::InferenceError;

pub use onnx_runtime::{OnnxExecutionDevice, initialize_onnx_runtime, preload_onnx_cuda_libraries};
pub use providers::{
    Audio8ExecutionDevice, Audio8OnnxAdapter, Audio8SynthesisOptions, OpenVoiceOnnxAdapter,
    OpenVoiceSynthesisOptions,
};

/// Decoded mono PCM returned by a TTS provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesizedPcm {
    pub bytes: Vec<u8>,
    pub sample_rate: u32,
}

pub(crate) fn decode_pcm16_wav(wav: &[u8]) -> Result<SynthesizedPcm, InferenceError> {
    if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(InferenceError::InvalidAudio {
            message: "TTS provider returned a non-WAV response".into(),
        });
    }
    let mut cursor = 12;
    let mut format = None;
    let mut data = None;
    while cursor + 8 <= wav.len() {
        let id = &wav[cursor..cursor + 4];
        let len = u32::from_le_bytes(wav[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= wav.len())
            .ok_or_else(|| InferenceError::InvalidAudio {
                message: "TTS WAV contains a truncated chunk".into(),
            })?;
        if id == b"fmt " && len >= 16 {
            format = Some((
                u16::from_le_bytes(wav[cursor..cursor + 2].try_into().unwrap()),
                u16::from_le_bytes(wav[cursor + 2..cursor + 4].try_into().unwrap()),
                u32::from_le_bytes(wav[cursor + 4..cursor + 8].try_into().unwrap()),
                u16::from_le_bytes(wav[cursor + 14..cursor + 16].try_into().unwrap()),
            ));
        } else if id == b"data" {
            data = Some(wav[cursor..end].to_vec());
        }
        cursor = end + (len & 1);
    }
    let Some((tag, channels, sample_rate, bits)) = format else {
        return Err(InferenceError::InvalidAudio {
            message: "TTS WAV has no format chunk".into(),
        });
    };
    if tag != 1 || channels != 1 || bits != 16 {
        return Err(InferenceError::InvalidAudio {
            message: format!(
                "TTS WAV must be mono PCM16 (tag={tag}, channels={channels}, bits={bits})"
            ),
        });
    }
    Ok(SynthesizedPcm {
        bytes: data.ok_or_else(|| InferenceError::InvalidAudio {
            message: "TTS WAV has no data chunk".into(),
        })?,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm16_mono_16khz_to_wav;

    #[test]
    fn extracts_provider_pcm_without_forwarding_a_wav_header() {
        let wav = pcm16_mono_16khz_to_wav(&[1, 0, 2, 0]).unwrap();
        assert_eq!(
            decode_pcm16_wav(&wav).unwrap(),
            SynthesizedPcm {
                bytes: vec![1, 0, 2, 0],
                sample_rate: 16_000
            }
        );
    }
}
