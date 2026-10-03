//! Bounded render-time sampling of the single companion owner's animation.
use super::{Appearance, Motion, Pose, Speech};
#[cfg(test)]
use std::time::Duration;
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
    #[cfg(test)]
    pub(crate) fn sample(&self, elapsed: Duration) -> Self {
        self.sample_at(self.clock + elapsed.as_secs_f64().min(0.2))
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::avatar::{Expression, Gaze};

    #[test]
    fn sparse_owner_snapshots_keep_rendering_without_replaying_behavior() {
        let mut motion = Motion::default();
        motion.advance(2.3, Expression::Happy, Gaze::default(), 0.0);
        let mut source = Presentation {
            pose: motion.advance(2.4, Expression::Happy, Gaze::default(), 0.0),
            motion: Some(motion),
            clock: 2.4,
            visible: true,
            ..Presentation::default()
        };
        source.speech.say("Hello there", source.clock);
        let original = source.clone();
        let now = Instant::now();
        let mut sampler = PresentationSampler::default();
        sampler.sample_at(&source, now);
        let between = sampler.sample_at(&source, now + Duration::from_millis(50));
        assert!(between.pose.blink > source.pose.blink);
        assert_ne!(between.pose, source.pose);
        assert_ne!(between.speech, source.speech);
        assert_eq!(source, original);
        source.clock += 0.1;
        source.motion.as_mut().unwrap().advance(
            source.clock,
            Expression::Happy,
            Gaze::default(),
            0.0,
        );
        assert_eq!(
            sampler
                .sample_at(&source, now + Duration::from_millis(100))
                .clock,
            source.clock
        );
        assert_eq!(
            sampler.sample_at(&source, now + Duration::from_secs(1)),
            source.sample(Duration::from_millis(200))
        );
        source.visible = false;
        let hidden = sampler.sample_at(&source, now + Duration::from_secs(2));
        assert_eq!(hidden.clock, source.clock);
        assert_eq!(hidden.speech, Speech::default());
        source.visible = true;
        assert_eq!(
            sampler
                .sample_at(&source, now + Duration::from_secs(3))
                .clock,
            source.clock
        );
        source
            .speech
            .say("A new reply at the same owner tick", source.clock);
        assert_eq!(
            sampler
                .sample_at(&source, now + Duration::from_secs(4))
                .clock,
            source.clock
        );

        let mut legacy = serde_json::to_value(&source).unwrap();
        legacy.as_object_mut().unwrap().remove("motion");
        let legacy: Presentation = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.sample(Duration::from_secs(1)), legacy);

        // A newer, slightly delayed packet must not undo the sampled dismissal.
        source.clock = 2.2;
        source.speech.say("A", 0.0);
        source.speech.advance(source.clock);
        sampler.sample_at(&source, now + Duration::from_secs(5));
        let finished = sampler.sample_at(&source, now + Duration::from_millis(5080));
        assert!(finished.speech.finished(finished.clock));
        source.clock = 2.23;
        let delayed = sampler.sample_at(&source, now + Duration::from_millis(5090));
        assert!(delayed.clock >= finished.clock);
        assert!(delayed.speech.finished(delayed.clock));
        source.speech.say("A new answer", source.clock);
        let answer = sampler.sample_at(&source, now + Duration::from_millis(5100));
        assert!(answer.clock >= delayed.clock);
        assert!(!answer.speech.finished(answer.clock));
        source.clock = 0.0; // A restarted owner intentionally resets its clock.
        assert_eq!(
            sampler
                .sample_at(&source, now + Duration::from_secs(6))
                .clock,
            0.0
        );
    }
}
