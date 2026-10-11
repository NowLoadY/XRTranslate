//! A one-request recognition hint, refreshed at phrase boundaries only.

use xrtranslate_engine::sentence_end_offsets;
use xrtranslate_vad::UtteranceEndReason;

use crate::{PipelineGeneration, UtteranceJob};

#[derive(Default)]
pub(crate) struct RecognitionBias {
    scope: Option<(PipelineGeneration, String)>,
    language: String,
    speaker: String,
    phrase: String,
    last_boundary: Option<(usize, String)>,
    completed_sentences: usize,
}

impl RecognitionBias {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Consuming the hint prevents it from following unrelated later windows.
    pub(crate) fn take(&mut self, job: &UtteranceJob) -> String {
        if !job.revisable
            || self.scope.as_ref().is_none_or(|(generation, turn)| {
                *generation != job.generation || *turn != job.topic_turn_id
            })
            || (job.source_language != "auto" && job.source_language != self.language)
            || job
                .speaker_id
                .as_ref()
                .is_some_and(|speaker| *speaker != self.speaker)
        {
            self.clear();
        }
        std::mem::take(&mut self.phrase)
    }

    pub(crate) fn observe(
        &mut self,
        job: &UtteranceJob,
        text: &str,
        language: &str,
        speaker: &str,
        completed_sentences: usize,
    ) {
        if self.scope.as_ref().is_none_or(|(generation, turn)| {
            *generation != job.generation || *turn != job.topic_turn_id
        }) || self.language != language
            || self.speaker != speaker
        {
            self.clear();
            self.scope = Some((job.generation, job.topic_turn_id.clone()));
            self.language = language.to_owned();
            self.speaker = speaker.to_owned();
        }
        let mut boundaries = sentence_end_offsets(text);
        boundaries.extend(text.char_indices().filter_map(|(offset, character)| {
            matches!(character, ',' | '，' | '、' | ';' | '；' | ':' | '：')
                .then_some(offset + character.len_utf8())
        }));
        if matches!(
            job.utterance.end_reason,
            UtteranceEndReason::Silence | UtteranceEndReason::AdaptiveSilence
        ) {
            boundaries.push(text.trim_end().len());
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        let boundary = boundaries
            .last()
            .copied()
            .filter(|end| *end > 0)
            .map(|end| {
                let start = boundaries.iter().rev().nth(1).copied().unwrap_or(0);
                let phrase = text[start..end].trim();
                // A small recent phrase is enough; do not grow a second transcript.
                let start = phrase
                    .char_indices()
                    .rev()
                    .nth(159)
                    .map_or(0, |(offset, _)| offset);
                (end, phrase[start..].to_owned())
            });
        if self.completed_sentences != completed_sentences {
            // Clear before translation is scheduled, including a revised
            // sentence boundary. Do not seed a hint from the completed sentence.
            self.phrase.clear();
            self.completed_sentences = completed_sentences;
            self.last_boundary = boundary;
        } else if boundary != self.last_boundary {
            self.phrase = boundary
                .as_ref()
                .map_or_else(String::new, |(_, text)| text.clone());
            self.last_boundary = boundary;
        }
    }
}
