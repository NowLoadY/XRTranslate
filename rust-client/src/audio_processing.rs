//! Source-entry processing shared by live capture, file imports and playback.

pub const DEFAULT_GATE_THRESHOLD_DB: f32 = -45.0;

#[derive(Clone, Debug, PartialEq)]
pub enum SourceEffect {
    Gain(f32),
    NoiseGate(f32),
}

pub fn default_source_effects() -> Vec<SourceEffect> {
    vec![SourceEffect::NoiseGate(DEFAULT_GATE_THRESHOLD_DB)]
}

/// Peak envelope with a speech-friendly hold and short gain ramps. Silence remains in
/// the stream to preserve timing and allow VAD to observe utterance endings.
pub struct NoiseGate {
    threshold: f32,
    hold_frames: u32,
    remaining: u32,
    gain: f32,
    attack_step: f32,
    release_step: f32,
}

impl NoiseGate {
    pub fn new(threshold_db: f32, sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            threshold: 10.0_f32.powf(threshold_db.clamp(-80.0, 0.0) / 20.0),
            hold_frames: (rate * 0.35).round() as u32,
            remaining: 0,
            gain: 0.0,
            attack_step: 1.0 / (rate * 0.002).max(1.0),
            release_step: 1.0 / (rate * 0.08).max(1.0),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples {
            if !sample.is_finite() {
                *sample = 0.0;
            }
            if sample.abs() > self.threshold {
                self.remaining = self.hold_frames;
            } else {
                self.remaining = self.remaining.saturating_sub(1);
            }
            self.gain = if self.remaining > 0 {
                (self.gain + self.attack_step).min(1.0)
            } else {
                (self.gain - self.release_step).max(0.0)
            };
            *sample *= self.gain;
        }
    }
}

enum ActiveEffect {
    Gain(f32),
    NoiseGate(NoiseGate),
}

pub struct SourcePipeline {
    effects: Vec<ActiveEffect>,
}

impl SourcePipeline {
    pub fn new(config: &[SourceEffect], sample_rate: u32) -> Self {
        Self {
            effects: config
                .iter()
                .map(|effect| match effect {
                    SourceEffect::Gain(gain) => ActiveEffect::Gain(*gain),
                    SourceEffect::NoiseGate(db) => {
                        ActiveEffect::NoiseGate(NoiseGate::new(*db, sample_rate))
                    }
                })
                .collect(),
        }
    }

    pub fn reset(&mut self) {
        for effect in &mut self.effects {
            if let ActiveEffect::NoiseGate(gate) = effect {
                gate.remaining = 0;
                gate.gain = 0.0;
            }
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for effect in &mut self.effects {
            match effect {
                ActiveEffect::Gain(gain) => samples.iter_mut().for_each(|sample| *sample *= *gain),
                ActiveEffect::NoiseGate(gate) => gate.process(samples),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tts_utterance_starts_with_a_closed_gate_after_idle() {
        let mut pipeline = SourcePipeline::new(&default_source_effects(), 48_000);
        pipeline.process(&mut vec![0.1; 4800]);
        pipeline.reset();
        let mut quiet = vec![0.001; 480];
        pipeline.process(&mut quiet);
        assert!(quiet.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn gate_silences_noise_passes_voice_and_closes_after_release() {
        for rate in [16_000, 44_100, 48_000] {
            let mut gate = NoiseGate::new(DEFAULT_GATE_THRESHOLD_DB, rate);
            let mut noise = vec![0.001; rate as usize / 10];
            gate.process(&mut noise);
            assert!(noise.iter().all(|sample| *sample == 0.0));
            let mut voice = vec![0.1; rate as usize / 10];
            gate.process(&mut voice);
            assert_eq!(*voice.last().unwrap(), 0.1);
            assert!(
                voice
                    .windows(2)
                    .all(|pair| (pair[1] - pair[0]).abs() < 0.004)
            );
            let mut tail = vec![0.001; rate as usize / 2];
            gate.process(&mut tail);
            assert!(
                tail[..rate as usize / 4]
                    .iter()
                    .all(|sample| *sample == 0.001)
            );
            assert!(
                tail[rate as usize * 45 / 100..]
                    .iter()
                    .all(|sample| *sample == 0.0)
            );
        }
    }

    #[test]
    fn brief_speech_pause_does_not_close_the_gate() {
        let mut gate = NoiseGate::new(-45.0, 16_000);
        gate.process(&mut vec![0.1; 1600]);
        let mut pause = vec![0.001; 3200];
        gate.process(&mut pause);
        assert!(pause.iter().all(|sample| *sample == 0.001));
        let mut next_word = vec![0.1; 1600];
        gate.process(&mut next_word);
        assert!(next_word.iter().all(|sample| *sample == 0.1));
    }

    #[test]
    fn gate_keeps_zero_crossings_and_chunk_boundaries_smooth() {
        let input = (0..4800)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.1)
            .collect::<Vec<_>>();
        let mut whole = input.clone();
        NoiseGate::new(-45.0, 48_000).process(&mut whole);
        let mut chunks = input.clone();
        let mut gate = NoiseGate::new(-45.0, 48_000);
        for chunk in chunks.chunks_mut(127) {
            gate.process(chunk);
        }
        assert_eq!(whole, chunks);
        assert_eq!(&whole[200..], &input[200..]);
    }

    #[test]
    fn serial_order_and_independent_thresholds_are_preserved() {
        let mut high_gate = SourcePipeline::new(
            &[SourceEffect::NoiseGate(-20.0), SourceEffect::Gain(4.0)],
            48_000,
        );
        let mut low_gate = SourcePipeline::new(&[SourceEffect::NoiseGate(-45.0)], 48_000);
        let mut high = vec![0.05; 4800];
        let mut low = high.clone();
        high_gate.process(&mut high);
        low_gate.process(&mut low);
        assert!(high.iter().all(|sample| *sample == 0.0));
        assert_eq!(*low.last().unwrap(), 0.05);
    }
}
