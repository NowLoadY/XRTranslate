use crate::session_coordinator::TranslationEvent;

/// First-arrival order for the combined feed, independent of translation latency.
#[derive(Clone, Default)]
pub(crate) struct FeedTimeline {
    turns: Vec<(u64, String)>,
    pub(crate) revision: u64,
}

impl FeedTimeline {
    pub(crate) fn observe(&mut self, event: &TranslationEvent) {
        self.revision = self.revision.wrapping_add(1);
        match event {
            TranslationEvent::Preview(preview) => {
                self.remember(preview.stream_id, &preview.turn_id);
            }
            TranslationEvent::Segment(segment) => {
                self.remember(segment.stream_id, &segment.turn_id);
            }
            TranslationEvent::ReplaceSegments(segments) => {
                for segment in segments {
                    self.remember(segment.stream_id, &segment.turn_id);
                }
            }
            TranslationEvent::StreamEnded { .. } | TranslationEvent::Finished { .. } => {}
        }
    }

    fn remember(&mut self, stream_id: u64, turn_id: &str) {
        if !turn_id.is_empty() && self.position(Some(stream_id), turn_id).is_none() {
            self.turns.push((stream_id, turn_id.to_owned()));
        }
    }

    pub(crate) fn retain(
        &mut self,
        recognition: &[super::RecognitionHistoryEntry],
        translations: &[super::TranslationHistoryEntry],
        previews: &[super::TranslationHistoryEntry],
    ) {
        self.turns.retain(|(stream, turn)| {
            recognition
                .iter()
                .any(|entry| entry.stream_id == Some(*stream) && entry.turn_id == *turn)
                || translations
                    .iter()
                    .chain(previews)
                    .any(|entry| entry.stream_id == Some(*stream) && entry.turn_id == *turn)
        });
    }

    pub(crate) fn position(&self, stream_id: Option<u64>, turn_id: &str) -> Option<usize> {
        self.turns
            .iter()
            .position(|(stream, turn)| Some(*stream) == stream_id && turn == turn_id)
    }
}
