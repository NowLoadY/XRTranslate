//! Independent output routes share recognition, never bilingual model history.

use std::future::Future;

use xr_corpus_client::{CorpusClient, CorpusSessionClient};
use xr_corpus_protocol::{ContextBudgets, PrepareAsrRequest, PrepareTranslationRequest};
use xrtranslate_engine::TranslationSegmentPair;
use xrtranslate_engine::language::SupportedLanguage;
use xrtranslate_prompt::{PromptExecutionTrace, PromptNodeGraph, TranslationPromptContext};
use xrtranslate_protocol::{AdditionalTranslation, InferenceWorkload};

use crate::{
    PipelineGeneration, align_translation_contexts,
    conversation_context::{ContextBinding, completed_turn, context_evidence},
    pipeline::{InferenceFailure, NativeInference, RecognizedOutput, TranslationOutput},
    prompt_context::prompt_context_for_segment,
    scheduler::InferenceScheduler,
    terminology::{rewrite_recognition_terms, rewrite_translation_terms},
};

#[derive(Default)]
pub(crate) struct AdditionalOutputScope {
    active: Option<(PipelineGeneration, String, CorpusSessionClient)>,
}

impl Drop for AdditionalOutputScope {
    fn drop(&mut self) {
        if let Some((_, _, session)) = self.active.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = session.close().await;
            });
        }
    }
}

pub(crate) struct AdditionalTurn {
    binding: ContextBinding,
    memory_turn_id: String,
    revision: u64,
    target: String,
    segments: Vec<AdditionalSegment>,
}

#[derive(Clone)]
pub(crate) struct AdditionalSegment {
    pub(crate) target: String,
    context: xr_corpus_protocol::SegmentContext,
    prompt: TranslationPromptContext,
    segment: TranslationSegmentPair,
}

fn same_output_language(left: &str, right: &str) -> bool {
    SupportedLanguage::parse(left)
        .zip(SupportedLanguage::parse(right))
        .is_some_and(|(left, right)| left == right)
}

impl AdditionalOutputScope {
    pub(crate) async fn close(&mut self) {
        if let Some((_, _, session)) = self.active.take() {
            let _ = session.close().await;
        }
    }

    pub(crate) async fn prepare(
        &mut self,
        client: &CorpusClient,
        generation: PipelineGeneration,
        target: Option<&str>,
        recognized: &RecognizedOutput,
        budgets: ContextBudgets,
        turn_id: &str,
        speaker_id: &str,
        binding: &ContextBinding,
        memory_turn_id: &str,
        revision: u64,
    ) -> Result<Option<AdditionalTurn>, String> {
        let Some(target) = target else {
            self.close().await;
            return Ok(None);
        };
        let needs_session =
            self.active
                .as_ref()
                .is_none_or(|(active_generation, active_target, _)| {
                    *active_generation != generation || active_target != target
                });
        if same_output_language(target, &recognized.target_language) {
            // Suspend this output without preparing or recording a turn. Its
            // own history resumes when the primary target changes again.
            if needs_session {
                self.close().await;
            }
            return Ok(None);
        }
        if needs_session {
            let session = client
                .create_session()
                .await
                .map_err(|error| error.to_string())?;
            if let Some((_, _, old)) = self.active.replace((generation, target.into(), session)) {
                tokio::spawn(async move {
                    let _ = old.close().await;
                });
            }
        }
        let task_reference = binding.snapshot(
            memory_turn_id,
            speaker_id,
            &recognized.source_language,
            target,
        );
        let session = &mut self.active.as_mut().expect("prepared additional scope").2;
        let asr = session
            .prepare_asr(&PrepareAsrRequest {
                source_language: recognized.source_language.clone(),
                target_language: target.into(),
                budgets,
            })
            .await
            .map_err(|error| error.to_string())?;
        let context = session
            .prepare_translation(&PrepareTranslationRequest {
                asr_context_id: asr.context_id,
                turn_id: Some(turn_id.into()),
                context_evidence: context_evidence(&task_reference),
                source_language: recognized.source_language.clone(),
                target_language: target.into(),
                recognized_text: recognized.source_text.clone(),
                segments: recognized
                    .segments
                    .iter()
                    .map(|segment| segment.translation_text.clone())
                    .collect(),
                budgets,
            })
            .await
            .map_err(|error| error.to_string())?;
        let segments = align_translation_contexts(&recognized.segments, &context.segments)
            .into_iter()
            .zip(recognized.segments.iter().cloned())
            .enumerate()
            .map(|(index, (context, segment))| AdditionalSegment {
                target: target.to_owned(),
                prompt: prompt_context_for_segment(
                    &recognized.source_language,
                    target,
                    &context,
                    &task_reference,
                    &recognized.segments,
                    index,
                    budgets.translation_tokens,
                ),
                context,
                segment,
            })
            .collect();
        Ok(Some(AdditionalTurn {
            binding: binding.clone(),
            memory_turn_id: memory_turn_id.to_owned(),
            revision,
            target: target.to_owned(),
            segments,
        }))
    }
}

impl AdditionalTurn {
    pub(crate) fn segment(&self, index: usize) -> Option<AdditionalSegment> {
        self.segments.get(index).cloned()
    }

    pub(crate) fn record(&self, speaker_id: String, source: &str, pairs: &[(String, String)]) {
        if let Some(turn) = completed_turn(
            self.memory_turn_id.clone(),
            speaker_id,
            source,
            &self.target,
            pairs,
        ) {
            self.binding.record(turn, self.revision);
        }
    }
}

/// Each provider call gets its own scheduler permit. Join only after both have
/// started, so a one-slot provider remains bounded without nested-permit deadlock.
pub(crate) async fn translate<F, Fut, T>(
    inference: &NativeInference,
    scheduler: &InferenceScheduler,
    workload: InferenceWorkload,
    segment: &TranslationSegmentPair,
    source: &str,
    target: &str,
    graph: PromptNodeGraph,
    context: TranslationPromptContext,
    additional: Option<AdditionalSegment>,
    asr_only: bool,
    on_update: F,
    on_trace: T,
) -> Result<TranslationOutput, InferenceFailure>
where
    F: FnMut(String) -> Fut + Send,
    Fut: Future<Output = ()> + Send,
    T: Fn(&PromptExecutionTrace) + Send + Sync,
{
    if asr_only {
        return Ok(NativeInference::recognition_output(segment, source));
    }
    let primary = async {
        let _permit = scheduler.acquire_translation(workload).await;
        inference
            .translate_segment_streaming(
                segment,
                source,
                target,
                graph.clone(),
                context.clone(),
                on_update,
                &on_trace,
            )
            .await
    };
    let extra = async {
        let Some(additional) =
            additional.filter(|extra| !same_output_language(target, &extra.target))
        else {
            return Ok(None);
        };
        let source_for_terms = rewrite_recognition_terms(
            &additional.segment.translation_text,
            &additional.context.source_corrections,
        )
        .corrected_text;
        let independent_segment = TranslationSegmentPair {
            source_text: source_for_terms.clone(),
            translation_text: source_for_terms.clone(),
        };
        let mut output = if source == additional.target {
            NativeInference::recognition_output(&independent_segment, source)
        } else {
            let mut prompt = additional.prompt;
            prompt.mode = context.mode;
            let _permit = scheduler.acquire_translation(workload).await;
            inference
                .translate_segment_streaming(
                    &independent_segment,
                    source,
                    &additional.target,
                    graph.clone(),
                    prompt,
                    |_| std::future::ready(()),
                    &on_trace,
                )
                .await?
        };
        let rewrite = rewrite_translation_terms(
            &source_for_terms,
            &output.translated_text,
            &additional.target,
            &additional.context.prompt_terms,
        );
        output.translated_text = rewrite.translated_text;
        Ok::<_, InferenceFailure>(Some((
            source_for_terms,
            AdditionalTranslation {
                target_lang: additional.target,
                translated_text: output.translated_text,
                term_matches: rewrite.term_matches,
            },
        )))
    };
    let (primary, extra) = tokio::join!(primary, extra);
    let mut output = primary?;
    if let Some((source, extra)) = extra? {
        output.additional_source_text = Some(source);
        output.additional_translations.push(extra);
    }
    Ok(output)
}
