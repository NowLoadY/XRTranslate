//! Shared PCM encoding for transport, inference and stored reference audio.

/// PCM audio format accepted by Qwen3-ASR in the initial native route.
pub const PCM16_MONO_16KHZ_FORMAT: &str = "PCM16LE mono 16000 Hz";

/// Wraps raw, little-endian, mono PCM16 at 16 kHz in a canonical RIFF/WAV file.
pub fn pcm16_mono_16khz_to_wav(pcm: &[u8]) -> Result<Vec<u8>, String> {
    if !pcm.len().is_multiple_of(2) {
        return Err("the byte length must be divisible by two for PCM16LE samples".into());
    }
    let data_len =
        u32::try_from(pcm.len()).map_err(|_| "audio is too large for a RIFF/WAV data chunk")?;
    let riff_len = data_len
        .checked_add(36)
        .ok_or("audio is too large for a RIFF/WAV container")?;

    let mut wav = Vec::with_capacity(pcm.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes()); // PCM fmt chunk size
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM format tag
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes()); // sample rate * 2 bytes
    wav.extend_from_slice(&2_u16.to_le_bytes()); // block alignment
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

pub fn f32_to_pcm16le(samples: Vec<f32>) -> Vec<u8> {
    samples
        .into_iter()
        .flat_map(|sample| {
            let sample = sample.clamp(-1.0, 1.0);
            let pcm = if sample < 0.0 {
                (sample * 32768.0) as i16
            } else {
                (sample * 32767.0) as i16
            };
            pcm.to_le_bytes()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_canonical_pcm_wav_header() {
        let wav = pcm16_mono_16khz_to_wav(&[1, 0, 2, 0]).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
    }

    #[test]
    fn rejects_partial_pcm16_samples() {
        assert!(pcm16_mono_16khz_to_wav(&[0]).is_err());
    }
}
