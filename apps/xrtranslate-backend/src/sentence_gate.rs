use tokio::sync::watch;
use xrtranslate_engine::{TranslationSegmentPair, ends_at_sentence_boundary, sentence_end_offsets};
use xrtranslate_vad::UtteranceEndReason;

/// A window cut or micro-pause needs text lookahead. An ordinary pause can
/// confirm sentence punctuation, but cannot turn a comma into a sentence end.
pub(crate) fn completed_prefix(text: &str, end_reason: UtteranceEndReason) -> usize {
    if matches!(
        end_reason,
        UtteranceEndReason::Flushed | UtteranceEndReason::SpeakerChange
    ) {
        return text.len();
    }
    let right_edge = text.trim_end().len();
    sentence_end_offsets(text)
        .into_iter()
        .filter(|end| *end < right_edge || end_reason == UtteranceEndReason::Silence)
        .next_back()
        .unwrap_or(0)
}

pub(crate) fn completed_sentences(
    segments: &[TranslationSegmentPair],
    end_reason: UtteranceEndReason,
) -> usize {
    if matches!(
        end_reason,
        UtteranceEndReason::Flushed | UtteranceEndReason::SpeakerChange
    ) {
        return segments.len();
    }
    segments
        .iter()
        .enumerate()
        .take_while(|(index, segment)| {
            ends_at_sentence_boundary(&segment.source_text)
                && (index + 1 < segments.len() || end_reason == UtteranceEndReason::Silence)
        })
        .count()
}

/// Keeps recognition responsive while translation waits for complete sentences.
/// Revisions invalidate pending work before it takes a model slot; only a real
/// input boundary releases an unfinished final sentence.
pub(crate) struct SentenceGate(watch::Sender<Snapshot>);

#[derive(Clone)]
pub(crate) struct SentenceReadiness(watch::Sender<Snapshot>);

#[derive(Clone, Default)]
struct Snapshot {
    revision: u64,
    final_input: bool,
    sentences: Vec<Sentence>,
}

#[derive(Clone)]
struct Sentence {
    text: String,
    complete: bool,
}

impl SentenceGate {
    pub(crate) fn new() -> Self {
        Self(watch::channel(Snapshot::default()).0)
    }

    pub(crate) fn observe(
        &self,
        revision: u64,
        segments: &[TranslationSegmentPair],
        completed_sentences: usize,
    ) -> SentenceReadiness {
        self.0.send_modify(|snapshot| {
            let sentences = segments
                .iter()
                .enumerate()
                .map(|(index, segment)| {
                    let text = &segment.source_text;
                    Sentence {
                        text: text.clone(),
                        complete: index < completed_sentences,
                    }
                })
                .collect();
            *snapshot = Snapshot {
                revision,
                final_input: false,
                sentences,
            };
        });
        SentenceReadiness(self.0.clone())
    }

    pub(crate) fn finish(&self) {
        self.0.send_if_modified(|snapshot| {
            if snapshot.final_input {
                return false;
            }
            snapshot.final_input = true;
            true
        });
    }
}

impl SentenceReadiness {
    pub(crate) fn is_current(&self, revision: u64) -> bool {
        self.0.borrow().revision == revision
    }

    pub(crate) fn contains(&self, index: usize, source_text: &str) -> bool {
        self.0
            .borrow()
            .sentences
            .get(index)
            .is_some_and(|sentence| sentence.text == source_text)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.0.borrow().revision
    }

    pub(crate) async fn source_changed(&self, index: usize, text: &str) {
        let mut observed = self.0.subscribe();
        while observed
            .borrow_and_update()
            .sentences
            .get(index)
            .is_some_and(|sentence| sentence.text == text)
        {
            if observed.changed().await.is_err() {
                break;
            }
        }
    }

    pub(crate) async fn wait(self, revision: u64, index: usize) -> bool {
        let mut observed = self.0.subscribe();
        loop {
            {
                let snapshot = observed.borrow_and_update();
                if snapshot.revision != revision {
                    return false;
                }
                let Some(sentence) = snapshot.sentences.get(index) else {
                    return false;
                };
                if sentence.complete || snapshot.final_input {
                    return true;
                }
            }
            if observed.changed().await.is_err() {
                return false;
            }
        }
    }
}
