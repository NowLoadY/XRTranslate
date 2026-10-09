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
