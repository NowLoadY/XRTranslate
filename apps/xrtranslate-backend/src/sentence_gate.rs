use std::time::Duration;

use tokio::{sync::watch, time::Instant};
use xrtranslate_engine::{TranslationSegmentPair, ends_at_sentence_boundary};

const SENTENCE_STABILITY: Duration = Duration::from_millis(300);

/// Keeps recognition responsive while translation waits for stable sentences.
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
    changed_at: Instant,
}

impl SentenceGate {
    pub(crate) fn new() -> Self {
        Self(watch::channel(Snapshot::default()).0)
    }

    pub(crate) fn observe(
        &self,
        revision: u64,
        segments: &[TranslationSegmentPair],
        final_input: bool,
    ) -> SentenceReadiness {
        let now = Instant::now();
        self.0.send_modify(|snapshot| {
            let sentences = segments
                .iter()
                .enumerate()
                .map(|(index, segment)| {
                    let text = &segment.source_text;
                    let previous = snapshot
                        .sentences
                        .get(index)
                        .filter(|old| old.text == *text);
                    Sentence {
                        text: text.clone(),
                        complete: ends_at_sentence_boundary(&segment.source_text),
                        changed_at: previous.map_or(now, |old| old.changed_at),
                    }
                })
                .collect();
            *snapshot = Snapshot {
                revision,
                final_input,
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
            for sentence in &mut snapshot.sentences {
                if !sentence.complete {
                    sentence.changed_at = Instant::now();
                }
            }
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

    pub(crate) async fn revised(&self, revision: u64) {
        let mut observed = self.0.subscribe();
        while observed.borrow_and_update().revision == revision {
            if observed.changed().await.is_err() {
                break;
            }
        }
    }

    pub(crate) async fn wait(self, revision: u64, index: usize) -> bool {
        let mut observed = self.0.subscribe();
        loop {
            let deadline = {
                let snapshot = observed.borrow_and_update();
                if snapshot.revision != revision {
                    return false;
                }
                let Some(sentence) = snapshot.sentences.get(index) else {
                    return false;
                };
                (sentence.complete || snapshot.final_input)
                    .then_some(sentence.changed_at + SENTENCE_STABILITY)
            };
            if let Some(deadline) = deadline {
                tokio::select! {
                    biased;
                    changed = observed.changed() => if changed.is_err() { return false; },
                    () = tokio::time::sleep_until(deadline) => return true,
                }
            } else if observed.changed().await.is_err() {
                return false;
            }
        }
    }
}
