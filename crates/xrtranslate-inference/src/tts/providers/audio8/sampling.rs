//! Deterministic Audio8 autoregressive sampling.

use crate::InferenceError;

use super::{Audio8SynthesisOptions, RuntimeManifest, native_error};

pub(super) fn sample_semantic(
    logits: &[f32],
    previous: &[i64],
    manifest: &RuntimeManifest,
    options: Audio8SynthesisOptions,
    rng: &mut NumpyPcg64,
) -> Result<i64, InferenceError> {
    let expected = manifest.semantic_end_id - manifest.semantic_begin_id + 2;
    if logits.len() != expected as usize {
        return Err(native_error("unexpected Audio8 semantic logits size"));
    }
    let normal_index = sample(
        logits,
        options.temperature,
        options.top_p,
        options.top_k,
        rng,
    );
    let high_index = sample(logits, 1.0, 0.9, options.top_k, rng);
    let map = |index: usize| {
        if index + 1 == logits.len() {
            manifest.im_end_id
        } else {
            manifest.semantic_begin_id + index as i64
        }
    };
    let normal = map(normal_index);
    if normal != manifest.im_end_id && previous.contains(&normal) {
        Ok(map(high_index))
    } else {
        Ok(normal)
    }
}

pub(super) fn sample(
    logits: &[f32],
    temperature: f64,
    top_p: f64,
    top_k: usize,
    rng: &mut NumpyPcg64,
) -> usize {
    let mut order = (0..logits.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|left, right| logits[*right].total_cmp(&logits[*left]));
    let max = f64::from(logits[order[0]]);
    let mut probabilities = order
        .iter()
        .map(|index| (f64::from(logits[*index]) - max).exp())
        .collect::<Vec<_>>();
    let sum = probabilities.iter().sum::<f64>();
    for value in &mut probabilities {
        *value /= sum;
    }
    let mut cumulative = 0.0;
    let mut kept = Vec::new();
    for (rank, (index, probability)) in order.into_iter().zip(probabilities).enumerate() {
        cumulative += probability;
        if rank > 0 && (rank >= top_k || cumulative > top_p) {
            break;
        }
        kept.push(index);
    }
    let mut retained = vec![false; logits.len()];
    for index in kept {
        retained[index] = true;
    }
    let maximum = retained
        .iter()
        .enumerate()
        .filter(|(_, retained)| **retained)
        .map(|(index, _)| f64::from(logits[index]) / temperature.max(1e-5))
        .fold(f64::NEG_INFINITY, f64::max);
    retained
        .into_iter()
        .enumerate()
        .map(|(index, retained)| {
            // NumPy consumes noise for every original logit, including masked
            // entries. This keeps later autoregressive choices reproducible.
            let noise = -rng.next_f64().max(1e-12).ln();
            let score = if retained {
                (f64::from(logits[index]) / temperature.max(1e-5) - maximum).exp() / noise
            } else {
                0.0
            };
            (index, score)
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// NumPy's PCG64 stream for `default_rng(42)`, the seed fixed by Audio8's
/// reference runtime.
pub(super) struct NumpyPcg64 {
    state: u128,
    increment: u128,
}

impl NumpyPcg64 {
    const MULTIPLIER: u128 = 0x2360_ED05_1FC6_5DA4_4385_DF64_9FCC_F645;

    pub(super) fn seed_42() -> Self {
        Self {
            state: 274_674_114_334_540_486_603_088_602_300_644_985_544,
            increment: 332_724_090_758_049_132_448_979_897_138_935_081_983,
        }
    }

    pub(super) fn next_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
        let folded = ((self.state >> 64) as u64) ^ self.state as u64;
        let raw = folded.rotate_right((self.state >> 122) as u32);
        (raw >> 11) as f64 * (1.0 / ((1_u64 << 53) as f64))
    }
}
