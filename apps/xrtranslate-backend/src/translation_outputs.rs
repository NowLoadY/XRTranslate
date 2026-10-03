//! Independent output routes share recognition, never bilingual model history.

use std::future::Future;

use xr_corpus_client::{CorpusClient, CorpusSessionClient};
use xr_corpus_protocol::{ContextBudgets, PrepareAsrRequest, PrepareTranslationRequest};
use xrtranslate_engine::TranslationSegmentPair;
use xrtranslate_engine::language::SupportedLanguage;
use xrtranslate_prompt::{PromptNodeGraph, TranslationPromptContext};
use xrtranslate_protocol::{AdditionalTranslation, InferenceWorkload};

use crate::{
    PipelineGeneration, align_translation_contexts,
    conversation_context::LogicalTurnRecord,
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
    session: CorpusSessionClient,
    context_id: u64,
    target: String,
    contexts: Vec<xr_corpus_protocol::SegmentContext>,
    segments: Vec<TranslationSegmentPair>,
}

#[derive(Clone)]
pub(crate) struct AdditionalSegment {
    pub(crate) target: String,
    context: xr_corpus_protocol::SegmentContext,
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
                speaker_id: speaker_id.into(),
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
        Ok(Some(AdditionalTurn {
            session: session.clone(),
            context_id: context.context_id,
            target: target.into(),
            contexts: align_translation_contexts(&recognized.segments, &context.segments),
            segments: recognized.segments.clone(),
        }))
    }
}

impl AdditionalTurn {
    pub(crate) fn segment(&self, index: usize) -> Option<AdditionalSegment> {
        self.contexts
            .get(index)
            .cloned()
            .zip(self.segments.get(index).cloned())
            .map(|(context, segment)| AdditionalSegment {
                target: self.target.clone(),
                context,
                segment,
            })
    }

    pub(crate) async fn record(
        &self,
        turn_id: String,
        speaker_id: String,
        source: &str,
        pairs: &[(String, String)],
    ) {
        if let Some(request) = (LogicalTurnRecord {
            context_id: self.context_id.clone(),
            turn_id,
            speaker_id,
            source_language: source,
            target_language: &self.target,
            completed_pairs: pairs,
        })
        .into_request()
        {
            if let Err(error) = self.session.record_translation(&request).await {
                tracing::warn!(%error, "could not record independent additional translation context");
            }
        }
    }
}

/// Each provider call gets its own scheduler permit. Join only after both have
/// started, so a one-slot provider remains bounded without nested-permit deadlock.
pub(crate) async fn translate<F, Fut>(
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
) -> Result<TranslationOutput, InferenceFailure>
where
    F: FnMut(String) -> Fut + Send,
    Fut: Future<Output = ()> + Send,
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
            let mut prompt =
                prompt_context_for_segment(source, &additional.target, &additional.context);
            prompt.mode = context.mode;
            let _permit = scheduler.acquire_translation(workload).await;
            inference
                .translate_segment(
                    &independent_segment,
                    source,
                    &additional.target,
                    graph.clone(),
                    prompt,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use xr_corpus_protocol::{BilingualContextTurn, SegmentContext, TranslationContextData};

    fn context(marker: &str, target: &str) -> SegmentContext {
        SegmentContext {
            corrected_text: "Hello friend.".into(),
            prompt_terms: Vec::new(),
            context_data: TranslationContextData {
                recent_turns: vec![BilingualContextTurn {
                    turn_id: Some(marker.into()),
                    speaker_id: String::new(),
                    source_language: "en".into(),
                    target_language: target.into(),
                    source_text: marker.into(),
                    translated_text: marker.into(),
                }],
                ..Default::default()
            },
            source_corrections: Vec::new(),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
        }
    }

    async fn fixture() -> (
        NativeInference,
        Arc<Mutex<Vec<serde_json::Value>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let observed = maximum.clone();
        let router = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(request): axum::Json<serde_json::Value>| {
            let captured = captured.clone(); let active = active.clone(); let observed = observed.clone();
            async move {
                captured.lock().unwrap().push(request);
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                observed.fetch_max(count, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                ([ (axum::http::header::CONTENT_TYPE, "text/event-stream") ], "data: {\"choices\":[{\"delta\":{\"content\":\"Bonjour mon ami.\"}}]}\n\ndata: [DONE]\n\n")
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config: serde_json::Value =
            serde_json::from_str(include_str!("../../../config.json")).unwrap();
        config["translation"]["providers"]["openai-custom"] =
            config["translation"]["providers"]["openai"].clone();
        config["translation"]["provider"] = "openai-custom".into();
        config["translation"]["providers"]["openai-custom"]["api_key"] = "test-key".into();
        config["translation"]["providers"]["openai-custom"]["url"] = endpoint.into();
        let config = xrtranslate_config::AppConfig::from_value(config).unwrap();
        let plan =
            crate::model_runtime::NativeProviderPlan::resolve(&config, &std::env::temp_dir())
                .unwrap();
        (
            NativeInference::for_text(&plan).unwrap(),
            requests,
            maximum,
            server,
        )
    }

    #[tokio::test]
    async fn outputs_start_concurrently_and_never_share_context_or_translate_the_primary_result() {
        let (inference, requests, maximum, server) = fixture().await;
        let segment = TranslationSegmentPair {
            source_text: "Hello friend.".into(),
            translation_text: "Hello friend.".into(),
        };
        let primary_context =
            prompt_context_for_segment("en", "fr", &context("PRIMARY_HISTORY", "fr"));
        let output = translate(
            &inference,
            &InferenceScheduler::new(1, 2),
            InferenceWorkload::Realtime,
            &segment,
            "en",
            "fr",
            PromptNodeGraph::builtin_default(),
            primary_context,
            Some(AdditionalSegment {
                target: "de".into(),
                context: context("EXTRA_HISTORY", "de"),
                segment: TranslationSegmentPair {
                    source_text: "Hello friend.".into(),
                    translation_text: "Hello friend.".into(),
                },
            }),
            false,
            |_| std::future::ready(()),
        )
        .await
        .unwrap();
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        assert_eq!(output.additional_translations.len(), 1);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let primary = requests
            .iter()
            .map(ToString::to_string)
            .find(|text| text.contains("PRIMARY_HISTORY"))
            .unwrap();
        let extra = requests
            .iter()
            .map(ToString::to_string)
            .find(|text| text.contains("EXTRA_HISTORY"))
            .unwrap();
        assert!(!primary.contains("EXTRA_HISTORY"));
        assert!(!extra.contains("PRIMARY_HISTORY"));
        assert!(primary.contains("Hello friend."));
        assert!(extra.contains("Hello friend."));
        assert!(!extra.contains("Bonjour mon ami."));
        server.abort();
    }

    #[tokio::test]
    async fn duplicate_target_uses_only_primary_provider_context_and_result() {
        let (inference, requests, maximum, server) = fixture().await;
        for (target, extra_target) in [("zh", "zh"), ("zh", "Chinese"), ("zh-TW", "zh_hant_HK")] {
            let segment = TranslationSegmentPair {
                source_text: "Hello friend.".into(),
                translation_text: "Hello friend.".into(),
            };
            let output = translate(
                &inference,
                &InferenceScheduler::new(1, 2),
                InferenceWorkload::Realtime,
                &segment,
                "en",
                target,
                PromptNodeGraph::builtin_default(),
                prompt_context_for_segment("en", target, &context("PRIMARY_HISTORY", target)),
                Some(AdditionalSegment {
                    target: extra_target.into(),
                    context: context("EXTRA_HISTORY", extra_target),
                    segment: segment.clone(),
                }),
                false,
                |_| std::future::ready(()),
            )
            .await
            .unwrap();
            assert_eq!(output.target_language, target);
            assert!(!output.translated_text.is_empty());
            assert!(!output.asr_only);
            assert!(output.additional_translations.is_empty());
            assert!(output.additional_source_text.is_none());
            let mut requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let request = requests[0].to_string();
            assert!(request.contains("PRIMARY_HISTORY"));
            assert!(!request.contains("EXTRA_HISTORY"));
            requests.clear();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn simplified_and_traditional_chinese_remain_independent_outputs() {
        let (inference, requests, _, server) = fixture().await;
        let segment = TranslationSegmentPair {
            source_text: "Hello friend.".into(),
            translation_text: "Hello friend.".into(),
        };
        let output = translate(
            &inference,
            &InferenceScheduler::new(1, 2),
            InferenceWorkload::Realtime,
            &segment,
            "en",
            "zh",
            PromptNodeGraph::builtin_default(),
            TranslationPromptContext::default(),
            Some(AdditionalSegment {
                target: "zh-TW".into(),
                context: context("EXTRA_HISTORY", "zh-TW"),
                segment: segment.clone(),
            }),
            false,
            |_| std::future::ready(()),
        )
        .await
        .unwrap();
        assert_eq!(output.target_language, "zh");
        assert_eq!(output.additional_translations.len(), 1);
        assert_eq!(output.additional_translations[0].target_lang, "zh-TW");
        assert_eq!(requests.lock().unwrap().len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn one_translation_slot_serves_two_outputs_without_a_nested_permit_deadlock() {
        let (inference, requests, maximum, server) = fixture().await;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            translate(
                &inference,
                &InferenceScheduler::new(1, 1),
                InferenceWorkload::Realtime,
                &TranslationSegmentPair {
                    source_text: "Hello friend.".into(),
                    translation_text: "Hello friend.".into(),
                },
                "en",
                "fr",
                PromptNodeGraph::builtin_default(),
                TranslationPromptContext::default(),
                Some(AdditionalSegment {
                    target: "de".into(),
                    context: context("EXTRA_HISTORY", "de"),
                    segment: TranslationSegmentPair {
                        source_text: "Hello friend.".into(),
                        translation_text: "Hello friend.".into(),
                    },
                }),
                false,
                |_| std::future::ready(()),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output.additional_translations.len(), 1);
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn asr_only_uses_no_provider_or_translation_slot_and_preserves_final_source() {
        let (inference, requests, _, server) = fixture().await;
        let scheduler = InferenceScheduler::new(1, 1);
        let _occupied = scheduler
            .acquire_translation(InferenceWorkload::Realtime)
            .await;
        let output = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            translate(
                &inference,
                &scheduler,
                InferenceWorkload::Realtime,
                &TranslationSegmentPair {
                    source_text: "Corrected recognition.".into(),
                    translation_text: "trimmed".into(),
                },
                "en",
                "fr",
                PromptNodeGraph::builtin_default(),
                TranslationPromptContext::default(),
                None,
                true,
                |_| std::future::ready(()),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(output.asr_only);
        assert_eq!(output.translated_text, "Corrected recognition.");
        assert!(requests.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn same_language_extra_keeps_its_own_recognition_correction_in_output_and_history() {
        let (inference, requests, _, server) = fixture().await;
        let mut extra_context = context("EXTRA_HISTORY", "en");
        extra_context
            .source_corrections
            .push(xr_corpus_protocol::CorpusRecognitionCorrection {
                start_byte: 0,
                end_byte: 5,
                original_text: "Hullo".into(),
                corrected_text: "Hello".into(),
                sources: Vec::new(),
            });
        let output = translate(
            &inference,
            &InferenceScheduler::new(1, 1),
            InferenceWorkload::Realtime,
            &TranslationSegmentPair {
                source_text: "PRIMARY_CORRECTION.".into(),
                translation_text: "PRIMARY_CORRECTION.".into(),
            },
            "en",
            "fr",
            PromptNodeGraph::builtin_default(),
            TranslationPromptContext::default(),
            Some(AdditionalSegment {
                target: "en".into(),
                context: extra_context,
                segment: TranslationSegmentPair {
                    source_text: "Hullo friend.".into(),
                    translation_text: "Hullo friend.".into(),
                },
            }),
            false,
            |_| std::future::ready(()),
        )
        .await
        .unwrap();
        assert_eq!(
            output.additional_translations[0].translated_text,
            "Hello friend."
        );
        assert_eq!(
            output.additional_source_text.as_deref(),
            Some("Hello friend.")
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn corpus_sessions_isolate_primary_history_and_reset_on_output_scope_changes() {
        use axum::{
            Json,
            extract::Path,
            routing::{delete, post},
        };
        use xr_corpus_protocol::{PrepareTranslationResponse, RecordTranslationRequest};
        let histories = Arc::new(Mutex::new(std::collections::HashMap::<
            String,
            BilingualContextTurn,
        >::new()));
        let translation_histories = histories.clone();
        let record_histories = histories.clone();
        let created = Arc::new(AtomicUsize::new(0));
        let ids = created.clone();
        let prepared = Arc::new(AtomicUsize::new(0));
        let asr_prepared = prepared.clone();
        let translation_prepared = prepared.clone();
        let closed = Arc::new(Mutex::new(Vec::new()));
        let close_ids = closed.clone();
        let router = axum::Router::new()
            .route(
                "/v2/sessions",
                post(move || {
                    let id = ids.fetch_add(1, Ordering::SeqCst);
                    async move { Json(serde_json::json!({"session_id": format!("session-{id}")})) }
                }),
            )
            .route(
                "/v2/sessions/{id}/asr",
                post(move || {
                    asr_prepared.fetch_add(1, Ordering::SeqCst);
                    async { Json(serde_json::json!({"context_id":1})) }
                }),
            )
            .route(
                "/v2/sessions/{id}/translation",
                post(
                    move |Path(id): Path<String>,
                          Json(request): Json<PrepareTranslationRequest>| {
                        translation_prepared.fetch_add(1, Ordering::SeqCst);
                        let history = translation_histories.lock().unwrap().get(&id).cloned();
                        async move {
                            Json(PrepareTranslationResponse {
                                context_id: 2,
                                corrected_text: request.recognized_text,
                                source_corrections: Vec::new(),
                                segments: request
                                    .segments
                                    .into_iter()
                                    .map(|text| {
                                        let mut context =
                                            context("unused", &request.target_language);
                                        context.corrected_text = text;
                                        context.context_data.recent_turns =
                                            history.clone().into_iter().collect();
                                        context
                                    })
                                    .collect(),
                            })
                        }
                    },
                ),
            )
            .route(
                "/v2/sessions/{id}/results",
                post(
                    move |Path(id): Path<String>, Json(request): Json<RecordTranslationRequest>| {
                        record_histories.lock().unwrap().insert(
                            id,
                            BilingualContextTurn {
                                turn_id: request.turn_id,
                                speaker_id: request.speaker_id,
                                source_language: request.source_language,
                                target_language: request.target_language,
                                source_text: request.source_text,
                                translated_text: request.translated_text,
                            },
                        );
                        async { Json(serde_json::json!({"term_matches":[]})) }
                    },
                ),
            )
            .route(
                "/v2/sessions/{id}",
                delete(move |Path(id): Path<String>| {
                    close_ids.lock().unwrap().push(id);
                    async { axum::http::StatusCode::NO_CONTENT }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            CorpusClient::new(format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let primary = client.create_session().await.unwrap();
        primary
            .record_translation(&RecordTranslationRequest {
                context_id: 2,
                turn_id: Some("turn-1".into()),
                speaker_id: String::new(),
                source_language: "en".into(),
                target_language: "fr".into(),
                source_text: "PRIMARY_SOURCE".into(),
                translated_text: "PRIMARY_HISTORY".into(),
            })
            .await
            .unwrap();
        let recognized = RecognizedOutput {
            source_text: "Hello friend.".into(),
            segments: vec![TranslationSegmentPair {
                source_text: "Hello friend.".into(),
                translation_text: "Hello friend.".into(),
            }],
            source_language: "en".into(),
            target_language: "fr".into(),
            asr_elapsed: std::time::Duration::ZERO,
            route_switched: None,
            prompt_trace: None,
        };
        let mut scope = AdditionalOutputScope::default();
        let mut generation = PipelineGeneration {
            route_epoch: xrtranslate_engine::RouteEpoch::INITIAL,
            audio_epoch: crate::AudioEpoch::INITIAL,
        };
        let budgets = ContextBudgets {
            asr_tokens: 128,
            translation_tokens: 256,
        };
        let mut overlapping = recognized.clone();
        overlapping.target_language = "ja".into();
        assert!(
            scope
                .prepare(
                    &client,
                    generation,
                    Some("ja"),
                    &overlapping,
                    budgets,
                    "turn-overlap-before-first-extra",
                    "",
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(scope.active.is_none());
        assert_eq!(created.load(Ordering::SeqCst), 1);
        assert_eq!(prepared.load(Ordering::SeqCst), 0);
        let extra = scope
            .prepare(
                &client,
                generation,
                Some("ja"),
                &recognized,
                budgets,
                "turn-1",
                "",
            )
            .await
            .unwrap()
            .unwrap();
        assert_ne!(extra.session.id(), primary.id());
        assert!(extra.contexts[0].context_data.recent_turns.is_empty());
        extra
            .record(
                "turn-1".into(),
                String::new(),
                "en",
                &[("EXTRA_SOURCE".into(), "EXTRA_HISTORY".into())],
            )
            .await;
        assert!(
            scope
                .prepare(
                    &client,
                    generation,
                    Some("ja"),
                    &overlapping,
                    budgets,
                    "turn-overlap",
                    "",
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(scope.active.as_ref().unwrap().2.id(), extra.session.id());
        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(prepared.load(Ordering::SeqCst), 2);
        let next = scope
            .prepare(
                &client,
                generation,
                Some("ja"),
                &recognized,
                budgets,
                "turn-2",
                "",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.session.id(), extra.session.id());
        assert_eq!(
            next.contexts[0].context_data.recent_turns[0].translated_text,
            "EXTRA_HISTORY"
        );
        assert_eq!(
            next.contexts[0].context_data.recent_turns[0].source_text,
            "EXTRA_SOURCE"
        );
        generation.audio_epoch.advance();
        assert!(
            scope
                .prepare(
                    &client,
                    generation,
                    Some("ja"),
                    &overlapping,
                    budgets,
                    "turn-overlap-new-generation",
                    "",
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(scope.active.is_none());
        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(prepared.load(Ordering::SeqCst), 4);
        let reset = scope
            .prepare(
                &client,
                generation,
                Some("ja"),
                &recognized,
                budgets,
                "turn-3",
                "",
            )
            .await
            .unwrap()
            .unwrap();
        assert_ne!(reset.session.id(), next.session.id());
        assert!(reset.contexts[0].context_data.recent_turns.is_empty());
        scope
            .prepare(
                &client,
                generation,
                None,
                &recognized,
                budgets,
                "turn-4",
                "",
            )
            .await
            .unwrap();
        assert!(scope.active.is_none());
        assert!(
            closed
                .lock()
                .unwrap()
                .contains(&reset.session.id().to_owned())
        );
        server.abort();
    }
}
