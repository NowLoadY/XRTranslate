//! Assemble linguistic sentences across bounded, non-revisable ASR chunks.
//!
//! An audio duration limit protects memory; it does not end the speaker's
//! sentence. Only the uncommitted tail is retained here. Committed sentences
//! leave the buffer once, while genuine input boundaries release the tail.

use std::time::Duration;

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
    /// Actual worker queue time, excluding ASR and retention across chunks.
    pub(super) queue_elapsed: Duration,
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
        // File transcription is consumed by a plugin that owns subtitle editing
        // and translation context. Preserve its audio boundaries even when the
        // recognizer omits punctuation; live translation still assembles sentences.
        if current.job.workload == xrtranslate_protocol::InferenceWorkload::Offline
            && current.job.translation_options.asr_only
        {
            if let Some(previous) = self.take_pending()
                && previous.job.generation == current.job.generation
            {
                ready.push(previous);
            }
            ready.push(current);
            return Ok(ready);
        }
        let next_tail_id = format!("{}:tail", current.job.turn_id);
        if let Some(previous) = self.pending.take() {
            if same_stream(&previous, &current) {
                let mut text = previous.recognized.source_text;
                append_text(&mut text, &current.recognized.source_text);
                current.recognized.apply_source_correction(text);
                current.recognized.asr_elapsed += previous.recognized.asr_elapsed;
                current.queue_elapsed += previous.queue_elapsed;
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

        let text = &current.recognized.source_text;
        // These are final, non-revisable results, not pseudo-streaming windows.
        // Trust reliable sentence punctuation immediately, retaining only the
        // incomplete suffix instead of waiting for the next audio/ASR window.
        let complete_end = crate::sentence_gate::completed_recognition_prefix(
            text,
            current.job.utterance.end_reason,
        );
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
                queue_elapsed: current.queue_elapsed,
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

#[cfg(test)]
mod tests {
    use super::*;
    use xrtranslate_protocol::{InferenceWorkload, TranslationOptions};

    fn recognized_chunk(
        text: &str,
        index: usize,
        workload: InferenceWorkload,
        asr_only: bool,
    ) -> PreparedRecognition {
        PreparedRecognition {
            job: UtteranceJob {
                utterance: xrtranslate_vad::Utterance {
                    samples: Vec::new(),
                    pre_roll_frames: 0,
                    overlap_frames: 0,
                    trailing_silence_frames: 0,
                    end_reason: UtteranceEndReason::AdaptiveSilence,
                },
                source_start_ms: index as f64 * 6_000.0,
                source_end_ms: (index + 1) as f64 * 6_000.0,
                revisable: false,
                generation: PipelineGeneration {
                    route_epoch: crate::RouteEpoch::INITIAL,
                    audio_epoch: crate::AudioEpoch::INITIAL,
                },
                turn_id: format!("turn-{index}"),
                topic_turn_id: format!("turn-{index}"),
                source_language: "en".into(),
                target_language: "zh".into(),
                translation_options: TranslationOptions {
                    asr_only,
                    ..Default::default()
                },
                speaker_id: None,
                workload,
                enqueued_at: tokio::time::Instant::now(),
                revision: 0,
            },
            recognized: RecognizedOutput {
                source_text: text.into(),
                segments: xrtranslate_engine::translation_segment_pairs_for_text_with_lang(
                    text, "en",
                ),
                source_language: "en".into(),
                target_language: "zh".into(),
                asr_elapsed: Duration::ZERO,
                route_switched: None,
                prompt_trace: None,
            },
            asr_context: xr_corpus_protocol::PrepareAsrResponse {
                context_id: 0,
                vocabulary: Vec::new(),
                prompt: None,
            },
            prompt_graph: xrtranslate_prompt::PromptNodeGraph::builtin_default(),
            queue_elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn offline_transcription_preserves_unpunctuated_audio_boundaries() {
        let mut assembler = SentenceAssembler::default();
        for index in 0..8 {
            let source = format!("Unpunctuated lyric line {index}");
            let input = recognized_chunk(&source, index, InferenceWorkload::Offline, true);
            let expected_start = input.job.source_start_ms;
            let expected_end = input.job.source_end_ms;
            let ready = assembler
                .push(input)
                .unwrap_or_else(|_| panic!("unexpected overflow"));
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].recognized.source_text, source);
            assert_eq!(ready[0].job.source_start_ms, expected_start);
            assert_eq!(ready[0].job.source_end_ms, expected_end);
            assert_eq!(
                ready[0].job.utterance.end_reason,
                UtteranceEndReason::AdaptiveSilence
            );
            assert!(assembler.pending().is_none());
        }
    }

    #[test]
    fn direct_translation_still_joins_an_unfinished_sentence() {
        for (workload, asr_only) in [
            (InferenceWorkload::Realtime, false),
            (InferenceWorkload::Realtime, true),
            (InferenceWorkload::Offline, false),
        ] {
            let mut assembler = SentenceAssembler::default();
            let first = assembler
                .push(recognized_chunk("We need", 0, workload, asr_only))
                .unwrap_or_else(|_| panic!("unexpected overflow"));
            assert!(first.is_empty());
            assert!(assembler.pending().is_some());
            let ready = assembler
                .push(recognized_chunk("more time.", 1, workload, asr_only))
                .unwrap_or_else(|_| panic!("unexpected overflow"));
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].recognized.source_text, "We need more time.");
            assert!(assembler.pending().is_none());
        }
    }
}
