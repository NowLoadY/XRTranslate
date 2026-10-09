//! Assemble linguistic sentences across bounded, non-revisable ASR chunks.
//!
//! An audio duration limit protects memory; it does not end the speaker's
//! sentence. Only the uncommitted tail is retained here. Committed sentences
//! leave the buffer once, while genuine input boundaries release the tail.

use xrtranslate_engine::sentence_end_offsets;
use xrtranslate_vad::UtteranceEndReason;

use crate::{PipelineGeneration, UtteranceJob, pipeline::RecognizedOutput};

/// A pathological unpunctuated stream must stop with a recoverable diagnostic,
/// rather than consume unbounded memory or silently translate arbitrary cuts.
pub(super) const MAX_PENDING_CHARACTERS: usize = 4096;

#[derive(Clone)]
pub(super) struct PreparedRecognition {
    pub(super) job: UtteranceJob,
    pub(super) recognized: RecognizedOutput,
    pub(super) asr_context: xr_corpus_protocol::PrepareAsrResponse,
    pub(super) prompt_graph: xrtranslate_prompt::PromptNodeGraph,
}

#[derive(Default)]
pub(super) struct SentenceAssembler {
    pending: Option<PreparedRecognition>,
}

pub(super) struct AssemblyOverflow {
    pub(super) visible: PreparedRecognition,
    pub(super) ready: Vec<PreparedRecognition>,
}

impl SentenceAssembler {
    pub(super) fn reset_unless_generation(&mut self, generation: PipelineGeneration) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.job.generation != generation)
        {
            self.pending = None;
        }
    }

    pub(super) fn take_pending(&mut self) -> Option<PreparedRecognition> {
        self.pending.take().map(|mut pending| {
            pending.job.utterance.end_reason = UtteranceEndReason::Flushed;
            pending
        })
    }

    pub(super) fn pending(&self) -> Option<&PreparedRecognition> {
        self.pending.as_ref()
    }

    /// Input has already had duplicated audio text removed and its speaker
    /// resolved. Different speakers/languages never share an unfinished tail.
    pub(super) fn push(
        &mut self,
        mut current: PreparedRecognition,
    ) -> Result<Vec<PreparedRecognition>, AssemblyOverflow> {
        let mut ready = Vec::new();
        let next_tail_id = format!("{}:tail", current.job.turn_id);
        if let Some(previous) = self.pending.take() {
            if same_stream(&previous, &current) {
                let mut text = previous.recognized.source_text;
                append_text(&mut text, &current.recognized.source_text);
                current.recognized.apply_source_correction(text);
                current.recognized.asr_elapsed += previous.recognized.asr_elapsed;
                current.job.source_start_ms = previous.job.source_start_ms;
                current.job.turn_id = previous.job.turn_id;
                current.job.topic_turn_id = previous.job.topic_turn_id;
                current.job.enqueued_at = previous.job.enqueued_at;
            } else if previous.job.generation == current.job.generation {
                let mut previous = previous;
                previous.job.utterance.end_reason = UtteranceEndReason::SpeakerChange;
                ready.push(previous);
            }
        }

        if current.job.utterance.end_reason != UtteranceEndReason::MaxActiveFrames {
            ready.push(current);
            return Ok(ready);
        }

        let text = &current.recognized.source_text;
        // Offline ASR can insert a period merely because its audio ended.
        // Hold the right-edge sentence for one chunk of lookahead; preserve
        // its punctuation rather than guessing that a genuine period is wrong.
        // This is conservative chunk lookahead, not independent ASR agreement.
        let right_edge = text.trim_end().len();
        let complete_end = sentence_end_offsets(text)
            .into_iter()
            .filter(|end| *end < right_edge)
            .next_back()
            .unwrap_or(0);
        let tail = text[complete_end..].trim().to_owned();
        if tail.is_empty() {
            ready.push(current);
            return Ok(ready);
        }
        if complete_end > 0 {
            // ASR has no word alignment in the neutral result contract. Use
            // explicitly estimated text partitions, never claimed timestamps.
            let prefix = text[..complete_end].trim().to_owned();
            let fraction =
                text[..complete_end].chars().count() as f64 / text.chars().count().max(1) as f64;
            let split_ms = current.job.source_start_ms
                + (current.job.source_end_ms - current.job.source_start_ms).max(0.0) * fraction;
            let mut completed = PreparedRecognition {
                job: current.job.clone(),
                recognized: current.recognized.clone(),
                asr_context: current.asr_context.clone(),
                prompt_graph: current.prompt_graph.clone(),
            };
            completed.job.source_end_ms = split_ms;
            completed.recognized.apply_source_correction(prefix);
            ready.push(completed);
            current.job.source_start_ms = split_ms;
            current.job.turn_id = next_tail_id.clone();
            current.job.topic_turn_id = next_tail_id;
        }
        current.recognized.apply_source_correction(tail);
        if current.recognized.source_text.chars().count() > MAX_PENDING_CHARACTERS {
            return Err(AssemblyOverflow {
                visible: current,
                ready,
            });
        }
        self.pending = Some(current);
        Ok(ready)
    }
}

fn same_stream(previous: &PreparedRecognition, current: &PreparedRecognition) -> bool {
    previous.job.generation == current.job.generation
        && previous.job.speaker_id == current.job.speaker_id
        && previous.recognized.source_language == current.recognized.source_language
        && previous.recognized.target_language == current.recognized.target_language
}

fn append_text(previous: &mut String, next: &str) {
    let next = next.trim();
    if next.is_empty() {
        return;
    }
    let needs_space = previous
        .chars()
        .next_back()
        .zip(next.chars().next())
        .is_some_and(|(left, right)| {
            !left.is_whitespace()
                && !right.is_whitespace()
                && !is_compact_script(left)
                && !is_compact_script(right)
                && !matches!(
                    right,
                    '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '，' | '。'
                )
        });
    if needs_space {
        previous.push(' ');
    }
    previous.push_str(next);
}

fn is_compact_script(character: char) -> bool {
    matches!(character, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}
