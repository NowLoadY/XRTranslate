//! Bounded render-time sampling of the single companion owner's animation.
use super::{Appearance, Motion, Pose, Speech};
use std::time::Instant;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Presentation {
    #[serde(default)]
    pub appearance: Appearance,
    pub pose: Pose,
    pub speech: Speech,
    pub clock: f64,
    pub visible: bool,
    #[serde(default)]
    pub(crate) motion: Option<Motion>,
}

impl Presentation {
    fn sample_at(&self, clock: f64) -> Self {
        let mut frame = self.clone();
        if !self.visible {
            frame.speech = Speech::default();
        } else if let Some(motion) = &self.motion {
            frame.clock = clock.max(self.clock).min(self.clock + 0.2);
            let mouth = frame.speech.advance(frame.clock);
            frame.pose = motion.sample_at(frame.clock, mouth);
        }
        frame
    }
}

/// A receiver clock, not another animator or dialogue owner. Repeated paints
/// never restart its origin; stale packets stop extrapolating after 200 ms.
#[derive(Default)]
pub(crate) struct PresentationSampler {
    received: Option<(Presentation, Instant)>,
    clock: Option<f64>,
}

impl PresentationSampler {
    pub(crate) fn sample(&mut self, source: &Presentation) -> Presentation {
        self.sample_at(source, Instant::now())
    }

    fn sample_at(&mut self, source: &Presentation, now: Instant) -> Presentation {
        let received = self.received.get_or_insert_with(|| (source.clone(), now));
        if source.clock < received.0.clock || source.visible != received.0.visible {
            self.clock = None;
        }
        if received.0 != *source {
            *received = (source.clone(), now);
        }
        // A delayed owner packet must not retract letters or resurrect a bubble
        // that already finished at the receiver's bounded presentation time.
        let clock = source.clock + now.saturating_duration_since(received.1).as_secs_f64();
        let frame = source.sample_at(clock.max(self.clock.unwrap_or(source.clock)));
        self.clock = source.visible.then_some(frame.clock);
        frame
    }
}
