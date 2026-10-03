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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{AudioEpoch, RouteEpoch};
    use tokio::time::Instant;
    use xrtranslate_protocol::InferenceWorkload;
    use xrtranslate_vad::Utterance;

    fn chunk(text: &str, id: &str, start: f64, reason: UtteranceEndReason) -> PreparedRecognition {
        let mut recognized = RecognizedOutput {
            source_text: String::new(),
            segments: Vec::new(),
            source_language: "en".into(),
            target_language: "zh".into(),
            asr_elapsed: Duration::from_millis(10),
            route_switched: None,
            prompt_trace: None,
        };
        recognized.apply_source_correction(text.into());
        PreparedRecognition {
            job: UtteranceJob {
                utterance: Utterance {
                    samples: Vec::new(),
                    pre_roll_frames: 0,
                    overlap_frames: 0,
                    trailing_silence_frames: 0,
                    end_reason: reason,
                },
                source_start_ms: start,
                source_end_ms: start + 8000.0,
                revisable: false,
                generation: PipelineGeneration {
                    route_epoch: RouteEpoch::INITIAL,
                    audio_epoch: AudioEpoch::INITIAL,
                },
                turn_id: id.into(),
                topic_turn_id: id.into(),
                source_language: "en".into(),
                target_language: "zh".into(),
                translation_options: Default::default(),
                speaker_id: Some("speaker-1".into()),
                workload: InferenceWorkload::Realtime,
                enqueued_at: Instant::now(),
                revision: 1,
            },
            recognized,
            asr_context: xr_corpus_protocol::PrepareAsrResponse {
                context_id: 1,
                vocabulary: Vec::new(),
                prompt: None,
                echo_guard: Vec::new(),
            },
            prompt_graph: xrtranslate_prompt::PromptNodeGraph::builtin_default(),
        }
    }

    fn push(
        buffer: &mut SentenceAssembler,
        chunk: PreparedRecognition,
    ) -> Vec<PreparedRecognition> {
        buffer
            .push(chunk)
            .unwrap_or_else(|_| panic!("unexpected overflow"))
    }

    #[test]
    fn dense_chunks_keep_tail_and_never_retranslate_completed_prefix() {
        let mut buffer = SentenceAssembler::default();
        let first = push(
            &mut buffer,
            chunk(
                "Ready. The contract",
                "a",
                0.0,
                UtteranceEndReason::MaxActiveFrames,
            ),
        );
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].recognized.source_text, "Ready.");
        let tail_id = buffer.pending().unwrap().job.turn_id.clone();
        let second = push(
            &mut buffer,
            chunk(
                "will start tomorrow. Please",
                "b",
                8000.0,
                UtteranceEndReason::MaxActiveFrames,
            ),
        );
        assert_eq!(second.len(), 1);
        assert_eq!(
            second[0].recognized.source_text,
            "The contract will start tomorrow."
        );
        assert_eq!(second[0].job.turn_id, tail_id);
        assert!(second[0].job.source_start_ms < 8000.0);
        assert_eq!(buffer.pending().unwrap().recognized.source_text, "Please");
        let end = push(
            &mut buffer,
            chunk("review it", "c", 16000.0, UtteranceEndReason::Silence),
        );
        assert_eq!(end[0].recognized.source_text, "Please review it");
        assert!(buffer.pending().is_none());
    }

    #[test]
    fn silence_speaker_change_and_empty_input_end_release_tail() {
        for reason in [
            UtteranceEndReason::Silence,
            UtteranceEndReason::AdaptiveSilence,
            UtteranceEndReason::SpeakerChange,
            UtteranceEndReason::Flushed,
        ] {
            let mut buffer = SentenceAssembler::default();
            assert!(
                push(
                    &mut buffer,
                    chunk("unfinished", "a", 0.0, UtteranceEndReason::MaxActiveFrames)
                )
                .is_empty()
            );
            let end = push(&mut buffer, chunk("", "b", 8000.0, reason));
            assert_eq!(end[0].recognized.source_text, "unfinished");
        }
        let mut buffer = SentenceAssembler::default();
        push(
            &mut buffer,
            chunk("unfinished", "a", 0.0, UtteranceEndReason::MaxActiveFrames),
        );
        assert_eq!(
            buffer.take_pending().unwrap().recognized.source_text,
            "unfinished"
        );
        assert!(buffer.take_pending().is_none());
    }

    #[test]
    fn changing_speaker_or_language_never_joins_sentences() {
        for change_language in [false, true] {
            let mut buffer = SentenceAssembler::default();
            push(
                &mut buffer,
                chunk(
                    "first speaker",
                    "a",
                    0.0,
                    UtteranceEndReason::MaxActiveFrames,
                ),
            );
            let mut next = chunk("Second.", "b", 8000.0, UtteranceEndReason::Silence);
            if change_language {
                next.recognized.source_language = "de".into();
            } else {
                next.job.speaker_id = Some("speaker-2".into());
            }
            let ready = push(&mut buffer, next);
            assert_eq!(ready.len(), 2);
            assert_eq!(ready[0].recognized.source_text, "first speaker");
            assert_eq!(ready[1].recognized.source_text, "Second.");
            assert_eq!(ready[0].job.turn_id, "a");
            assert_eq!(ready[1].job.turn_id, "b");
        }
    }

    #[test]
    fn generation_change_discards_cancelled_tail() {
        let mut buffer = SentenceAssembler::default();
        push(
            &mut buffer,
            chunk("cancelled", "a", 0.0, UtteranceEndReason::MaxActiveFrames),
        );
        let mut generation = buffer.pending().unwrap().job.generation;
        generation.audio_epoch.advance();
        buffer.reset_unless_generation(generation);
        assert!(buffer.take_pending().is_none());
    }

    #[test]
    fn duration_limit_holds_right_edge_punctuation_until_lookahead() {
        let mut buffer = SentenceAssembler::default();
        let ready = push(
            &mut buffer,
            chunk(
                "First. Last.",
                "a",
                0.0,
                UtteranceEndReason::MaxActiveFrames,
            ),
        );
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].recognized.source_text, "First.");
        assert_eq!(buffer.pending().unwrap().recognized.source_text, "Last.");
        let next = push(
            &mut buffer,
            chunk(
                "Another sentence",
                "b",
                8000.0,
                UtteranceEndReason::MaxActiveFrames,
            ),
        );
        assert_eq!(next[0].recognized.source_text, "Last.");
        assert_eq!(
            buffer.pending().unwrap().recognized.source_text,
            "Another sentence"
        );
    }

    #[test]
    fn overlap_is_removed_once_and_original_context_survives_assembly() {
        let mut buffer = SentenceAssembler::default();
        let first = chunk(
            "The contract will",
            "a",
            0.0,
            UtteranceEndReason::MaxActiveFrames,
        );
        let previous_text = first.recognized.source_text.clone();
        push(&mut buffer, first);
        let mut next = chunk(
            "will start tomorrow.",
            "b",
            8000.0,
            UtteranceEndReason::Silence,
        );
        assert!(next.recognized.remove_overlap_with(&previous_text));
        let ready = push(&mut buffer, next);
        assert_eq!(
            ready[0].recognized.source_text,
            "The contract will start tomorrow."
        );

        let unchanged = push(
            &mut buffer,
            chunk(
                "Normal short utterance.",
                "c",
                16000.0,
                UtteranceEndReason::Silence,
            ),
        );
        assert_eq!(unchanged[0].asr_context.context_id, 1);
    }

    #[test]
    fn excessive_unfinished_text_is_visible_error_not_synthetic_sentence() {
        let mut buffer = SentenceAssembler::default();
        let text = "x".repeat(MAX_PENDING_CHARACTERS + 1);
        let error = match buffer.push(chunk(&text, "a", 0.0, UtteranceEndReason::MaxActiveFrames)) {
            Err(error) => error,
            Ok(_) => panic!("missing buffer limit"),
        };
        assert_eq!(error.visible.recognized.source_text, text);
        assert!(error.ready.is_empty());
        assert!(buffer.pending().is_none());
    }

    #[test]
    fn worker_end_fences_wait_for_pending_source_without_another_asr_request() {
        use crate::{InferenceJob, InferenceWork, flush_sentence_before_boundary};
        use std::collections::VecDeque;
        use xrtranslate_protocol::DrainReason;

        for stream_end in [false, true] {
            let mut buffer = SentenceAssembler::default();
            let pending = chunk(
                "unpunctuated ending",
                "a",
                0.0,
                UtteranceEndReason::MaxActiveFrames,
            );
            let generation = pending.job.generation;
            push(&mut buffer, pending);
            let boundary = if stream_end {
                InferenceJob::StreamEnded {
                    generation,
                    turn_id: "stream".into(),
                }
            } else {
                InferenceJob::Drain {
                    generation,
                    reason: DrainReason::InputEnded,
                }
            };
            let mut queue = VecDeque::new();
            let first = flush_sentence_before_boundary(
                InferenceWork::Queued(boundary),
                &mut buffer,
                &mut queue,
                generation,
            );
            let InferenceWork::Prepared(first) = first else {
                panic!("tail must precede fence")
            };
            assert_eq!(first.recognized.source_text, "unpunctuated ending");
            assert_eq!(first.job.utterance.end_reason, UtteranceEndReason::Flushed);
            assert!(buffer.pending().is_none());
            assert_eq!(queue.len(), 1);
            let fence = flush_sentence_before_boundary(
                queue.pop_front().unwrap(),
                &mut buffer,
                &mut queue,
                generation,
            );
            assert!(matches!(fence, InferenceWork::Queued(_)));
            assert!(queue.is_empty());
        }
    }

    #[test]
    fn terminal_error_fence_follows_retained_source_before_consumers_retire_session() {
        use crate::{InferenceWork, flush_sentence_before_boundary};
        use std::collections::VecDeque;

        let mut buffer = SentenceAssembler::default();
        let pending = chunk(
            "recognized before failure",
            "a",
            0.0,
            UtteranceEndReason::MaxActiveFrames,
        );
        let generation = pending.job.generation;
        push(&mut buffer, pending);
        let mut queue = VecDeque::new();
        let next = flush_sentence_before_boundary(
            InferenceWork::ErrorFence {
                generation,
                message: "ASR unavailable".into(),
                configuration_required: true,
            },
            &mut buffer,
            &mut queue,
            generation,
        );
        let InferenceWork::Prepared(source) = next else {
            panic!("terminal error overtook source")
        };
        assert_eq!(source.recognized.source_text, "recognized before failure");
        assert_eq!(source.job.turn_id, "a");
        assert_eq!(source.job.utterance.end_reason, UtteranceEndReason::Flushed);
        assert!(buffer.pending().is_none());
        let error = flush_sentence_before_boundary(
            queue.pop_front().unwrap(),
            &mut buffer,
            &mut queue,
            generation,
        );
        let InferenceWork::ErrorFence {
            generation: actual,
            message,
            configuration_required,
        } = error
        else {
            panic!("missing terminal error fence")
        };
        assert_eq!(actual, generation);
        assert_eq!(message, "ASR unavailable");
        assert!(configuration_required);
        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn original_audio_reference_is_internal_and_assembled_source_cannot_duplicate_it() {
        use crate::{InferenceEvent, SessionAdapter, handle_inference_event};
        let original = chunk(
            "First. unfinished",
            "a",
            0.0,
            UtteranceEndReason::MaxActiveFrames,
        );
        let reference = InferenceEvent::RecognitionReference {
            generation: original.job.generation,
            source_text: original.recognized.source_text.clone(),
            samples: vec![1, 2, 3],
        };
        assert_eq!(
            reference.clone_reference(),
            Some(("First. unfinished", &[1, 2, 3][..]))
        );
        let (writer, mut messages) = tokio::sync::mpsc::channel(4);
        let mut session = SessionAdapter::new("en", "zh").unwrap();
        assert!(
            !handle_inference_event(
                &writer,
                &mut session,
                original.job.generation,
                reference,
                None,
                None,
                "",
                false,
                500
            )
            .await
            .unwrap()
        );
        assert!(messages.try_recv().is_err());

        let mut buffer = SentenceAssembler::default();
        let mut ready = push(&mut buffer, original);
        let source = InferenceEvent::Recognized {
            generation: ready[0].job.generation,
            recognized: ready.remove(0).recognized,
            segments: Vec::new(),
            reference_samples: None,
        };
        assert!(source.clone_reference().is_none());
        assert_eq!(
            buffer.pending().unwrap().recognized.source_text,
            "unfinished"
        );
    }
}
