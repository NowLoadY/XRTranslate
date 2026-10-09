use std::collections::VecDeque;

use xrtranslate_engine::{
    EngineConfig, Language, LanguageRoute, OutboundPayload, ProtocolEvent, RouteEpoch,
    SessionEngine, TtsEpoch,
};
use xrtranslate_prompt::PromptExecutionTrace;
use xrtranslate_protocol::{
    AsrResult, AsrResultKind, CorpusTermMatch, LatencyMetrics, SegmentBoundary, SegmentTiming,
    ServerEvent, SourceSegmentReady, TranslationReady, TtsFinished,
};

pub(crate) enum WireOutput {
    Event(ServerEvent),
    Pcm(Vec<u8>),
}

/// Immutable identity and timeline carried by one source/translation pair.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SegmentContext {
    pub(crate) turn_id: String,
    pub(crate) segment_index: u32,
    pub(crate) segment_count: u32,
    pub(crate) speaker_id: String,
    pub(crate) source_start_ms: f64,
    pub(crate) source_end_ms: f64,
    pub(crate) timing: SegmentTiming,
    pub(crate) boundary: SegmentBoundary,
    pub(crate) revisable: bool,
    pub(crate) overlap_ratio: f32,
    pub(crate) authoritative_snapshot: bool,
    pub(crate) revision: u64,
    pub(crate) activation_matches: Vec<CorpusTermMatch>,
    pub(crate) context_matches: Vec<CorpusTermMatch>,
}

/// Translates the no-I/O session engine into the stable WebSocket contract.
///
/// Inference tasks may run concurrently, but only this adapter is allowed to
/// mutate the engine and drain its FIFO output.  This is the boundary which
/// prevents stale route/TTS work from reaching a connected client.
pub(crate) struct SessionAdapter {
    engine: SessionEngine,
    turn_id: String,
    recognized_turn_ids: VecDeque<String>,
    translation_metadata: VecDeque<TranslationMetadata>,
    translation_options: xrtranslate_protocol::TranslationOptions,
}

#[derive(Clone)]
struct TranslationMetadata {
    additional_translations: Vec<xrtranslate_protocol::AdditionalTranslation>,
    asr_only: bool,
    metrics: LatencyMetrics,
    context: SegmentContext,
    term_matches: Vec<CorpusTermMatch>,
    prompt_trace: Option<PromptExecutionTrace>,
}

impl SessionAdapter {
    pub(crate) fn new(source_lang: &str, target_lang: &str) -> Result<Self, String> {
        Ok(Self {
            engine: SessionEngine::new(route(source_lang, target_lang)?, EngineConfig::default()),
            turn_id: "native-1".into(),
            recognized_turn_ids: VecDeque::new(),
            translation_metadata: VecDeque::new(),
            translation_options: Default::default(),
        })
    }

    pub(crate) fn source_lang(&self) -> &str {
        self.engine.route().source.as_str()
    }

    pub(crate) fn target_lang(&self) -> &str {
        self.engine.route().target.as_str()
    }

    /// Captures the route identity that must travel with queued inference.
    pub(crate) fn route_epoch(&self) -> RouteEpoch {
        self.engine.route_epoch()
    }

    /// Captures the turn associated with queued work before later client
    /// controls can begin another turn.
    pub(crate) fn turn_id(&self) -> String {
        self.turn_id.clone()
    }

    pub(crate) fn set_route(&mut self, source_lang: &str, target_lang: &str) -> Result<(), String> {
        self.engine.set_route(route(source_lang, target_lang)?);
        self.recognized_turn_ids.clear();
        self.translation_metadata.clear();
        Ok(())
    }

    pub(crate) fn translation_options(&self) -> &xrtranslate_protocol::TranslationOptions {
        &self.translation_options
    }
    pub(crate) fn set_translation_options(
        &mut self,
        options: xrtranslate_protocol::TranslationOptions,
    ) {
        self.translation_options = options;
    }

    pub(crate) fn set_tts_enabled(&mut self, enabled: bool) {
        self.engine.set_tts_enabled(enabled);
    }

    pub(crate) const fn tts_enabled(&self) -> bool {
        self.engine.tts_enabled()
    }

    pub(crate) const fn tts_epoch(&self) -> TtsEpoch {
        self.engine.tts_epoch()
    }

    pub(crate) fn submit_tts_audio(
        &mut self,
        route_epoch: RouteEpoch,
        tts_epoch: TtsEpoch,
        pcm: Vec<u8>,
    ) -> Result<bool, String> {
        if route_epoch != self.engine.route_epoch() || tts_epoch != self.engine.tts_epoch() {
            return Ok(false);
        }
        self.engine
            .submit(ProtocolEvent::TtsAudio {
                route_epoch,
                tts_epoch,
                pcm,
            })
            .map(|_| true)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn set_turn_id(&mut self, turn_id: String) {
        if !turn_id.trim().is_empty() {
            self.turn_id = turn_id;
        }
    }

    pub(crate) fn submit_recognized_for_route_and_turn(
        &mut self,
        route_epoch: RouteEpoch,
        text: String,
        is_final: bool,
        turn_id: String,
    ) -> Result<bool, String> {
        if route_epoch != self.engine.route_epoch() {
            return Ok(false);
        }
        self.engine
            .submit(ProtocolEvent::RecognizedText {
                route_epoch,
                text,
                is_final,
            })
            .map(|_| {
                self.recognized_turn_ids.push_back(turn_id);
                true
            })
            .map_err(|error| error.to_string())
    }

    /// Same as [`Self::submit_translation_segment_for_route`], retaining the
    /// turn captured when the VAD utterance entered the worker queue.
    pub(crate) fn submit_translation_segment_for_route_and_turn(
        &mut self,
        route_epoch: RouteEpoch,
        source_text: String,
        translated_text: String,
        term_matches: Vec<CorpusTermMatch>,
        additional_translations: Vec<xrtranslate_protocol::AdditionalTranslation>,
        asr_only: bool,
        prompt_trace: Option<PromptExecutionTrace>,
        metrics: LatencyMetrics,
        context: SegmentContext,
    ) -> Result<bool, String> {
        if route_epoch != self.engine.route_epoch() {
            return Ok(false);
        }
        self.engine
            .submit(ProtocolEvent::TranslatedText {
                route_epoch,
                source_text,
                translated_text,
                is_final: true,
            })
            .map_err(|error| error.to_string())?;
        self.translation_metadata.push_back(TranslationMetadata {
            metrics,
            context,
            term_matches,
            additional_translations,
            asr_only,
            prompt_trace,
        });
        Ok(true)
    }

    /// Builds a source event using the turn captured for its queued utterance.
    pub(crate) fn source_segment_ready_for_turn(
        &self,
        source_text: String,
        context: SegmentContext,
        prompt_trace: Option<PromptExecutionTrace>,
    ) -> ServerEvent {
        ServerEvent::SourceSegmentReady(SourceSegmentReady {
            source_text,
            prompt_trace,
            activation_matches: context.activation_matches,
            context_matches: context.context_matches,
            turn_id: context.turn_id,
            segment_index: context.segment_index,
            segment_count: context.segment_count,
            speaker_id: context.speaker_id,
            source_start_ms: context.source_start_ms,
            source_end_ms: context.source_end_ms,
            timing: context.timing,
            boundary: context.boundary,
            revisable: context.revisable,
            overlap_ratio: context.overlap_ratio,
            authoritative_snapshot: context.authoritative_snapshot,
            revision: context.revision,
        })
    }

    /// Drains strictly ordered event/audio output for the only WebSocket writer.
    pub(crate) fn drain_wire_output(&mut self) -> Vec<WireOutput> {
        let mut output = Vec::new();
        for event in self.engine.drain_outbound() {
            match event.payload {
                OutboundPayload::RecognizedText { text, is_final } => {
                    let turn_id = self
                        .recognized_turn_ids
                        .pop_front()
                        .unwrap_or_else(|| self.turn_id.clone());
                    output.push(WireOutput::Event(ServerEvent::AsrResult(AsrResult {
                        kind: if is_final {
                            AsrResultKind::Final
                        } else {
                            AsrResultKind::Partial
                        },
                        text,
                        delta: String::new(),
                        turn_id,
                        ts: None,
                    })));
                }
                OutboundPayload::TranslatedText {
                    source_text,
                    translated_text,
                    ..
                } => {
                    let metadata =
                        self.translation_metadata
                            .pop_front()
                            .unwrap_or(TranslationMetadata {
                                metrics: LatencyMetrics {
                                    queue_ms: 0,
                                    asr_ms: 0,
                                    mt_ms: 0,
                                    tts_ms: 0,
                                    total_ms: 0,
                                },
                                context: SegmentContext {
                                    segment_index: 1,
                                    segment_count: 1,
                                    turn_id: self.turn_id.clone(),
                                    speaker_id: String::new(),
                                    source_start_ms: 0.0,
                                    source_end_ms: 0.0,
                                    timing: SegmentTiming::Unknown,
                                    boundary: SegmentBoundary::Unknown,
                                    revisable: false,
                                    overlap_ratio: 0.0,
                                    authoritative_snapshot: false,
                                    revision: 0,
                                    activation_matches: Vec::new(),
                                    context_matches: Vec::new(),
                                },
                                term_matches: Vec::new(),
                                additional_translations: Vec::new(),
                                asr_only: false,
                                prompt_trace: None,
                            });
                    output.push(WireOutput::Event(ServerEvent::TranslationReady(
                        TranslationReady {
                            source_text,
                            translated_text,
                            term_matches: metadata.term_matches,
                            additional_translations: metadata.additional_translations,
                            asr_only: metadata.asr_only,
                            prompt_trace: metadata.prompt_trace,
                            turn_id: metadata.context.turn_id,
                            segment_index: metadata.context.segment_index,
                            segment_count: metadata.context.segment_count,
                            speaker_id: metadata.context.speaker_id,
                            source_start_ms: metadata.context.source_start_ms,
                            source_end_ms: metadata.context.source_end_ms,
                            timing: metadata.context.timing,
                            boundary: metadata.context.boundary,
                            revisable: metadata.context.revisable,
                            overlap_ratio: metadata.context.overlap_ratio,
                            authoritative_snapshot: metadata.context.authoritative_snapshot,
                            revision: metadata.context.revision,
                            clone_audio_path: String::new(),
                            tts_audio_path: String::new(),
                            metrics: metadata.metrics,
                        },
                    )));
                }
                OutboundPayload::TtsAudio { pcm } => {
                    output.push(WireOutput::Pcm(pcm));
                    output.push(WireOutput::Event(ServerEvent::TtsFinished(TtsFinished {
                        text: String::new(),
                    })));
                }
            }
        }
        output
    }
}

fn route(source_lang: &str, target_lang: &str) -> Result<LanguageRoute, String> {
    let source = Language::new(source_lang.to_owned()).map_err(|error| error.to_string())?;
    let target = Language::new(target_lang.to_owned()).map_err(|error| error.to_string())?;
    Ok(LanguageRoute::new(source, target))
}
