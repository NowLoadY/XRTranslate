//! A draft assembled from the existing host recognition results.
use std::collections::BTreeMap;

use crate::session_coordinator::{TranslationEvent, TranslationSegment};
use xrtranslate_client_core::services::RecognitionInput;
use xrtranslate_engine::text_processing::sentence_end_offsets;

#[derive(Default)]
pub(crate) struct TextCollection {
    pub input: RecognitionInput,
    prefix: String,
    turns: Vec<Turn>,
    budget: Option<usize>,
}

struct Turn {
    stream: u64,
    id: String,
    segments: BTreeMap<u32, String>,
    skipped_sentences: usize,
    discarded: bool,
    finished: bool,
}

impl Turn {
    fn text(&self) -> String {
        let text = self
            .segments
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        if self.skipped_sentences == 0 {
            return text;
        }
        let start = sentence_end_offsets(&text)
            .get(self.skipped_sentences - 1)
            .copied()
            .unwrap_or(text.len());
        text[start..].trim_start().to_owned()
    }
}

impl TextCollection {
    pub fn start(&mut self, text: String) {
        if self.input.active {
            return;
        }
        // Resuming an untouched draft retains which words came from ASR, so
        // older collected sentences remain eligible for automatic eviction.
        if text != self.input.text {
            self.prefix = text;
            self.turns.clear();
        }
        self.input.active = true;
        self.input.can_copy = !self.original().is_empty();
        self.input.text = self.draft(&self.original());
        self.input.revision += 1;
    }

    pub fn stop(&mut self) -> Option<String> {
        if !self.input.active {
            return None;
        }
        self.input.active = false;
        self.input.revision += 1;
        self.input.can_copy.then(|| self.input.text.clone())
    }

    pub fn original(&self) -> String {
        self.turns
            .iter()
            .map(Turn::text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set_budget(&mut self, budget: Option<usize>) {
        if self.budget != budget {
            self.budget = budget;
            if self.input.active {
                self.refresh();
            }
        }
    }

    pub fn observe(&mut self, event: &TranslationEvent) {
        if !self.input.active {
            return;
        }
        match event {
            TranslationEvent::Segment(segment) if segment.translated.is_none() => {
                let turn = self.turn(segment);
                if turn.discarded {
                    return;
                }
                turn.finished = !segment.revisable;
                turn.segments
                    .insert(segment.segment_index, segment.source.trim().into());
            }
            TranslationEvent::ReplaceSegments(segments)
                if segments
                    .first()
                    .is_some_and(|segment| segment.translated.is_none()) =>
            {
                let turn = self.turn(&segments[0]);
                if turn.discarded {
                    return;
                }
                turn.finished = segments.iter().all(|segment| !segment.revisable);
                turn.segments = segments
                    .iter()
                    .map(|segment| (segment.segment_index, segment.source.trim().into()))
                    .collect();
            }
            _ => return,
        }
        self.refresh();
    }

    fn draft(&self, original: &str) -> String {
        match (self.prefix.trim_end(), original) {
            ("", original) => original.to_owned(),
            (prefix, "") => prefix.to_owned(),
            (prefix, original) => format!("{prefix}\n{original}"),
        }
    }

    fn refresh(&mut self) {
        if let Some(budget) = self.budget {
            // Retain the newest sentence and unfinished speech. Manual text is
            // never evicted; only the collected source is a rolling window.
            while xrtranslate_prompt::estimated_tokens(&self.draft(&self.original())) > budget {
                let newest = self.turns.iter().rposition(|turn| !turn.text().is_empty());
                let removed = self.turns.iter_mut().enumerate().any(|(index, turn)| {
                    let text = turn.text();
                    if text.is_empty() {
                        return false;
                    }
                    let end = sentence_end_offsets(&text)
                        .first()
                        .copied()
                        .or_else(|| (turn.finished && Some(index) != newest).then_some(text.len()));
                    let Some(end) = end else {
                        return false;
                    };
                    let empty = text[end..].trim().is_empty();
                    if empty && Some(index) == newest {
                        return false;
                    }
                    turn.skipped_sentences += 1;
                    if empty && turn.finished {
                        turn.discarded = true;
                        turn.segments.clear();
                    }
                    true
                });
                if !removed {
                    break;
                }
            }
        }
        let original = self.original();
        self.input.can_copy = !original.is_empty();
        let text = self.draft(&original);
        if self.input.text != text {
            self.input.text = text;
            self.input.revision += 1;
        }
    }

    fn turn(&mut self, segment: &TranslationSegment) -> &mut Turn {
        let index = self
            .turns
            .iter()
            .position(|turn| turn.stream == segment.stream_id && turn.id == segment.turn_id)
            .unwrap_or_else(|| {
                self.turns.push(Turn {
                    stream: segment.stream_id,
                    id: segment.turn_id.clone(),
                    segments: BTreeMap::new(),
                    skipped_sentences: 0,
                    discarded: false,
                    finished: false,
                });
                self.turns.len() - 1
            });
        &mut self.turns[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureSource;
    use xrtranslate_protocol::{SegmentBoundary, SegmentTiming};

    fn segment(turn: &str, index: u32, text: &str) -> TranslationSegment {
        TranslationSegment {
            stream_id: 1,
            audio_source: CaptureSource::Microphone,
            turn_id: turn.into(),
            segment_index: index,
            segment_count: 1,
            source: text.into(),
            translated: None,
            source_lang: "en".into(),
            target_lang: "zh".into(),
            additional_translations: Vec::new(),
            asr_only: false,
            speaker_id: String::new(),
            source_start_ms: 0.0,
            source_end_ms: 0.0,
            timing: SegmentTiming::Unknown,
            boundary: SegmentBoundary::Unknown,
            revisable: true,
            live: true,
        }
    }

    #[test]
    fn revisions_replace_the_turn_without_duplicating_the_draft() {
        let mut collection = TextCollection::default();
        collection.start("Question:".into());
        collection.observe(&TranslationEvent::Segment(segment("first", 1, "Hello")));
        collection.observe(&TranslationEvent::Segment(segment("first", 2, "world")));
        collection.observe(&TranslationEvent::Segment(segment(
            "second",
            1,
            "How are you?",
        )));
        collection.observe(&TranslationEvent::ReplaceSegments(vec![segment(
            "first",
            1,
            "Hello, world!",
        )]));
        assert_eq!(
            collection.input.text,
            "Question:\nHello, world!\nHow are you?"
        );
        assert_eq!(collection.original(), "Hello, world!\nHow are you?");
        let revision = collection.input.revision;
        let mut translated = segment("first", 1, "Hello, world!");
        translated.translated = Some("你好，世界！".into());
        collection.observe(&TranslationEvent::Segment(translated));
        assert_eq!(collection.input.revision, revision);
    }

    #[test]
    fn collecting_neither_replays_old_results_nor_changes_the_finished_draft() {
        let mut collection = TextCollection::default();
        let event = TranslationEvent::Segment(segment("first", 1, "Hello"));
        collection.observe(&event);
        collection.start("".into());
        assert!(collection.input.text.is_empty());
        collection.observe(&event);
        assert_eq!(collection.stop().as_deref(), Some("Hello"));
        collection.observe(&TranslationEvent::Segment(segment("second", 1, "Goodbye")));
        assert_eq!(collection.input.text, "Hello");
        assert!(collection.stop().is_none());
        collection.start("My draft".into());
        assert_eq!(collection.input.text, "My draft");
        assert!(!collection.input.can_copy);
        assert!(collection.stop().is_none());
    }

    #[test]
    fn capacity_evicts_old_speech_without_erasing_manual_text_or_reviving_old_turns() {
        let mut collection = TextCollection::default();
        collection.start("Manual note.".into());
        collection.set_budget(Some(9));
        let mut old = segment("first", 1, "Old sentence.");
        old.revisable = false;
        collection.observe(&TranslationEvent::Segment(old.clone()));
        collection.observe(&TranslationEvent::Segment(segment("second", 1, "Newest.")));
        assert_eq!(collection.input.text, "Manual note.\nNewest.");
        collection.observe(&TranslationEvent::ReplaceSegments(vec![old]));
        assert_eq!(collection.original(), "Newest.");
        let draft = collection.stop().unwrap();
        collection.start(draft);
        collection.observe(&TranslationEvent::Segment(segment("third", 1, "Latest.")));
        assert_eq!(collection.input.text, "Manual note.\nLatest.");
    }

    #[test]
    fn capacity_preserves_the_unfinished_tail_across_recognition_revisions() {
        let mut collection = TextCollection::default();
        collection.start(String::new());
        collection.set_budget(Some(12));
        collection.observe(&TranslationEvent::ReplaceSegments(vec![segment(
            "first",
            1,
            "One complete sentence. Another complete sentence. still speaking",
        )]));
        assert_eq!(collection.original(), "still speaking");
        collection.observe(&TranslationEvent::ReplaceSegments(vec![segment(
            "first",
            1,
            "One complete sentence. Another complete sentence. still speaking clearly",
        )]));
        assert_eq!(collection.original(), "still speaking clearly");
        collection.set_budget(Some(1));
        assert_eq!(collection.original(), "still speaking clearly");
    }
}
