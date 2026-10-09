use super::{OnlineSpeakerTracker, SpeakerError, cosine, normalize_embedding, speaker_id};

#[derive(Debug)]
struct PendingSpeakerCandidate {
    centroid: Vec<f32>,
    observations: usize,
    first_observed_at: usize,
}

impl PendingSpeakerCandidate {
    fn new(embedding: Vec<f32>, observed_at: usize) -> Self {
        Self {
            centroid: embedding,
            observations: 1,
            first_observed_at: observed_at,
        }
    }

    fn is_consistent(&self, embedding: &[f32], threshold: f32) -> bool {
        cosine(&self.centroid, embedding) >= threshold
    }

    fn accumulate(&mut self, embedding: &[f32]) -> Result<(), SpeakerError> {
        let incoming_weight = 1.0 / (self.observations as f32 + 1.0);
        for (mean, value) in self.centroid.iter_mut().zip(embedding) {
            *mean = *mean * (1.0 - incoming_weight) + *value * incoming_weight;
        }
        self.centroid = normalize_embedding(std::mem::take(&mut self.centroid))?;
        self.observations = self.observations.saturating_add(1);
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum StableIdentityDecision {
    Pending,
    Continues(String),
    Switch { previous: String, new: String },
}

/// Keeps untrusted windows out of the permanent online clustering state.
///
/// A different voice must remain internally coherent across mostly independent
/// windows before it can update an existing centroid or allocate a new ID.
#[derive(Debug, Default)]
pub(super) struct StableIdentityTracker {
    pub(super) current: Option<String>,
    last_stable: Option<String>,
    pending: Option<PendingSpeakerCandidate>,
}

impl StableIdentityTracker {
    pub(super) fn begin_turn(&mut self, inherit_previous: bool, retain_candidate: bool) {
        if inherit_previous {
            self.current.clone_from(&self.last_stable);
        } else {
            self.current = None;
        }
        if !retain_candidate {
            self.pending = None;
        }
    }

    pub(super) fn observe(
        &mut self,
        tracker: &mut OnlineSpeakerTracker,
        embedding: &[f32],
        observed_at: usize,
        window_samples: usize,
        required_observations: usize,
    ) -> Result<StableIdentityDecision, SpeakerError> {
        let observation = tracker.observe(embedding, self.current.is_some())?;

        // The first identity has no competing hypothesis, so it is safe to
        // establish immediately. Every later new identity is transactional.
        if tracker.speaker_count() == 0 {
            let assignment = tracker.commit(observation)?;
            return Ok(self.stabilize(assignment.speaker_id));
        }

        if let Some(index) = observation.matched_index {
            let matched_id = speaker_id(index);
            if self.current.as_deref() == Some(&matched_id) || self.current.is_none() {
                let assignment = tracker.commit(observation)?;
                self.pending = None;
                return Ok(self.stabilize(assignment.speaker_id));
            }
        }

        let consistency_threshold = tracker.config.similarity_threshold;
        match &mut self.pending {
            Some(candidate)
                if candidate.is_consistent(&observation.embedding, consistency_threshold) =>
            {
                candidate.accumulate(&observation.embedding)?;
            }
            _ => {
                self.pending = Some(PendingSpeakerCandidate::new(
                    observation.embedding,
                    observed_at,
                ));
            }
        }

        let candidate = self.pending.as_ref().expect("candidate was just created");
        // Two overlapping windows still contain independently arrived speech.
        // Requiring three quarters of a window made normal conversational
        // turns wait for a third inference even after two coherent results.
        let minimum_evidence_span = window_samples.saturating_mul(3) / 8;
        if candidate.observations < required_observations.max(2)
            || observed_at.saturating_sub(candidate.first_observed_at) < minimum_evidence_span
        {
            return Ok(StableIdentityDecision::Pending);
        }

        let candidate = self.pending.take().expect("confirmed candidate exists");
        let confirmed = tracker.observe(&candidate.centroid, self.current.is_some())?;
        if confirmed.matched_index.is_none() && tracker.is_full() {
            return Ok(self.current.clone().map_or(
                StableIdentityDecision::Pending,
                StableIdentityDecision::Continues,
            ));
        }
        let assignment = tracker.commit(confirmed)?;
        match self.current.replace(assignment.speaker_id.clone()) {
            Some(previous) if previous != assignment.speaker_id => {
                self.last_stable = Some(assignment.speaker_id.clone());
                Ok(StableIdentityDecision::Switch {
                    previous,
                    new: assignment.speaker_id,
                })
            }
            _ => Ok(self.stabilize(assignment.speaker_id)),
        }
    }

    fn stabilize(&mut self, speaker_id: String) -> StableIdentityDecision {
        self.current = Some(speaker_id.clone());
        self.last_stable = Some(speaker_id.clone());
        StableIdentityDecision::Continues(speaker_id)
    }

    pub(super) fn finish_turn(&mut self) -> Option<String> {
        let speaker = self.current.take();
        if let Some(speaker) = &speaker {
            self.last_stable = Some(speaker.clone());
        }
        speaker
    }

    pub(super) fn reset(&mut self) {
        self.current = None;
        self.last_stable = None;
        self.pending = None;
    }
}
