//! Bounded observation of normalized dialogue facts, without owning their pipelines.
mod commands;

pub(crate) use commands::Command;

use crate::{
    CaptureSource,
    session_coordinator::{
        TranslationEvent, TranslationOutcome, TranslationSegment, TranslationSessionOwner,
    },
};
use std::collections::{BTreeMap, VecDeque};

const MAX_TURNS: usize = 48;
const MAX_PARTS: u32 = 128;
const MAX_REMEMBERED: usize = 256;
const MAX_PENDING: usize = 16;

struct Part {
    segment: TranslationSegment,
    source_observed: bool,
}

pub(crate) struct Turn {
    key: (u64, String),
    pub owner: TranslationSessionOwner,
    pub is_text: bool,
    parts: BTreeMap<u32, Part>,
}

impl Turn {
    pub fn segments(
        &self,
    ) -> impl DoubleEndedIterator<Item = &TranslationSegment> + ExactSizeIterator {
        self.parts.values().map(|part| &part.segment)
    }

    fn source_text(&self) -> String {
        let unspaced = |c: char| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0x3040..=0x30FF | 0x31F0..=0x31FF);
        let mut text = String::new();
        for segment in self.segments() {
            let part = segment.source.trim();
            if part.is_empty() {
                continue;
            }
            // Only the segment boundary changes: punctuation and words inside
            // a fragment remain intact for exact whole-utterance matching.
            if !text.is_empty()
                && !(text.chars().next_back().is_some_and(unspaced)
                    && part.chars().next().is_some_and(unspaced))
            {
                text.push(' ');
            }
            text.push_str(part);
        }
        text
    }

    fn complete_source(&self) -> bool {
        let Some(first) = self.segments().next() else {
            return false;
        };
        let count = first.segment_count;
        self.parts.len() == count as usize
            && self.parts.values().enumerate().all(|(index, part)| {
                part.source_observed
                    && part.segment.segment_index == index as u32 + 1
                    && part.segment.segment_count == count
                    && !part.segment.revisable
                    && !part.segment.live
            })
    }

    fn merge(&mut self, incoming: &[TranslationSegment], replace: bool) {
        let source = incoming[0].translated.is_none();
        if source {
            self.parts
                .retain(|index, _| *index <= incoming[0].segment_count);
        }
        let previous = if replace && source {
            std::mem::take(&mut self.parts)
        } else {
            if replace {
                self.parts.retain(|_, part| {
                    part.segment.translated = None;
                    part.source_observed
                });
            }
            BTreeMap::new()
        };
        for segment in incoming {
            let index = segment.segment_index;
            if source {
                let mut segment = segment.clone();
                segment.translated = previous
                    .get(&index)
                    .or_else(|| self.parts.get(&index))
                    .filter(|part| part.segment.source == segment.source)
                    .and_then(|part| part.segment.translated.clone());
                self.parts.insert(
                    index,
                    Part {
                        segment,
                        source_observed: true,
                    },
                );
            } else if let Some(part) = self.parts.get_mut(&index)
                && part.source_observed
            {
                // Translations cannot replace authoritative recognition or its finality.
                if part.segment.source == segment.source {
                    part.segment.translated = segment.translated.clone();
                }
            } else {
                self.parts.insert(
                    index,
                    Part {
                        segment: segment.clone(),
                        source_observed: false,
                    },
                );
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Reader {
    turns: VecDeque<Turn>,
    consumed: VecDeque<(u64, String)>,
    retired: VecDeque<u64>,
    pending: VecDeque<(u64, Command)>,
}

impl Reader {
    pub fn context(&self) -> impl DoubleEndedIterator<Item = &Turn> + ExactSizeIterator {
        self.turns.iter()
    }

    pub fn observe(
        &mut self,
        owner: &TranslationSessionOwner,
        is_text: bool,
        event: &TranslationEvent,
    ) {
        let (incoming, replace) = match event {
            TranslationEvent::Segment(segment) => (std::slice::from_ref(segment), false),
            TranslationEvent::ReplaceSegments(segments) => (segments.as_slice(), true),
            TranslationEvent::Finished { stream_id, outcome }
                if !matches!(outcome, TranslationOutcome::Completed) =>
            {
                self.retire_stream(*stream_id);
                return;
            }
            // A normal end can follow a final utterance in the same pump: do not
            // discard its command before the application has drained the queue.
            _ => return,
        };
        let Some(first) = incoming.first() else {
            return;
        };
        if self.retired.contains(&first.stream_id)
            || incoming.len() > MAX_PARTS as usize
            || incoming.iter().any(|segment| {
                segment.stream_id != first.stream_id
                    || segment.turn_id != first.turn_id
                    || segment.audio_source != first.audio_source
                    || segment.translated.is_some() != first.translated.is_some()
                    || segment.segment_count == 0
                    || segment.segment_count > MAX_PARTS
                    || segment.segment_index == 0
                    || segment.segment_index > segment.segment_count
            })
        {
            return;
        }
        let key = (first.stream_id, first.turn_id.clone());
        let index = self
            .turns
            .iter()
            .position(|turn| turn.key == key)
            .unwrap_or_else(|| {
                if self.turns.len() == MAX_TURNS {
                    self.turns.pop_front();
                }
                self.turns.push_back(Turn {
                    key: key.clone(),
                    owner: owner.clone(),
                    is_text,
                    parts: BTreeMap::new(),
                });
                self.turns.len() - 1
            });
        self.turns[index].merge(incoming, replace);
        let turn = self.context().nth(index).unwrap();
        if first.translated.is_some() || !turn.complete_source() || self.consumed.contains(&key) {
            return;
        }
        let command = (turn.owner.is_host()
            && !turn.is_text
            && turn
                .segments()
                .all(|segment| segment.audio_source == CaptureSource::Microphone))
        .then(|| turn.source_text())
        .and_then(|source| Command::parse(&source));
        // Consume every complete source turn, including non-commands: a later
        // revision or UI history clear must never turn it into a fresh action.
        if self.consumed.len() == MAX_REMEMBERED {
            self.consumed.pop_front();
        }
        self.consumed.push_back(key);
        if let Some(command) = command
            && self.pending.len() < MAX_PENDING
        {
            self.pending.push_back((first.stream_id, command));
        }
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.pending.drain(..).map(|(_, command)| command).collect()
    }

    pub fn retire_stream(&mut self, stream_id: u64) {
        self.pending.retain(|(stream, _)| *stream != stream_id);
        if !self.retired.contains(&stream_id) {
            if self.retired.len() == MAX_REMEMBERED {
                self.retired.pop_front();
            }
            self.retired.push_back(stream_id);
        }
    }

    pub fn clear_history(&mut self) {
        self.turns.clear();
        self.pending.clear();
    }
}
