//! Provider-neutral mono PCM helpers used by native TTS adapters.

use crate::InferenceError;

pub(crate) fn pcm16_bytes_to_f32(bytes: &[u8]) -> Result<Vec<f32>, InferenceError> {
    if bytes.len() % 2 != 0 {
        return Err(InferenceError::InvalidAudio {
            message: "invalid PCM16 reference data".into(),
        });
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|bytes| f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0)
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return Err(InferenceError::InvalidAudio {
            message: "empty reference recording".into(),
        });
    }
    Ok(samples)
}

pub(crate) fn resample_pcm16(
    bytes: &[u8],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, InferenceError> {
    let source = pcm16_bytes_to_f32(bytes)?;
    resample(&source, source_rate, target_rate)
}

pub(crate) fn resample(
    source: &[f32],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, InferenceError> {
    if source_rate == 0 || target_rate == 0 {
        return Err(InferenceError::InvalidAudio {
            message: "audio sample rate must be greater than zero".into(),
        });
    }
    if source.is_empty() {
        return Err(InferenceError::InvalidAudio {
            message: "empty reference recording".into(),
        });
    }
    if source_rate == target_rate {
        return Ok(source.to_vec());
    }
    Ok(resample_poly(
        source,
        target_rate as usize,
        source_rate as usize,
    ))
}

/// Pure-Rust equivalent of SciPy's default
/// `signal.resample_poly(x, up, down, window=("kaiser", 5.0))` path.
fn resample_poly(source: &[f32], mut up: usize, mut down: usize) -> Vec<f32> {
    let factor = gcd(up, down);
    up /= factor;
    down /= factor;
    if up == down {
        return source.to_vec();
    }

    let output_len = (source.len() * up).div_ceil(down);
    let max_rate = up.max(down);
    let half_len = 10 * max_rate;
    let taps = 2 * half_len + 1;
    let cutoff = 1.0_f64 / max_rate as f64;
    let alpha = half_len as f64;
    let denominator = bessel_i0(5.0);
    let mut filter = (0..taps)
        .map(|index| {
            let offset = index as f64 - alpha;
            let sinc = if offset == 0.0 {
                cutoff
            } else {
                (std::f64::consts::PI * cutoff * offset).sin() / (std::f64::consts::PI * offset)
            };
            let ratio = offset / alpha;
            let window = bessel_i0(5.0 * (1.0 - ratio * ratio).max(0.0).sqrt()) / denominator;
            sinc * window
        })
        .collect::<Vec<_>>();
    let scale = filter.iter().sum::<f64>();
    for coefficient in &mut filter {
        *coefficient = (*coefficient / scale * up as f64) as f32 as f64;
    }

    let pre_pad = down - half_len % down;
    let pre_remove = (half_len + pre_pad) / down;
    let mut output = Vec::with_capacity(output_len);
    for kept_index in 0..output_len {
        let filtered_index = (kept_index + pre_remove) * down;
        let mut value = 0.0_f32;
        let first_source = filtered_index
            .saturating_sub(pre_pad + taps - 1)
            .div_ceil(up);
        let last_source =
            (filtered_index.saturating_sub(pre_pad) / up).min(source.len().saturating_sub(1));
        if first_source <= last_source {
            for source_index in first_source..=last_source {
                let upsampled_index = source_index * up;
                let Some(filter_index) = filtered_index
                    .checked_sub(pre_pad + upsampled_index)
                    .filter(|index| *index < taps)
                else {
                    continue;
                };
                value += source[source_index] * filter[filter_index] as f32;
            }
        }
        output.push(value);
    }
    output
}

fn gcd(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left.max(1)
}

fn bessel_i0(value: f64) -> f64 {
    let quarter_square = value * value / 4.0;
    let mut sum = 1.0;
    let mut term = 1.0;
    for order in 1..=32 {
        term *= quarter_square / (order * order) as f64;
        sum += term;
        if term <= sum * f64::EPSILON {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_is_bounded_and_has_the_expected_duration() {
        let bytes = [0_i16, 100, -100, 200]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let output = resample_pcm16(&bytes, 16_000, 44_100).unwrap();
        assert_eq!(output.len(), 12);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}
