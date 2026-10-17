//! Task-owned context, maintained beside inference rather than in the vocabulary service.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
};
use xrtranslate_prompt::{ContextFacts, PromptTurn, TaskContext, TranslationPromptContext};

const MAX_TURNS: usize = 32;
const MAX_CHARACTERS: usize = 16_384;

#[derive(Clone, Default)]
pub(crate) struct ContextRegistry(Arc<Mutex<Vec<(String, Weak<Mutex<History>>)>>>);
impl ContextRegistry {
    pub(crate) fn bind(&self, context: TaskContext) -> ContextBinding {
        match context {
            TaskContext::Snapshot { facts } => ContextBinding::Snapshot(facts),
            TaskContext::Conversation { scope, seed } => {
                if scope.trim().is_empty() {
                    return ContextBinding::Snapshot(seed);
                }
                let ContextFacts {
                    recent_turns,
                    surrounding_source,
                } = seed;
                let mut scopes = self.0.lock().unwrap_or_else(|error| error.into_inner());
                scopes.retain(|(_, memory)| memory.strong_count() > 0);
                let memory = scopes
                    .iter()
                    .find(|(id, _)| id == &scope)
                    .and_then(|(_, memory)| memory.upgrade())
                    .unwrap_or_else(|| {
                        let mut history = History::default();
                        for (index, mut turn) in recent_turns.into_iter().enumerate() {
                            let id = format!("seed-{index}");
                            turn.turn_id = Some(id.clone());
                            history.observe(&id);
                            history.record(turn, 0);
                        }
                        history.surrounding_source = surrounding_source;
                        let memory = Arc::new(Mutex::new(history));
                        scopes.push((scope, Arc::downgrade(&memory)));
                        memory
                    });
                ContextBinding::Conversation(memory)
            }
        }
    }
}

/// Exactly one owner: immutable caller facts or backend conversation memory.
#[derive(Clone)]
pub(crate) enum ContextBinding {
    Snapshot(ContextFacts),
    Conversation(Arc<Mutex<History>>),
}
impl Default for ContextBinding {
    fn default() -> Self {
        Self::Snapshot(ContextFacts::default())
    }
}
#[derive(Default)]
pub(crate) struct History {
    next_order: u64,
    observed: VecDeque<(String, u64)>,
    turns: Vec<(u64, u64, PromptTurn)>,
    surrounding_source: Option<xrtranslate_prompt::SurroundingSource>,
}
impl History {
    fn observe(&mut self, id: &str) {
        if self.observed.iter().any(|(existing, _)| existing == id) {
            return;
        }
        self.observed.push_back((id.to_owned(), self.next_order));
        self.next_order += 1;
        while self.observed.len() > MAX_TURNS * 2 {
            self.observed.pop_front();
        }
    }
    fn record(&mut self, turn: PromptTurn, revision: u64) {
        let Some(id) = turn.turn_id.as_deref() else {
            return;
        };
        if let Some((_, previous_revision, previous)) =
            self.turns.iter_mut().find(|(_, _, previous)| {
                previous.turn_id.as_deref() == Some(id)
                    && previous.target_language == turn.target_language
            })
        {
            if revision >= *previous_revision {
                *previous_revision = revision;
                *previous = turn;
                self.surrounding_source = None;
            }
        } else if let Some((_, order)) = self.observed.iter().find(|(existing, _)| existing == id) {
            self.turns.push((*order, revision, turn));
            self.turns.sort_by_key(|(order, _, _)| *order);
            self.surrounding_source = None;
        }
        while self.turns.len() > MAX_TURNS
            || self
                .turns
                .iter()
                .map(|(_, _, turn)| {
                    turn.source_text.chars().count() + turn.translated_text.chars().count()
                })
                .sum::<usize>()
                > MAX_CHARACTERS
        {
            self.turns.remove(0);
        }
    }
}
impl ContextBinding {
    pub(crate) fn is_snapshot(&self) -> bool {
        matches!(self, Self::Snapshot(_))
    }

    pub(crate) fn observe(&self, id: &str) {
        if let Self::Conversation(memory) = self {
            memory
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .observe(id);
        }
    }
    pub(crate) fn snapshot(
        &self,
        id: &str,
        speaker: &str,
        source: &str,
        target: &str,
    ) -> TranslationPromptContext {
        let (turns, surrounding_source) = match self {
            Self::Snapshot(facts) => (facts.recent_turns.clone(), facts.surrounding_source.clone()),
            Self::Conversation(memory) => {
                let memory = memory.lock().unwrap_or_else(|error| error.into_inner());
                (
                    memory
                        .turns
                        .iter()
                        .map(|(_, _, turn)| turn.clone())
                        .collect(),
                    memory.surrounding_source.clone(),
                )
            }
        };
        let mut context = TranslationPromptContext {
            current_speaker_id: surrounding_source
                .as_ref()
                .map(|source| source.speaker_id.trim())
                .filter(|speaker| !speaker.is_empty())
                .unwrap_or(speaker)
                .to_owned(),
            language_order: vec![source.to_owned(), target.to_owned()],
            surrounding_source,
            ..Default::default()
        };
        for turn in turns.into_iter().filter(|turn| {
            turn.target_language == target
                || (turn.source_language == target && turn.target_language == source)
        }) {
            if turn.turn_id.as_deref() == Some(id) {
                context.previous_revision = Some(turn);
            } else {
                context.recent_turns.push(turn);
            }
        }
        context
    }
    pub(crate) fn record(&self, turn: PromptTurn, revision: u64) {
        if turn.source_language == turn.target_language
            || turn.source_text.trim().is_empty()
            || turn.translated_text.trim().is_empty()
        {
            return;
        }
        if let Self::Conversation(memory) = self {
            memory
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .record(turn, revision);
        }
    }
}

/// Input-lane evidence for echo rejection, never an ASR prompt or shared conversation.
#[derive(Clone, Default)]
pub(crate) struct RecognitionEvidence(Arc<Mutex<VecDeque<(String, Vec<String>)>>>);
impl RecognitionEvidence {
    pub(crate) fn record(&self, id: &str, texts: Vec<String>) {
        let mut turns = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let texts = texts
            .into_iter()
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.chars().take(512).collect())
            .collect();
        if let Some((_, previous)) = turns.iter_mut().find(|(existing, _)| existing == id) {
            *previous = texts;
        } else {
            turns.push_back((id.to_owned(), texts));
        }
        while turns.len() > 6 {
            turns.pop_front();
        }
    }
    pub(crate) fn texts(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .flat_map(|(_, texts)| texts.iter().cloned())
            .collect()
    }
}

pub(crate) fn context_evidence(context: &TranslationPromptContext) -> Vec<String> {
    let mut evidence = context
        .recent_turns
        .iter()
        .flat_map(|turn| [turn.source_text.clone(), turn.translated_text.clone()])
        .collect::<Vec<_>>();
    if let Some(source) = &context.surrounding_source {
        evidence.extend([source.before.clone(), source.after.clone()]);
    }
    evidence
}

pub(crate) fn join_segments<'a>(
    language: &str,
    segments: impl IntoIterator<Item = &'a str>,
) -> String {
    let separate = !matches!(
        language.trim().split(['-', '_']).next().unwrap_or_default(),
        "zh" | "ja"
    );
    let mut joined = String::new();
    for text in segments
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        if separate && !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(text);
    }
    joined
}

pub(crate) fn completed_turn(
    id: String,
    speaker: String,
    source: &str,
    target: &str,
    pairs: &[(String, String)],
) -> Option<PromptTurn> {
    (!pairs.is_empty()).then(|| PromptTurn {
        turn_id: Some(id),
        speaker_id: speaker,
        source_language: source.to_owned(),
        target_language: target.to_owned(),
        source_text: join_segments(source, pairs.iter().map(|(source, _)| source.as_str())),
        translated_text: join_segments(
            target,
            pairs.iter().map(|(_, translated)| translated.as_str()),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turn(id: &str, text: &str, target: &str) -> PromptTurn {
        PromptTurn {
            turn_id: Some(id.into()),
            speaker_id: "speaker".into(),
            source_language: "en".into(),
            target_language: target.into(),
            source_text: text.into(),
            translated_text: format!("translated {text}"),
        }
    }
    #[test]
    fn shared_lanes_keep_observation_order_and_latest_revision() {
        let registry = ContextRegistry::default();
        let microphone = registry.bind(TaskContext::conversation("meeting"));
        let system = registry.bind(TaskContext::conversation("meeting"));
        microphone.observe("mic/1");
        system.observe("system/1");
        system.record(turn("system/1", "second", "zh"), 1);
        microphone.record(turn("mic/1", "first revised", "zh"), 2);
        microphone.record(turn("mic/1", "stale", "zh"), 1);
        let context = microphone.snapshot("next", "", "en", "zh");
        assert_eq!(
            context
                .recent_turns
                .iter()
                .map(|turn| turn.source_text.as_str())
                .collect::<Vec<_>>(),
            ["first revised", "second"]
        );
        let revision = microphone.snapshot("mic/1", "", "en", "zh");
        assert_eq!(revision.recent_turns.len(), 1);
        assert_eq!(
            revision.previous_revision.unwrap().source_text,
            "first revised"
        );
    }
    #[test]
    fn snapshots_and_other_scopes_never_inherit_dialogue() {
        let registry = ContextRegistry::default();
        let live = registry.bind(TaskContext::conversation("live"));
        live.observe("one");
        live.record(turn("one", "hello", "zh"), 1);
        live.record(turn("one", "bonjour", "fr"), 1);
        assert_eq!(live.snapshot("next", "", "en", "zh").recent_turns.len(), 1);
        let ocr = registry.bind(TaskContext::default());
        ocr.observe("ocr");
        ocr.record(turn("ocr", "screen", "zh"), 1);
        assert!(ocr.snapshot("next", "", "en", "zh").recent_turns.is_empty());
        for text in ["document A", "document B"] {
            let facts = ContextFacts {
                recent_turns: vec![turn("previous", text, "zh")],
                ..ContextFacts::default()
            };
            let snapshot = registry.bind(TaskContext::snapshot(facts.clone()));
            snapshot.observe("current");
            snapshot.record(turn("current", "must not become history", "zh"), 1);
            assert_eq!(
                snapshot.snapshot("next", "", "en", "zh").recent_turns,
                facts.recent_turns
            );
        }
        assert_eq!(
            live.snapshot("next", "", "en", "zh").recent_turns,
            vec![turn("one", "hello", "zh")]
        );
        assert!(
            registry
                .bind(TaskContext::conversation("other"))
                .snapshot("next", "", "en", "zh")
                .recent_turns
                .is_empty()
        );
    }
    #[test]
    fn conversation_seed_keeps_neighbors_without_reseeding_live_memory() {
        let registry = ContextRegistry::default();
        let facts = ContextFacts {
            recent_turns: vec![turn("existing", "earlier", "zh")],
            surrounding_source: Some(xrtranslate_prompt::SurroundingSource {
                speaker_id: String::new(),
                source_language: "en".into(),
                before: "before".into(),
                after: "after".into(),
            }),
        };
        let first = registry.bind(TaskContext::Conversation {
            scope: "meeting".into(),
            seed: facts.clone(),
        });
        let second = registry.bind(TaskContext::Conversation {
            scope: "meeting".into(),
            seed: facts.clone(),
        });
        let context = second.snapshot("next", "", "en", "zh");
        assert_eq!(context.surrounding_source, facts.surrounding_source);
        assert_eq!(context.recent_turns.len(), 1);
        assert_eq!(first.snapshot("next", "", "en", "zh").recent_turns.len(), 1);
        first.observe("new");
        first.record(turn("new", "new recording", "zh"), 1);
        assert!(
            second
                .snapshot("next", "", "en", "zh")
                .surrounding_source
                .is_none()
        );
    }
}
