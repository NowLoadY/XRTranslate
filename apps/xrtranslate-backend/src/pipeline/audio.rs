//! Provider-independent buffering and speech admission. Recognition and delayed
//! detection audio stay aligned after the selected input preprocessing.

use std::{collections::VecDeque, mem};
use xrtranslate_vad::{FRAME_BYTES, FRAME_SAMPLES, Utterance};

#[derive(Default)]
pub(super) struct AudioBuffer {
    raw: Vec<u8>,
    denoised: Vec<u8>,
}

pub(super) struct AudioFrames {
    pub(super) raw: Vec<u8>,
    pub(super) denoised: Vec<u8>,
    pub(super) real_samples: usize,
}

impl AudioFrames {
    pub(super) fn padding_samples(&self) -> usize {
        self.raw.len() / 2 - self.real_samples
    }
}

impl AudioBuffer {
    pub(super) fn push(&mut self, raw: &[u8], denoised: Option<&[u8]>) {
        self.raw.extend_from_slice(raw);
        match denoised {
            // Empty output means the enhancer is buffering, not bypassing.
            Some(output) => self.denoised.extend_from_slice(output),
            // Bypass starts at the oldest unprocessed sample, including any
            // delayed input from before an enhancer failure.
            None => self.denoised.clone_from(&self.raw),
        }
    }

    pub(super) fn take_ready(&mut self) -> AudioFrames {
        let bytes = self.raw.len().min(self.denoised.len()) / FRAME_BYTES * FRAME_BYTES;
        AudioFrames {
            raw: self.raw.drain(..bytes).collect(),
            denoised: self.denoised.drain(..bytes).collect(),
            real_samples: bytes / 2,
        }
    }

    pub(super) fn finish(&mut self, denoised_tail: Option<&[u8]>) -> AudioFrames {
        self.push(&[], denoised_tail);
        let real_samples = self.raw.len() / 2;
        let bytes = self.raw.len().div_ceil(FRAME_BYTES) * FRAME_BYTES;
        if self.denoised.len() < self.raw.len() {
            // Preserve alignment even if an enhancer fails to drain its tail.
            tracing::warn!("denoiser returned an incomplete tail; using pending raw audio");
            self.denoised.clone_from(&self.raw);
        }
        // Discard synthetic enhancer padding, then pad only the final VAD frame.
        self.denoised.truncate(self.raw.len());
        self.raw.resize(bytes, 0);
        self.denoised.resize(bytes, 0);
        AudioFrames {
            raw: mem::take(&mut self.raw),
            denoised: mem::take(&mut self.denoised),
            real_samples,
        }
    }

    pub(super) fn reset(&mut self) {
        self.raw.clear();
        self.denoised.clear();
    }
}

/// Bounded VAD evidence shared by complete turns, overlapping windows and live
/// stream onset. Padding, silence at the edges and old overlap cannot establish
/// fresh speech. No text, language or model-specific rules belong here.
pub(super) struct SpeechGate {
    probabilities: VecDeque<(f32, usize)>,
    capacity: usize,
    frames_seen: u64,
    accepted_until: Option<u64>,
}

impl SpeechGate {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            probabilities: VecDeque::with_capacity(capacity),
            capacity,
            frames_seen: 0,
            accepted_until: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.probabilities.clear();
        self.frames_seen = 0;
        self.accepted_until = None;
    }

    pub(super) fn observe(&mut self, probability: f32, real_samples: usize) {
        if self.probabilities.len() == self.capacity {
            self.probabilities.pop_front();
        }
        self.probabilities.push_back((probability, real_samples));
        self.frames_seen += 1;
    }

    pub(super) fn allows_onset(&self, samples: usize, threshold: f32) -> bool {
        self.allows(samples.div_ceil(FRAME_SAMPLES), 0, 0, threshold)
    }

    pub(super) fn accept(&mut self, utterance: &Utterance, threshold: f32) -> bool {
        let accepted = self.allows(
            utterance.samples.len().div_ceil(FRAME_SAMPLES),
            utterance.pre_roll_frames,
            utterance.overlap_frames,
            threshold,
        );
        if accepted {
            self.accepted_until = Some(self.frames_seen);
        } else {
            tracing::debug!(
                frames = utterance.frame_count(),
                "skipping audio with insufficient speech evidence"
            );
        }
        accepted
    }

    fn allows(&self, frames: usize, pre_roll: usize, overlap: usize, threshold: f32) -> bool {
        if frames == 0 || frames > self.probabilities.len() {
            return false;
        }
        let leading = pre_roll.max(overlap).min(frames);
        let mut first = None;
        let mut last = 0;
        let mut speech = 0;
        let mut confident = 0;
        let mut elapsed = 0;
        for &(probability, samples) in self
            .probabilities
            .iter()
            .skip(self.probabilities.len() - frames + leading)
        {
            elapsed += samples;
            if probability >= threshold {
                first.get_or_insert(elapsed - samples);
                last = elapsed;
                speech += samples;
                if probability >= threshold.max(0.8) {
                    confident += samples;
                }
            }
        }
        let Some(first) = first else {
            return false;
        };
        let span = last - first;
        let start = self.frames_seen.saturating_sub(frames as u64);
        let continuation = overlap > 0 && self.accepted_until == Some(start + overlap as u64);
        // Normally require 160 ms of speech. Clear short words may use 96 ms;
        // an accepted overlapping turn can finish with a shorter new tail.
        let enough_speech =
            continuation || speech >= 5 * FRAME_SAMPLES || confident >= 3 * FRAME_SAMPLES;
        enough_speech && speech * 2 >= span
    }
}
