//! Axum entrypoint for the native XRTranslate backend.
//!
//! This bootstrap intentionally exposes the legacy health and WebSocket
//! contract before model execution is connected.  Keeping transport separate
//! lets the remaining engine work land without another client protocol change.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::time::{Instant, sleep};
use tokio::{
    net::TcpListener,
    sync::{mpsc, watch},
};
use tracing::{info, warn};
use xrtranslate_config::{AppConfig, RuntimeLayout};
use xrtranslate_engine::{
    RevisableTranscript, RouteEpoch, translation_segment_pairs_for_final_text_with_lang,
};
use xrtranslate_inference::{AsyncHttpClient, HttpRequest, ReqwestClient, pcm16_mono_16khz_to_wav};
use xrtranslate_prompt::{AsrPromptContext, PromptMode, PromptNodeGraph};
use xrtranslate_protocol::{
    ActionControl, ClientControl, DrainReason, ErrorEvent, EventControl, Feature,
    InferenceWorkload, LatencyMetrics, PcmFormat, PcmFrame, PipelineDrained, PromptGraphSet,
    RecognitionStreamEnded, RouteChanged, SegmentBoundary, SegmentTiming, ServerEvent,
    SessionReady, VadActivity, VoiceClonePhase, VoiceCloneState,
};
use xrtranslate_supervisor::{
    LlamaServerLauncher, LlamaServerProcess, LlamaServerProcessHandle, StdLlamaServerLauncher,
};
use xrtranslate_vad::{FRAME_SAMPLES, SAMPLE_RATE_HZ, Utterance, UtteranceEndReason};

use crate::{
    conversation_context::LogicalTurnRecord,
    language::AdaptiveLanguageRoute,
    pipeline::{
        InferenceFailure, NativeInference, NativePipeline, PipelineEvent, RecognizedOutput,
        TimedUtterance, TranslationOutput, validate_input_chunk_size, validate_input_sample_rate,
    },
    prompt_context::prompt_context_for_segment,
    session::{SegmentContext, SessionAdapter, WireOutput},
    terminology::{rewrite_recognition_terms, rewrite_translation_terms},
};

mod conversation_context;
mod language;
mod model_runtime;
mod pipeline;
mod prompt_context;
mod scheduler;
mod session;
mod terminology;
mod tts_session;
use model_runtime::{
    NativeProviderPlan, NativeTtsAdapter, OnnxRuntimeDiagnostic, initialize_managed_onnx_runtime,
    runtime_diagnostic,
};
use scheduler::InferenceScheduler;
use tts_session::{
    TtsSynthesisJob, VoiceCloneCapture, clone_voice_name, max_input_chars as tts_max_input_chars,
    restore_persisted_voice_clones, run_tts_worker, save_persisted_voice_clone,
    split_text as split_tts_text,
};
use xr_corpus_client::CorpusClient;
use xr_corpus_protocol::{
    ContextBudgets, PrepareAsrRequest, PrepareTranslationRequest,
    SegmentContext as CorpusSegmentContext,
};

/// Complete turns awaiting the per-session worker. Awaited sends retain media
/// throughput, so a deep queue only hides overload and increases cancellation
/// cost without improving model utilization.
const INFERENCE_QUEUE_CAPACITY: usize = 4;
/// Results awaiting the only WebSocket writer. This protects backend memory
/// when a client socket stops consuming messages.
const INFERENCE_RESULT_CAPACITY: usize = 32;
/// The socket writer owns this bounded queue. Keeping it separate from model
/// results makes the WebSocket write path explicit and preserves event order.
const OUTBOUND_MESSAGE_CAPACITY: usize = 64;
/// Per-turn fan-out is additionally bounded by the global scheduler, whose
/// capacity comes from the configured model runtime.
const TRANSLATION_CONCURRENCY_PER_SESSION: usize = 2;
/// TTS is intentionally independent from the ASR/translation event pump. A
/// small queue preserves speech order while applying backpressure instead of
/// allowing long translated sessions to accumulate unbounded audio work.
const TTS_QUEUE_CAPACITY: usize = 4;

#[derive(Clone, Copy)]
struct StreamWindowContext {
    start_ms: f64,
    end_ms: f64,
    revisable: bool,
    overlap_ratio: f32,
    boundary: SegmentBoundary,
    authoritative_snapshot: bool,
    revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AudioEpoch(u64);

impl AudioEpoch {
    const INITIAL: Self = Self(0);

    fn advance(&mut self) {
        self.0 = self.0.checked_add(1).expect("audio epoch overflow");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PipelineGeneration {
    route_epoch: RouteEpoch,
    audio_epoch: AudioEpoch,
}

struct UtteranceJob {
    utterance: Utterance,
    source_start_ms: f64,
    source_end_ms: f64,
    revisable: bool,
    generation: PipelineGeneration,
    turn_id: String,
    topic_turn_id: String,
    source_language: String,
    target_language: String,
    speaker_id: Option<String>,
    workload: InferenceWorkload,
    enqueued_at: Instant,
    revision: u64,
}

struct TextJob {
    text: String,
    generation: PipelineGeneration,
    turn_id: String,
    topic_turn_id: String,
    source_language: String,
    target_language: String,
    speaker_id: Option<String>,
    workload: InferenceWorkload,
    enqueued_at: Instant,
    revision: u64,
}

fn validate_prompt_graph_set(
    graphs: &PromptGraphSet,
) -> Result<(), xrtranslate_prompt::PromptGraphError> {
    graphs.graph.validate_for_activation()
}

enum InferenceJob {
    Utterance(UtteranceJob),
    Text(TextJob),
    StreamEnded {
        generation: PipelineGeneration,
        turn_id: String,
    },
    /// An ordered fence. The worker emits the matching event only after every
    /// inference job queued before it has completed.
    Drain {
        generation: PipelineGeneration,
        reason: DrainReason,
    },
}

enum InferenceEvent {
    WindowObserved {
        generation: PipelineGeneration,
        text_units: usize,
    },
    Recognized {
        generation: PipelineGeneration,
        recognized: RecognizedOutput,
        segments: Vec<SegmentContext>,
        reference_samples: Option<Vec<i16>>,
    },
    Translation {
        generation: PipelineGeneration,
        target_language: String,
        queue_elapsed: Duration,
        asr_elapsed: Duration,
        total_elapsed: Duration,
        context: SegmentContext,
        output: Result<TranslationOutput, InferenceFailure>,
    },
    StreamEnded {
        generation: PipelineGeneration,
        turn_id: String,
    },
    Drained {
        generation: PipelineGeneration,
        reason: DrainReason,
    },
    Error {
        generation: PipelineGeneration,
        message: String,
        configuration_required: bool,
    },
}

impl InferenceEvent {
    const fn generation(&self) -> PipelineGeneration {
        match self {
            Self::WindowObserved { generation, .. }
            | Self::Recognized { generation, .. }
            | Self::Translation { generation, .. }
            | Self::StreamEnded { generation, .. }
            | Self::Drained { generation, .. }
            | Self::Error { generation, .. } => *generation,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionInputState {
    Running,
    Paused,
    Draining,
}

impl SessionInputState {
    const fn accepts_audio(self) -> bool {
        matches!(self, Self::Running)
    }

    const fn accepts_controls(self) -> bool {
        !matches!(self, Self::Draining)
    }
}

struct OutboundMessage {
    generation: Option<PipelineGeneration>,
    message: Message,
}

impl OutboundMessage {
    fn current(generation: PipelineGeneration, message: Message) -> Self {
        Self {
            generation: Some(generation),
            message,
        }
    }

    fn independent(message: Message) -> Self {
        Self {
            generation: None,
            message,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "xrtranslate-backend",
    version,
    about = "Native XRTranslate backend"
)]
struct Arguments {
    /// Path to the compatibility config.json file.
    #[arg(long, default_value = "config.json")]
    config: std::path::PathBuf,
    /// Start and own the two local llama.cpp model servers for this backend.
    ///
    /// Leave this off only when the Qwen3-ASR and Hy-MT2 endpoints are already
    /// managed by another native process.
    #[arg(long)]
    manage_llama_servers: bool,
    /// Maximum time to wait for managed llama-server instances to report ready.
    #[arg(long, default_value_t = 120)]
    model_start_timeout_seconds: u64,
    #[arg(long, default_value = "http://127.0.0.1:7766")]
    corpus_url: String,
}

#[derive(Clone)]
struct BackendState {
    config: AppConfig,
    model_plan: Arc<NativeProviderPlan>,
    corpus_client: CorpusClient,
    project_root: PathBuf,
    voice_clones_dir: PathBuf,
    next_session_id: Arc<AtomicU64>,
    inference_scheduler: InferenceScheduler,
    tts: Option<NativeTtsAdapter>,
    tts_runtime: OnnxRuntimeDiagnostic,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    runtime: &'static str,
    protocol_version: u16,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run_backend().await {
        eprintln!("[XRTRANSLATE_STARTUP_ERROR] {error}");
        std::process::exit(1);
    }
}

async fn run_backend() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let args = Arguments::parse();
    let configured_root = args
        .config
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let project_root = std::path::absolute(&configured_root).unwrap_or(configured_root);
    let config = AppConfig::from_path_with_user_config(&args.config, &project_root)?;
    initialize_managed_onnx_runtime(&project_root, &config)?;
    let model_plan = Arc::new(NativeProviderPlan::resolve(&config, &project_root)?);
    validate_native_route(&config, &project_root, &model_plan)?;
    let corpus_client = CorpusClient::new(&args.corpus_url)?;
    let corpus_health = corpus_client.ensure_compatible().await?;
    info!(
        count = corpus_health.corpus_count,
        api_version = corpus_health.api_version,
        "connected to XR Corpus"
    );
    let _model_processes = if args.manage_llama_servers {
        let mut processes = start_llama_servers(&model_plan)?;
        wait_for_model_servers(
            &model_plan,
            args.model_start_timeout_seconds,
            &mut processes,
        )
        .await?;
        info!("managed llama.cpp model servers are ready");
        Some(processes)
    } else {
        None
    };
    let address = format!("{}:{}", config.server.host, config.server.port);
    let inference_scheduler = InferenceScheduler::new(
        usize::from(model_plan.asr_runtime().parallel_slots),
        usize::from(model_plan.translation_runtime().parallel_slots),
    );
    let tts = model_plan.tts_adapter(&config)?;
    let tts_runtime = match &tts {
        Some(adapter) => {
            let device = adapter.prepare().await.map_err(|error| error.to_string())?;
            runtime_diagnostic(&project_root, &config, device)
        }
        None => OnnxRuntimeDiagnostic::default(),
    };
    let voice_clones_dir =
        RuntimeLayout::for_config(&project_root, &config.model_manager).voice_clones_directory();
    if let Some(adapter) = &tts {
        let restored = restore_persisted_voice_clones(&voice_clones_dir, adapter).await;
        if restored > 0 {
            info!(restored, "restored persisted voice clones");
        }
    }
    info!(
        provider = %config.tts.provider,
        configured = tts.is_some(),
        backend = tts_runtime.backend.as_deref().unwrap_or("none"),
        cuda = tts_runtime.cuda_version.as_deref().unwrap_or("none"),
        "native TTS runtime configured"
    );
    let state = BackendState {
        config,
        model_plan,
        corpus_client,
        project_root,
        voice_clones_dir,
        next_session_id: Arc::new(AtomicU64::new(1)),
        inference_scheduler,
        tts,
        tts_runtime,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/ws", get(websocket))
        .route("/integrations/vrcx/status", get(vrcx_status))
        .with_state(state);

    let listener = TcpListener::bind(&address).await?;
    info!(%address, "native backend transport is listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        runtime: "native-gguf",
        protocol_version: xrtranslate_protocol::PROTOCOL_VERSION,
    })
}

async fn vrcx_status(State(state): State<BackendState>) -> impl IntoResponse {
    match state.corpus_client.vrcx_status().await {
        Ok(status) => (axum::http::StatusCode::OK, Json(status)).into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn websocket(
    State(state): State<BackendState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| serve_session(socket, state))
}

async fn serve_session(socket: WebSocket, state: BackendState) {
    let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
    let mut session = match SessionAdapter::new(
        &state.config.translation.source_lang,
        &state.config.translation.target_lang,
    ) {
        Ok(session) => session,
        Err(error) => {
            warn!(%session_id, %error, "invalid session route");
            return;
        }
    };
    let mut generation = PipelineGeneration {
        route_epoch: session.route_epoch(),
        audio_epoch: AudioEpoch::INITIAL,
    };
    let (generation_sender, generation_receiver) = watch::channel(generation);
    let (socket_writer, mut reader) = socket.split();
    let (outbound_sender, outbound_receiver) = mpsc::channel(OUTBOUND_MESSAGE_CAPACITY);
    let mut writer_task = tokio::spawn(run_websocket_writer(
        socket_writer,
        outbound_receiver,
        generation_receiver.clone(),
    ));
    let mut pipeline =
        match NativePipeline::new(&state.config, &state.project_root, &state.model_plan) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                warn!(%session_id, %error, "native pipeline initialization failed");
                let _ = send_error(&outbound_sender, error).await;
                return;
            }
        };
    let mut input_format = PcmFormat::mono_s16le(state.config.audio.sample_rate);
    let speaker_available = pipeline.inference().speaker_is_available();
    let speaker_recognition_enabled = Arc::new(AtomicBool::new(false));
    let speaker_state_revision = Arc::new(AtomicU64::new(0));

    let (job_sender, job_receiver) = mpsc::channel(INFERENCE_QUEUE_CAPACITY);
    let (result_sender, mut result_receiver) = mpsc::channel(INFERENCE_RESULT_CAPACITY);
    let prompt_graphs = Arc::new(tokio::sync::RwLock::new(PromptGraphSet {
        graph: PromptNodeGraph::builtin_default(),
    }));
    let worker = tokio::spawn(run_inference_worker(
        pipeline.inference(),
        job_receiver,
        result_sender,
        generation_receiver,
        state.corpus_client.clone(),
        state.inference_scheduler.clone(),
        Arc::clone(&speaker_recognition_enabled),
        Arc::clone(&speaker_state_revision),
        Arc::clone(&prompt_graphs),
    ));
    let mut job_sender = Some(job_sender);
    let mut input_state = SessionInputState::Running;
    let mut graceful_shutdown = false;
    let mut next_utterance_sequence = 1_u64;
    let mut workload = InferenceWorkload::Realtime;
    let tts = state.tts.clone();
    let (tts_job_sender, tts_job_receiver) = mpsc::channel(TTS_QUEUE_CAPACITY);
    let (tts_result_sender, mut tts_result_receiver) = mpsc::channel(TTS_QUEUE_CAPACITY);
    let mut tts_worker = tts
        .clone()
        .map(|adapter| tokio::spawn(run_tts_worker(adapter, tts_job_receiver, tts_result_sender)));
    let tts_job_sender = tts.as_ref().map(|_| tts_job_sender);
    let mut tts_result_open = tts_worker.is_some();
    let mut pending_tts_jobs = 0_usize;
    let mut pending_drain = None;
    let mut clone_capture = VoiceCloneCapture::from_config(&state.config);
    let tts_max_input_chars = tts_max_input_chars(&state.config);
    let mut audio_source = xrtranslate_protocol::AudioSource::Microphone;

    if send_event(
        &outbound_sender,
        None,
        ServerEvent::SessionReady(SessionReady {
            session_id: format!("native-{session_id}"),
            source_lang: session.source_lang().into(),
            target_lang: session.target_lang().into(),
            tts_backend: state.tts_runtime.backend.clone(),
            tts_cuda_version: state.tts_runtime.cuda_version.clone(),
        }),
    )
    .await
    .is_err()
    {
        return;
    }

    'session: loop {
        tokio::select! {
            result = result_receiver.recv() => {
                let Some(result) = result else {
                    break;
                };
                let drained = match &result {
                    InferenceEvent::Drained { reason, .. } => Some(*reason),
                    _ => None,
                };
                if let InferenceEvent::WindowObserved { text_units, .. } = &result {
                    pipeline.observe_text_density(*text_units);
                }
                if let InferenceEvent::Recognized { recognized, reference_samples: Some(samples), .. } = &result
                    && clone_capture.armed
                {
                    let remaining = clone_capture.maximum_samples.saturating_sub(clone_capture.samples.len());
                    clone_capture.samples.extend_from_slice(&samples[..samples.len().min(remaining)]);
                    if !recognized.source_text.trim().is_empty() {
                        clone_capture.transcript.push(recognized.source_text.trim().to_owned());
                    }
                    let collected = clone_capture.collected_seconds();
                    if send_event(&outbound_sender, Some(generation), ServerEvent::VoiceCloneState(VoiceCloneState {
                        state: VoiceClonePhase::Collecting,
                        collected_seconds: collected,
                        required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32,
                        message: None,
                    })).await.is_err() { break; }
                    if clone_capture.samples.len() >= clone_capture.minimum_samples {
                        let samples = std::mem::take(&mut clone_capture.samples);
                        let transcript = std::mem::take(&mut clone_capture.transcript).join(" ");
                        clone_capture.armed = false;
                        let registering = VoiceCloneState { state: VoiceClonePhase::Registering, collected_seconds: collected, required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32, message: None };
                        if send_event(&outbound_sender, Some(generation), ServerEvent::VoiceCloneState(registering)).await.is_err() { break; }
                        let registration = match &tts {
                            Some(tts) => {
                                let pcm = samples.into_iter().flat_map(i16::to_le_bytes).collect::<Vec<_>>();
                                match pcm16_mono_16khz_to_wav(&pcm) {
                                    Ok(wav) => {
                                        let result = tts
                                            .register_voice(clone_voice_name(audio_source), wav.clone(), &transcript)
                                            .await
                                            .map_err(|error| error.to_string());
                                        if result.is_ok() {
                                            if let Err(error) = save_persisted_voice_clone(
                                                &state.voice_clones_dir,
                                                clone_voice_name(audio_source),
                                                &wav,
                                                &transcript,
                                            ) {
                                                warn!(
                                                    voice = clone_voice_name(audio_source),
                                                    %error,
                                                    "failed to persist voice clone to disk"
                                                );
                                            }
                                        }
                                        result
                                    }
                                    Err(error) => Err(error.to_string()),
                                }
                            }
                            None => Err("Select and configure a TTS provider before cloning a voice.".into()),
                        };
                        if registration.is_ok() {
                            clone_capture.ready = true;
                        }
                        let state = match registration {
                            Ok(()) => {
                                info!(
                                    %session_id,
                                    voice = clone_voice_name(audio_source),
                                    collected_seconds = collected,
                                    "TTS voice clone registered"
                                );
                                VoiceCloneState { state: VoiceClonePhase::Ready, collected_seconds: 0.0, required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32, message: None }
                            },
                            Err(message) => VoiceCloneState { state: VoiceClonePhase::Failed, collected_seconds: 0.0, required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32, message: Some(message) },
                        };
                        clone_capture.clear_capture();
                        if send_event(&outbound_sender, Some(generation), ServerEvent::VoiceCloneState(state)).await.is_err() { break; }
                    }
                }
                match handle_inference_event(
                    &outbound_sender,
                    &mut session,
                    generation,
                    result,
                    tts.as_ref(),
                    tts_job_sender.as_ref(),
                    clone_voice_name(audio_source),
                    clone_capture.ready,
                    tts_max_input_chars,
                ).await {
                    Ok(true) => pending_tts_jobs += 1,
                    Ok(false) => {}
                    Err(_) => break,
                }
                if let Some(reason) = drained {
                    if pending_tts_jobs == 0 {
                        if send_event(
                            &outbound_sender,
                            Some(generation),
                            ServerEvent::PipelineDrained(PipelineDrained { reason }),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        if reason != DrainReason::Paused {
                            graceful_shutdown = true;
                            break;
                        }
                    } else {
                        pending_drain = Some(reason);
                    }
                }
            }
            tts_result = tts_result_receiver.recv(), if tts_result_open => {
                let Some(tts_result) = tts_result else {
                    tts_result_open = false;
                    continue;
                };
                pending_tts_jobs = pending_tts_jobs.saturating_sub(1);
                if tts_result.generation == generation
                    && tts_result.tts_epoch == session.tts_epoch()
                {
                    match tts_result.output {
                        Ok(chunks) => {
                            for audio in chunks {
                                if session
                                    .submit_tts_audio(
                                        tts_result.generation.route_epoch,
                                        tts_result.tts_epoch,
                                        audio.bytes,
                                    )
                                    .unwrap_or(false)
                                {
                                    if send_session_output(&outbound_sender, &mut session, generation)
                                        .await
                                        .is_err()
                                    {
                                        break 'session;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            if send_scoped_error(
                                &outbound_sender,
                                generation,
                                format!("TTS failed: {error}"),
                                error.requires_provider_configuration(),
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                if pending_tts_jobs == 0
                    && let Some(reason) = pending_drain.take()
                {
                    if send_event(
                        &outbound_sender,
                        Some(generation),
                        ServerEvent::PipelineDrained(PipelineDrained { reason }),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    if reason != DrainReason::Paused {
                        graceful_shutdown = true;
                        break;
                    }
                }
            }
            frame = reader.next(), if input_state.accepts_controls() => {
                let Some(frame) = frame else {
                    break;
                };
                let Ok(frame) = frame else {
                    break;
                };
                match frame {
                    Message::Text(text) => match serde_json::from_str::<ClientControl>(&text) {
                        Ok(ClientControl::Action(ActionControl::SessionConfig {
                            source_lang: source,
                            target_lang: target,
                            sample_rate,
                            prompt_graphs: graphs,
                        })) => {
                            if let Some(graphs) = graphs {
                                if let Err(error) = validate_prompt_graph_set(&graphs) {
                                    if send_error(
                                        &outbound_sender,
                                        format!("Invalid prompt graph: {error}"),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                                *prompt_graphs.write().await = graphs;
                            }
                            if let Some(sample_rate) = sample_rate {
                                if let Err(error) = validate_input_sample_rate(sample_rate) {
                                    if send_error(&outbound_sender, error).await.is_err() {
                                        break;
                                    }
                                    continue;
                                }
                                input_format = PcmFormat::mono_s16le(sample_rate);
                            }
                            if let Err(error) = session.set_route(&source, &target) {
                                if send_error(&outbound_sender, error).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            pipeline.reset();
                            generation.route_epoch = session.route_epoch();
                            generation.audio_epoch.advance();
                            generation_sender.send_replace(generation);
                            info!(%session_id, source_lang = session.source_lang(), target_lang = session.target_lang(), "session route configured");
                        }
                        Ok(ClientControl::Action(ActionControl::SetPromptGraphs {
                            prompt_graphs: graphs,
                        })) => {
                            if let Err(error) = validate_prompt_graph_set(&graphs) {
                                if send_error(
                                    &outbound_sender,
                                    format!("Invalid prompt graph: {error}"),
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                            *prompt_graphs.write().await = graphs;
                        }
                        Ok(ClientControl::Event(EventControl::ConfigAudio {
                            sample_rate,
                            source_lang: source,
                            target_lang: target,
                            audio_source: configured_audio_source,
                            vad_threshold,
                            vad_silence_ms,
                            continuous_recognition: configured_continuous_recognition,
                            workload: configured_workload,
                        })) => {
                            if let Err(error) = validate_input_sample_rate(sample_rate) {
                                if send_error(&outbound_sender, error).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            input_format = PcmFormat::mono_s16le(sample_rate);
                            if let Err(error) = pipeline.configure_segmentation(
                                vad_threshold,
                                vad_silence_ms,
                                configured_continuous_recognition,
                                configured_audio_source,
                            ) {
                                if send_error(&outbound_sender, error).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            if let Err(error) = session.set_route(&source, &target) {
                                if send_error(&outbound_sender, error).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            pipeline.reset();
                            workload = configured_workload;
                            audio_source = configured_audio_source;
                            clone_capture.ready = match &tts {
                                Some(tts) => tts.has_voice(clone_voice_name(audio_source)).await,
                                None => false,
                            };
                            generation.route_epoch = session.route_epoch();
                            generation.audio_epoch.advance();
                            generation_sender.send_replace(generation);
                            info!(
                                %session_id,
                                source_lang = session.source_lang(),
                                target_lang = session.target_lang(),
                                sample_rate,
                                voice = clone_voice_name(audio_source),
                                voice_ready = clone_capture.ready,
                                "audio configured"
                            );
                            if clone_capture.ready
                                && send_event(&outbound_sender, Some(generation), ServerEvent::VoiceCloneState(VoiceCloneState {
                                    state: VoiceClonePhase::Ready,
                                    collected_seconds: 0.0,
                                    required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32,
                                    message: None,
                                })).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(ClientControl::Event(EventControl::Pause)) => {
                            if input_state == SessionInputState::Running {
                                let Some(sender) = job_sender.as_ref() else { break };
                                if let Err(error) = queue_pipeline_drain(
                                    &mut pipeline,
                                    sender,
                                    &session,
                                    generation,
                                    workload,
                                    DrainReason::Paused,
                                    &mut next_utterance_sequence,
                                )
                                .await
                                {
                                    if send_error(&outbound_sender, error).await.is_err() {
                                        break;
                                    }
                                }
                                let mut send_failed = false;
                                for active in pipeline.take_vad_transitions() {
                                    if send_event(
                                        &outbound_sender,
                                        Some(generation),
                                        ServerEvent::VadActivity(VadActivity { active }),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        send_failed = true;
                                        break;
                                    }
                                }
                                if send_failed {
                                    break;
                                }
                                input_state = SessionInputState::Paused;
                            }
                        }
                        Ok(ClientControl::Event(EventControl::Resume)) => {
                            if input_state == SessionInputState::Paused {
                                input_state = SessionInputState::Running;
                            }
                        }
                        Ok(ClientControl::Event(control @ (EventControl::Finish | EventControl::InputEnded | EventControl::Stop))) => {
                            let reason = match control {
                                EventControl::Finish => DrainReason::Finished,
                                EventControl::InputEnded => DrainReason::InputEnded,
                                EventControl::Stop => DrainReason::Stopped,
                                _ => unreachable!(),
                            };
                            let Some(sender) = job_sender.as_ref() else { break };
                            if let Err(error) = queue_pipeline_drain(
                                &mut pipeline,
                                sender,
                                &session,
                                    generation,
                                    workload,
                                    reason,
                                &mut next_utterance_sequence,
                            )
                            .await
                            {
                                if send_error(&outbound_sender, error).await.is_err() {
                                    break;
                                }
                            }
                            let mut send_failed = false;
                            for active in pipeline.take_vad_transitions() {
                                if send_event(
                                    &outbound_sender,
                                    Some(generation),
                                    ServerEvent::VadActivity(VadActivity { active }),
                                )
                                .await
                                .is_err()
                                {
                                    send_failed = true;
                                    break;
                                }
                            }
                            if send_failed {
                                break;
                            }
                            job_sender.take();
                            input_state = SessionInputState::Draining;
                        }
                        Ok(ClientControl::Action(ActionControl::ToggleFeature { feature, enabled })) => {
                            match feature {
                                Feature::Tts => session.set_tts_enabled(enabled),
                                Feature::SpeakerRecognition if enabled && !speaker_available => {
                                    if send_error(
                                        &outbound_sender,
                                        "speaker recognition is unavailable; enable speaker.enabled and install its model".into(),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break;
                                    }
                                    continue;
                                }
                                Feature::SpeakerRecognition => {
                                    if speaker_recognition_enabled.swap(enabled, Ordering::AcqRel)
                                        != enabled
                                    {
                                        speaker_state_revision.fetch_add(1, Ordering::AcqRel);
                                    }
                                    pipeline.set_speaker_recognition_enabled(enabled);
                                }
                            }
                            info!(%session_id, ?feature, enabled, "session feature configured");
                        }
                        Ok(ClientControl::Action(ActionControl::BeginVoiceClone)) => {
                            clone_capture.arm();
                            info!(
                                %session_id,
                                voice = clone_voice_name(audio_source),
                                required_seconds = clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32,
                                "TTS voice clone capture armed"
                            );
                            if send_event(&outbound_sender, Some(generation), ServerEvent::VoiceCloneState(VoiceCloneState {
                                state: VoiceClonePhase::Collecting,
                                collected_seconds: 0.0,
                                required_seconds: clone_capture.minimum_samples as f32 / SAMPLE_RATE_HZ as f32,
                                message: None,
                            })).await.is_err() { break; }
                        }
                        Ok(ClientControl::Action(ActionControl::TranslateText {
                            text,
                            source_lang,
                            target_lang,
                            stream_id: _,
                        })) => {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() && input_state != SessionInputState::Draining {
                                let Some(sender) = job_sender.as_ref() else { break };
                                let source_language = source_lang
                                    .filter(|s| !s.trim().is_empty())
                                    .unwrap_or_else(|| session.source_lang().to_string());
                                let target_language = target_lang
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or_else(|| session.target_lang().to_string());
                                let revision = next_utterance_sequence;
                                let turn_id = format!("text-{revision}");
                                next_utterance_sequence = next_utterance_sequence.wrapping_add(1);
                                let topic_turn_id = turn_id.clone();
                                let job = TextJob {
                                    text: trimmed.to_string(),
                                    generation,
                                    turn_id,
                                    topic_turn_id,
                                    source_language,
                                    target_language,
                                    speaker_id: None,
                                    workload,
                                    enqueued_at: Instant::now(),
                                    revision,
                                };
                                if sender.send(InferenceJob::Text(job)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Ok(ClientControl::Event(EventControl::TurnStarted { turn_id })) => {
                            session.set_turn_id(turn_id);
                        }
                        Err(error) => {
                            if send_error(&outbound_sender, format!("Invalid client control: {error}"))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    },
                    Message::Binary(audio) => {
                        if !input_state.accepts_audio() {
                            continue;
                        }
                        if let Err(error) = validate_input_chunk_size(audio.len()) {
                            if send_error(&outbound_sender, error).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        match PcmFrame::new(audio.to_vec(), input_format) {
                            Ok(frame) => match pipeline.push_pcm(frame.as_bytes()) {
                                Ok(utterances) => {
                                    let mut vad_send_failed = false;
                                    for active in pipeline.take_vad_transitions() {
                                        if send_event(
                                            &outbound_sender,
                                            Some(generation),
                                            ServerEvent::VadActivity(VadActivity { active }),
                                        )
                                        .await
                                        .is_err()
                                        {
                                            vad_send_failed = true;
                                            break;
                                        }
                                    }
                                    if vad_send_failed {
                                        break;
                                    }
                                    if let Some(sender) = job_sender.as_ref()
                                        && let Err(error) = enqueue_utterances(
                                            sender,
                                            &session,
                                            generation,
                                            workload,
                                            utterances,
                                            &mut next_utterance_sequence,
                                        )
                                        .await
                                        && send_error(&outbound_sender, error).await.is_err() {
                                            break;
                                        }
                                }
                                Err(error) => {
                                    if send_error(&outbound_sender, error).await.is_err() {
                                        break;
                                    }
                                }
                            },
                            Err(error) => {
                                if send_error(&outbound_sender, error.to_string()).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if outbound_sender
                            .send(OutboundMessage::independent(Message::Pong(payload)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                }
            }
        }
    }

    job_sender.take();
    if !graceful_shutdown {
        worker.abort();
    }
    let _ = worker.await;
    drop(tts_job_sender);
    if let Some(tts_worker) = tts_worker.as_mut() {
        if !graceful_shutdown {
            tts_worker.abort();
        }
        let _ = tts_worker.await;
    }
    drop(outbound_sender);
    if graceful_shutdown {
        if tokio::time::timeout(Duration::from_secs(2), &mut writer_task)
            .await
            .is_err()
        {
            writer_task.abort();
        }
    } else {
        writer_task.abort();
    }
}

fn validate_native_route(
    config: &AppConfig,
    project_root: &std::path::Path,
    model_plan: &NativeProviderPlan,
) -> Result<(), String> {
    validate_input_sample_rate(config.audio.sample_rate)?;
    if model_plan.uses_local_runtime() {
        model_plan.check_assets()?;
    }
    let vad_path = project_root.join("models/silero-vad/src/silero_vad/data/silero_vad.onnx");
    if !vad_path.is_file() {
        return Err(format!(
            "native Silero VAD model is missing: {}",
            vad_path.display()
        ));
    }
    Ok(())
}

fn start_llama_servers(model_plan: &NativeProviderPlan) -> Result<Vec<LlamaServerProcess>, String> {
    if !model_plan.llama_server_path().is_file() {
        return Err(format!(
            "llama-server executable is missing: {}",
            model_plan.llama_server_path().display()
        ));
    }

    model_plan.check_assets()?;
    let asr_port = model_plan
        .asr_uses_local_runtime()
        .then(|| local_endpoint_port(model_plan.asr_url()))
        .transpose()?;
    let translation_port = model_plan
        .translation_uses_local_runtime()
        .then(|| local_endpoint_port(model_plan.translation_url()))
        .transpose()?;
    if let (Some(asr_port), Some(translation_port)) = (asr_port, translation_port)
        && asr_port == translation_port
    {
        return Err(format!(
            "ASR and translation llama-server endpoints both use port {asr_port}"
        ));
    }

    let (asr_spec, translation_spec) =
        model_plan.managed_server_specs(asr_port.unwrap_or(0), translation_port.unwrap_or(0))?;

    let launcher = StdLlamaServerLauncher;
    let mut processes = Vec::new();
    if let Some(spec) = asr_spec {
        processes.push(
            launcher
                .launch(&spec)
                .map_err(|error| format!("cannot start Qwen3-ASR llama-server: {error}"))?,
        );
    }
    if let Some(spec) = translation_spec {
        processes.push(
            launcher
                .launch(&spec)
                .map_err(|error| format!("cannot start Hy-MT2 llama-server: {error}"))?,
        );
    }
    info!(
        asr_port,
        translation_port, "started managed llama.cpp model servers"
    );
    Ok(processes)
}

async fn wait_for_model_servers(
    model_plan: &NativeProviderPlan,
    timeout_seconds: u64,
    processes: &mut [LlamaServerProcess],
) -> Result<(), String> {
    let asr_health = model_plan
        .asr_uses_local_runtime()
        .then(|| health_url(model_plan.asr_url()))
        .transpose()?;
    let translation_health = model_plan
        .translation_uses_local_runtime()
        .then(|| health_url(model_plan.translation_url()))
        .transpose()?;
    let asr_models = model_plan
        .asr_uses_local_runtime()
        .then(|| models_url(model_plan.asr_url()))
        .transpose()?;
    let translation_models = model_plan
        .translation_uses_local_runtime()
        .then(|| models_url(model_plan.translation_url()))
        .transpose()?;
    let client =
        ReqwestClient::new_direct(Duration::from_secs(2)).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);

    loop {
        for process in processes.iter_mut() {
            let role = process.role().model_alias();
            if let Some(status) = process
                .try_wait()
                .map_err(|error| format!("cannot inspect managed {role} process: {error}"))?
            {
                return Err(format!(
                    "managed {role} llama-server exited during startup ({status}); check whether its port is already in use and inspect the lines above this error"
                ));
            }
        }
        let asr = match (&asr_health, &asr_models) {
            (Some(health), Some(models)) => {
                check_model_ready(&client, health, models, model_plan.asr_model_alias()).await
            }
            _ => Ok(()),
        };
        let translation = match (&translation_health, &translation_models) {
            (Some(health), Some(models)) => {
                check_model_ready(
                    &client,
                    health,
                    models,
                    model_plan.translation_model_alias(),
                )
                .await
            }
            _ => Ok(()),
        };
        if asr.is_ok() && translation.is_ok() {
            return Ok(());
        }
        let last_status = format!(
            "Qwen3-ASR: {}; Hy-MT2: {}",
            health_status(&asr),
            health_status(&translation)
        );
        if Instant::now() >= deadline {
            return Err(format!(
                "managed llama.cpp model servers did not become ready within {timeout_seconds} seconds ({last_status})"
            ));
        }
        sleep(Duration::from_millis(300)).await;
    }
}

/// Confirms both llama.cpp readiness and the OpenAI model alias advertised by
/// the actual inference endpoint.  A bare `/health` response can be true for
/// a server that loaded the wrong model or failed to expose the requested API.
async fn check_model_ready(
    client: &ReqwestClient,
    health_url: &str,
    models_url: &str,
    expected_model_alias: &str,
) -> Result<(), String> {
    let response = client
        .execute(HttpRequest {
            method: "GET".into(),
            url: health_url.into(),
            headers: Vec::new(),
            body: serde_json::Value::Null,
        })
        .await
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!("health endpoint returned HTTP {}", response.status));
    }

    let response = client
        .execute(HttpRequest {
            method: "GET".into(),
            url: models_url.into(),
            headers: Vec::new(),
            body: serde_json::Value::Null,
        })
        .await
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!("models endpoint returned HTTP {}", response.status));
    }
    model_alias_is_advertised(&response.body, expected_model_alias)
}

fn model_alias_is_advertised(
    response_body: &str,
    expected_model_alias: &str,
) -> Result<(), String> {
    let document: serde_json::Value = serde_json::from_str(response_body)
        .map_err(|error| format!("models endpoint returned invalid JSON: {error}"))?;
    let Some(models) = document.get("data").and_then(serde_json::Value::as_array) else {
        return Err("models endpoint response is missing its data array".into());
    };
    let found = models.iter().any(|model| {
        model
            .get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id == expected_model_alias)
    });
    if found {
        Ok(())
    } else {
        Err(format!(
            "models endpoint does not advertise expected model alias {expected_model_alias:?}"
        ))
    }
}

fn health_status(status: &Result<(), String>) -> String {
    match status {
        Ok(()) => "ready".into(),
        Err(error) => error.clone(),
    }
}

fn local_endpoint_port(url: &str) -> Result<u16, String> {
    let parsed =
        url::Url::parse(url).map_err(|error| format!("invalid model URL {url:?}: {error}"))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "0.0.0.0") {
        return Err(format!(
            "--manage-llama-servers requires a local model URL, not {url:?}"
        ));
    }
    parsed
        .port()
        .ok_or_else(|| format!("managed local model URL must include an explicit port: {url:?}"))
}

fn health_url(chat_url: &str) -> Result<String, String> {
    local_model_url(chat_url, "/health")
}

fn models_url(chat_url: &str) -> Result<String, String> {
    local_model_url(chat_url, "/v1/models")
}

fn local_model_url(chat_url: &str, path: &str) -> Result<String, String> {
    let mut parsed = url::Url::parse(chat_url)
        .map_err(|error| format!("invalid model URL {chat_url:?}: {error}"))?;
    let _ = local_endpoint_port(chat_url)?;
    if parsed.host_str() == Some("0.0.0.0") {
        parsed
            .set_host(Some("127.0.0.1"))
            .map_err(|_| "cannot construct local llama-server health URL".to_owned())?;
    }
    parsed.set_path(path);
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.into())
}

async fn enqueue_utterances(
    sender: &mpsc::Sender<InferenceJob>,
    session: &SessionAdapter,
    generation: PipelineGeneration,
    workload: InferenceWorkload,
    utterances: Vec<PipelineEvent>,
    next_utterance_sequence: &mut u64,
) -> Result<(), String> {
    debug_assert_eq!(generation.route_epoch, session.route_epoch());
    let jobs = inference_jobs(
        session,
        generation,
        workload,
        utterances,
        next_utterance_sequence,
    )?;
    for job in jobs {
        sender
            .send(job)
            .await
            .map_err(|_| "native inference worker has stopped".to_owned())?;
    }
    Ok(())
}

fn inference_jobs(
    session: &SessionAdapter,
    generation: PipelineGeneration,
    workload: InferenceWorkload,
    utterances: Vec<PipelineEvent>,
    next_utterance_sequence: &mut u64,
) -> Result<Vec<InferenceJob>, String> {
    let turn_id_prefix = session.turn_id();
    let source_language = session.source_lang().to_owned();
    let target_language = session.target_lang().to_owned();
    let mut jobs = Vec::with_capacity(utterances.len());
    for event in utterances {
        if matches!(event, PipelineEvent::StreamEnded) {
            jobs.push(InferenceJob::StreamEnded {
                generation,
                turn_id: turn_id_prefix.clone(),
            });
            continue;
        }
        let PipelineEvent::Utterance(timed) = event else {
            unreachable!()
        };
        let TimedUtterance {
            utterance,
            source_start_ms,
            source_end_ms,
            revisable,
            topic_turn_sequence,
            speaker_id,
        } = timed;
        let revision = *next_utterance_sequence;
        let turn_id = format!("{turn_id_prefix}:utterance-{revision}");
        *next_utterance_sequence = (*next_utterance_sequence)
            .checked_add(1)
            .ok_or_else(|| "utterance identity counter exhausted".to_owned())?;
        let topic_turn_id = topic_turn_sequence.map_or_else(
            || turn_id.clone(),
            |sequence| format!("{turn_id_prefix}:generation-{generation:?}:speech-{sequence}"),
        );
        let duration_ms = utterance.samples.len().saturating_mul(1_000) / SAMPLE_RATE_HZ as usize;
        info!(
            duration_ms,
            overlap_frames = utterance.overlap_frames,
            end_reason = ?utterance.end_reason,
            "VAD finalized an utterance; queuing it for ASR"
        );
        jobs.push(InferenceJob::Utterance(UtteranceJob {
            utterance,
            source_start_ms,
            source_end_ms,
            revisable,
            generation,
            turn_id,
            topic_turn_id,
            source_language: source_language.clone(),
            target_language: target_language.clone(),
            speaker_id,
            workload,
            enqueued_at: Instant::now(),
            revision,
        }));
    }
    Ok(jobs)
}

/// Flushes the current VAD turn and places an ordered fence behind all queued
/// model work. Unlike live ingestion, this waits for bounded queue capacity so
/// a pause or EOF cannot silently discard its final utterance.
async fn queue_pipeline_drain(
    pipeline: &mut NativePipeline,
    sender: &mpsc::Sender<InferenceJob>,
    session: &SessionAdapter,
    generation: PipelineGeneration,
    workload: InferenceWorkload,
    reason: DrainReason,
    next_utterance_sequence: &mut u64,
) -> Result<(), String> {
    let flush_error = match pipeline.flush() {
        Ok(Some(utterance)) => {
            for job in inference_jobs(
                session,
                generation,
                workload,
                vec![utterance],
                next_utterance_sequence,
            )? {
                sender
                    .send(job)
                    .await
                    .map_err(|_| "native inference worker has stopped".to_owned())?;
            }
            None
        }
        Ok(None) => None,
        Err(error) => Some(error),
    };
    sender
        .send(InferenceJob::Drain { generation, reason })
        .await
        .map_err(|_| "native inference worker has stopped".to_owned())?;
    flush_error.map_or(Ok(()), Err)
}

async fn run_inference_worker(
    inference: NativeInference,
    mut jobs: mpsc::Receiver<InferenceJob>,
    events: mpsc::Sender<InferenceEvent>,
    mut generation: watch::Receiver<PipelineGeneration>,
    corpus_client: CorpusClient,
    scheduler: InferenceScheduler,
    speaker_recognition_enabled: Arc<AtomicBool>,
    speaker_state_revision: Arc<AtomicU64>,
    prompt_graphs: Arc<tokio::sync::RwLock<PromptGraphSet>>,
) {
    let mut previous_transcript: Option<(PipelineGeneration, String)> = None;
    let mut active_revisable_transcript: Option<(PipelineGeneration, String, RevisableTranscript)> =
        None;
    let mut adaptive_route = AdaptiveLanguageRoute::default();
    let mut diarizer: Option<xrtranslate_speaker::OnlineSpeakerDiarizer> = None;
    let speaker_min_utterance_ms = inference.speaker_min_utterance_ms().unwrap_or(0);
    let mut diarizer_generation: Option<PipelineGeneration> = Some(*generation.borrow());
    let mut speaker_revision_seen = 0;
    let mut speaker_load_failed = false;
    let mut corpus_session = match corpus_client.create_session().await {
        Ok(session) => session,
        Err(message) => {
            let current_generation = *generation.borrow();
            let _ = events
                .send(InferenceEvent::Error {
                    generation: current_generation,
                    message: message.to_string(),
                    configuration_required: false,
                })
                .await;
            return;
        }
    };
    let mut pending_translation: Option<tokio::task::JoinHandle<Vec<InferenceEvent>>> = None;
    // Stable sentence translations are reusable across revisions of one
    // logical stream. The cache is bounded and excludes the revisable tail,
    // which must always be regenerated.
    let stable_translation_cache = Arc::new(tokio::sync::Mutex::new(HashMap::<
        String,
        TranslationOutput,
    >::new()));
    loop {
        let job = if let Some(pending) = pending_translation.as_mut() {
            tokio::select! {
                job = jobs.recv() => job,
                completed = pending => {
                    pending_translation = None;
                    let Ok(batch) = completed else { break };
                    if !send_inference_batch(&events, batch).await {
                        break;
                    }
                    continue;
                }
            }
        } else {
            jobs.recv().await
        };
        let Some(job) = job else {
            if let Some(pending) = pending_translation.take() {
                let Ok(batch) = pending.await else { break };
                let _ = send_inference_batch(&events, batch).await;
            }
            break;
        };
        let job = match job {
            InferenceJob::Utterance(job) => job,
            InferenceJob::Text(job) => {
                if let Some(pending) = pending_translation.take() {
                    let Ok(batch) = pending.await else { break };
                    if !send_inference_batch(&events, batch).await {
                        break;
                    }
                }
                if *generation.borrow() != job.generation {
                    continue;
                }
                let job_queue_elapsed = job.enqueued_at.elapsed();
                let (asr_tokens, translation_tokens) = inference.prompt_context_token_budgets();
                let context_budgets = ContextBudgets {
                    asr_tokens,
                    translation_tokens,
                };
                let (routed_source, routed_target) = if let Some((new_src, new_tgt)) =
                    xrtranslate_engine::auto_route_language_pair(
                        &job.text,
                        &job.source_language,
                        &job.target_language,
                    ) {
                    (new_src.to_string(), new_tgt.to_string())
                } else {
                    (job.source_language.clone(), job.target_language.clone())
                };
                adaptive_route.configure(&routed_source, &routed_target);
                let active_target_language = adaptive_route.active_targets(&routed_target);
                let asr_context = match corpus_session
                    .prepare_asr(&PrepareAsrRequest {
                        source_language: routed_source.clone(),
                        target_language: active_target_language,
                        budgets: context_budgets,
                    })
                    .await
                {
                    Ok(prompt) => prompt,
                    Err(message) => {
                        if events
                            .send(InferenceEvent::Error {
                                generation: job.generation,
                                message: message.to_string(),
                                configuration_required: false,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                let segments =
                    translation_segment_pairs_for_final_text_with_lang(&job.text, &routed_source);
                let mut recognized = RecognizedOutput {
                    source_text: job.text.clone(),
                    segments,
                    source_language: routed_source,
                    target_language: routed_target,
                    asr_elapsed: Duration::ZERO,
                    route_switched: None,
                    prompt_trace: None,
                };
                let translation_context = match corpus_session
                    .prepare_translation(&PrepareTranslationRequest {
                        asr_context_id: asr_context.context_id,
                        turn_id: Some(job.topic_turn_id.clone()),
                        speaker_id: job.speaker_id.clone().unwrap_or_default(),
                        source_language: recognized.source_language.clone(),
                        target_language: recognized.target_language.clone(),
                        recognized_text: recognized.source_text.clone(),
                        segments: recognized
                            .segments
                            .iter()
                            .map(|segment| segment.translation_text.clone())
                            .collect(),
                        budgets: context_budgets,
                    })
                    .await
                {
                    Ok(context) => context,
                    Err(message) => {
                        if events
                            .send(InferenceEvent::Error {
                                generation: job.generation,
                                message: message.to_string(),
                                configuration_required: false,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                let source_rewrite = rewrite_recognition_terms(
                    &recognized.source_text,
                    &translation_context.source_corrections,
                );
                if source_rewrite.corrected_text != recognized.source_text {
                    recognized.apply_source_correction(source_rewrite.corrected_text.clone());
                }
                let translation_context_segments =
                    align_translation_contexts(&recognized.segments, &translation_context.segments);
                for (segment, context) in recognized
                    .segments
                    .iter_mut()
                    .zip(&translation_context_segments)
                {
                    let rewrite = rewrite_recognition_terms(
                        &segment.translation_text,
                        &context.source_corrections,
                    );
                    segment.translation_text = rewrite.corrected_text;
                    segment.source_text.clone_from(&segment.translation_text);
                }
                let asr_elapsed = Duration::ZERO;
                let source_language = recognized.source_language.clone();
                let target_language = recognized.target_language.clone();
                let segments = recognized.segments.clone();
                let segment_contexts = segment_contexts(
                    &segments,
                    &translation_context_segments,
                    job.turn_id.clone(),
                    job.speaker_id.clone().unwrap_or_default(),
                    StreamWindowContext {
                        start_ms: 0.0,
                        end_ms: 0.0,
                        revisable: false,
                        overlap_ratio: 0.0,
                        boundary: SegmentBoundary::InputBoundary,
                        authoritative_snapshot: false,
                        revision: job.revision,
                    },
                );
                if events
                    .send(InferenceEvent::Recognized {
                        generation: job.generation,
                        recognized,
                        segments: segment_contexts.clone(),
                        reference_samples: None,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                let translation_generation = generation.clone();
                let event_generation = job.generation;
                let workload = job.workload;
                let turn_started_at = job.enqueued_at;
                let queue_elapsed = job_queue_elapsed;
                let context_id = translation_context.context_id;
                let corpus_session = corpus_session.clone();
                let inference = inference.clone();
                let scheduler = scheduler.clone();
                let logical_turn_id = job.topic_turn_id.clone();
                let history_speaker_id = job.speaker_id.unwrap_or_default();
                let history_source_language = source_language.clone();
                let history_target_language = target_language.clone();
                let prompt_graph_for_turn = prompt_graphs.read().await.graph.clone();
                let turn_id_for_end = job.turn_id.clone();
                pending_translation = Some(tokio::spawn(async move {
                    let translations = futures_util::stream::iter(
                        segments
                            .into_iter()
                            .zip(segment_contexts)
                            .zip(translation_context.segments),
                    )
                    .map(|((segment, segment_context), corpus_context)| {
                        let inference = inference.clone();
                        let scheduler = scheduler.clone();
                        let source_language = source_language.clone();
                        let target_language = target_language.clone();
                        let source_for_terms = segment.translation_text.clone();
                        let prompt_terms = corpus_context.prompt_terms.clone();
                        let prompt_graph = prompt_graph_for_turn.clone();
                        async move {
                            let _permit = scheduler.acquire_translation(workload).await;
                            let prompt_context = prompt_context_for_segment(
                                &source_language,
                                &target_language,
                                &corpus_context,
                            );
                            let output = inference
                                .translate_segment(
                                    &segment,
                                    &source_language,
                                    &target_language,
                                    prompt_graph,
                                    prompt_context,
                                )
                                .await;
                            (segment_context, source_for_terms, prompt_terms, output)
                        }
                    })
                    .buffered(TRANSLATION_CONCURRENCY_PER_SESSION);
                    tokio::pin!(translations);
                    let mut batch = Vec::new();
                    let mut completed_pairs = Vec::new();
                    let mut cancelled = false;
                    while let Some((segment_context, source_for_terms, prompt_terms, mut output)) =
                        translations.next().await
                    {
                        if *translation_generation.borrow() != event_generation {
                            cancelled = true;
                            break;
                        }
                        if let Ok(translated) = &mut output {
                            let rewrite = rewrite_translation_terms(
                                &source_for_terms,
                                &translated.translated_text,
                                &translated.target_language,
                                &prompt_terms,
                            );
                            translated.translated_text = rewrite.translated_text;
                            translated.term_matches = rewrite.term_matches;
                            completed_pairs.push((
                                translated.source_text.clone(),
                                translated.translated_text.clone(),
                            ));
                        }
                        batch.push(InferenceEvent::Translation {
                            generation: event_generation,
                            target_language: target_language.clone(),
                            queue_elapsed,
                            asr_elapsed,
                            total_elapsed: turn_started_at.elapsed(),
                            context: segment_context,
                            output,
                        });
                    }
                    if !cancelled
                        && let Some(request) = (LogicalTurnRecord {
                            context_id,
                            turn_id: logical_turn_id,
                            speaker_id: history_speaker_id,
                            source_language: &history_source_language,
                            target_language: &history_target_language,
                            completed_pairs: &completed_pairs,
                        })
                        .into_request()
                    {
                        if let Err(message) = corpus_session.record_translation(&request).await {
                            warn!(%message, "could not record XR Corpus translation context");
                        }
                    }
                    batch.push(InferenceEvent::StreamEnded {
                        generation: event_generation,
                        turn_id: turn_id_for_end,
                    });
                    batch
                }));
                continue;
            }
            InferenceJob::StreamEnded {
                generation: event_generation,
                turn_id,
            } => {
                if let Some(pending) = pending_translation.take() {
                    let Ok(batch) = pending.await else { break };
                    if !send_inference_batch(&events, batch).await {
                        break;
                    }
                }
                if *generation.borrow() == event_generation
                    && events
                        .send(InferenceEvent::StreamEnded {
                            generation: event_generation,
                            turn_id,
                        })
                        .await
                        .is_err()
                {
                    break;
                }
                active_revisable_transcript = None;
                stable_translation_cache.lock().await.clear();
                continue;
            }
            InferenceJob::Drain {
                generation: event_generation,
                reason,
            } => {
                if let Some(pending) = pending_translation.take() {
                    let Ok(batch) = pending.await else { break };
                    if !send_inference_batch(&events, batch).await {
                        break;
                    }
                }
                if *generation.borrow() == event_generation
                    && events
                        .send(InferenceEvent::Drained {
                            generation: event_generation,
                            reason,
                        })
                        .await
                        .is_err()
                {
                    break;
                }
                continue;
            }
        };
        if *generation.borrow() != job.generation {
            continue;
        }
        let job_queue_elapsed = job.enqueued_at.elapsed();
        if job_queue_elapsed >= Duration::from_millis(100) {
            warn!(
                queue_ms = job_queue_elapsed.as_millis(),
                workload = ?job.workload,
                "utterance waited for inference worker"
            );
        }
        if previous_transcript
            .as_ref()
            .is_some_and(|(previous_generation, _)| *previous_generation != job.generation)
        {
            previous_transcript = None;
        }
        if diarizer_generation != Some(job.generation) {
            adaptive_route = AdaptiveLanguageRoute::default();
            if let Some(diarizer) = &mut diarizer {
                diarizer.reset();
            }
            match corpus_client.create_session().await {
                Ok(session) => {
                    let old_session = std::mem::replace(&mut corpus_session, session);
                    tokio::spawn(async move {
                        let _ = old_session.close().await;
                    });
                }
                Err(message) => {
                    let _ = events
                        .send(InferenceEvent::Error {
                            generation: job.generation,
                            message: message.to_string(),
                            configuration_required: false,
                        })
                        .await;
                    continue;
                }
            }
            diarizer_generation = Some(job.generation);
        }
        let overlap_frames = job.utterance.overlap_frames;
        let overlap_ratio = (overlap_frames.saturating_mul(FRAME_SAMPLES) as f32
            / job.utterance.samples.len().max(1) as f32)
            .clamp(0.0, 1.0);
        let (asr_tokens, translation_tokens) = inference.prompt_context_token_budgets();
        let context_budgets = ContextBudgets {
            asr_tokens,
            translation_tokens,
        };
        adaptive_route.configure(&job.source_language, &job.target_language);
        let active_target_language = adaptive_route.active_targets(&job.target_language);
        let asr_context = match corpus_session
            .prepare_asr(&PrepareAsrRequest {
                source_language: job.source_language.clone(),
                target_language: active_target_language,
                budgets: context_budgets,
            })
            .await
        {
            Ok(prompt) => prompt,
            Err(message) => {
                if events
                    .send(InferenceEvent::Error {
                        generation: job.generation,
                        message: message.to_string(),
                        configuration_required: false,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        let current_generation = *generation.borrow_and_update();
        if current_generation != job.generation {
            continue;
        }
        let asr_slot_started = Instant::now();
        let asr_permit = tokio::select! {
            permit = scheduler.acquire_asr(job.workload) => permit,
            changed = generation.changed() => {
                if changed.is_err() {
                    break;
                }
                continue;
            }
        };
        let asr_slot_wait = asr_slot_started.elapsed();
        if asr_slot_wait >= Duration::from_millis(25) {
            info!(
                wait_ms = asr_slot_wait.as_millis(),
                workload = ?job.workload,
                "ASR waited for a scheduled model slot"
            );
        }
        let prompt_graph_set = prompt_graphs.read().await.clone();
        let prompt_graph_for_revision = prompt_graph_set.graph;
        let current_generation = *generation.borrow_and_update();
        if current_generation != job.generation {
            drop(asr_permit);
            continue;
        }
        let recognized_result = tokio::select! {
            result = inference.transcribe(
                &job.utterance.samples,
                &job.source_language,
                &job.target_language,
                &mut adaptive_route,
                &prompt_graph_for_revision,
                AsrPromptContext {
                    vocabulary: asr_context.vocabulary.clone(),
                    mode: if job.revisable {
                        PromptMode::PseudoStreaming
                    } else {
                        PromptMode::Ordinary
                    },
                },
                &asr_context.echo_guard,
            ) => result,
            changed = generation.changed() => {
                drop(asr_permit);
                if changed.is_err() {
                    break;
                }
                continue;
            }
        };
        drop(asr_permit);
        let mut recognized = match recognized_result {
            Ok(Some(recognized)) => recognized,
            Ok(None) => {
                if events
                    .send(InferenceEvent::WindowObserved {
                        generation: job.generation,
                        text_units: 0,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            Err(failure) => {
                if events
                    .send(InferenceEvent::Error {
                        generation: job.generation,
                        message: failure.message,
                        configuration_required: failure.configuration_required,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        if !job.revisable
            && overlap_frames > 0
            && let Some((_, previous)) = &previous_transcript
            && !recognized.remove_overlap_with(previous)
        {
            info!(
                overlap_frames,
                "ASR overlap contained no new text; suppressing duplicate result"
            );
            if events
                .send(InferenceEvent::WindowObserved {
                    generation: job.generation,
                    text_units: 0,
                })
                .await
                .is_err()
            {
                break;
            }
            continue;
        }
        if job.revisable {
            let source_snapshot = match &mut active_revisable_transcript {
                Some((active_generation, active_turn, transcript))
                    if *active_generation == job.generation
                        && *active_turn == job.topic_turn_id =>
                {
                    transcript.update(&recognized.source_text, overlap_ratio)
                }
                _ => {
                    let transcript = RevisableTranscript::new(&recognized.source_text);
                    let snapshot = transcript.text();
                    active_revisable_transcript =
                        Some((job.generation, job.topic_turn_id.clone(), transcript));
                    snapshot
                }
            };
            recognized.apply_source_correction(source_snapshot);
            recognized.prepare_revisable_snapshot();
        }
        if *generation.borrow() != job.generation {
            continue;
        }
        if events
            .send(InferenceEvent::WindowObserved {
                generation: job.generation,
                text_units: text_density_units(&recognized.source_text),
            })
            .await
            .is_err()
        {
            break;
        }
        // Speaker identity is neutral recognition metadata and belongs to the
        // logical dialogue turn. Resolve it while the preceding translation
        // may still be running, then make it available to context selection.
        let speaker_enabled = speaker_recognition_enabled.load(Ordering::Acquire);
        let speaker_revision = speaker_state_revision.load(Ordering::Acquire);
        if speaker_revision != speaker_revision_seen {
            if let Some(diarizer) = &mut diarizer {
                diarizer.reset();
            }
            speaker_revision_seen = speaker_revision;
            speaker_load_failed = false;
        }
        let duration_ms = job.source_end_ms - job.source_start_ms;
        let speaker_id = if let Some(assigned) = job.speaker_id.clone() {
            assigned
        } else if speaker_enabled && duration_ms >= f64::from(speaker_min_utterance_ms) {
            if diarizer.is_none() && !speaker_load_failed {
                match tokio::task::block_in_place(|| inference.speaker_diarizer()) {
                    Ok(loaded) => diarizer = loaded,
                    Err(error) => {
                        speaker_load_failed = true;
                        warn!(%error, "speaker model failed to initialize; preserving ASR");
                    }
                }
            }
            match &mut diarizer {
                Some(diarizer) => {
                    match tokio::task::block_in_place(|| diarizer.identify(&job.utterance.samples))
                    {
                        Ok(assignment) => {
                            info!(
                                speaker_id = assignment.speaker_id,
                                similarity = assignment.similarity,
                                is_new = assignment.is_new,
                                "speaker voiceprint assigned"
                            );
                            assignment.speaker_id
                        }
                        Err(error) => {
                            warn!(%error, "speaker embedding failed; preserving ASR with unknown speaker");
                            "speaker-unknown".into()
                        }
                    }
                }
                None => "speaker-unknown".into(),
            }
        } else if speaker_enabled {
            "speaker-unknown".into()
        } else {
            String::new()
        };
        // ASR for this turn ran while the previous turn translated. Commit the
        // prior translation before selecting this turn's corpus context so
        // prompt history and emitted events retain strict stream order.
        if let Some(pending) = pending_translation.take() {
            let Ok(batch) = pending.await else { break };
            if !send_inference_batch(&events, batch).await {
                break;
            }
        }
        if *generation.borrow() != job.generation {
            continue;
        }
        let overlap_ms =
            overlap_frames as f64 * FRAME_SAMPLES as f64 * 1_000.0 / f64::from(SAMPLE_RATE_HZ);
        let non_overlapping_start_ms = if job.revisable {
            job.source_start_ms
        } else {
            (job.source_start_ms + overlap_ms).min(job.source_end_ms)
        };
        let boundary = segment_boundary(&job.utterance.end_reason);
        let stream_window = StreamWindowContext {
            start_ms: non_overlapping_start_ms,
            end_ms: job.source_end_ms,
            revisable: job.revisable,
            overlap_ratio,
            boundary,
            authoritative_snapshot: job.revisable,
            revision: job.revision,
        };
        let wire_turn_id = if job.revisable {
            job.topic_turn_id.clone()
        } else {
            job.turn_id.clone()
        };
        let translation_context = match corpus_session
            .prepare_translation(&PrepareTranslationRequest {
                asr_context_id: asr_context.context_id,
                turn_id: Some(job.topic_turn_id.clone()),
                speaker_id: speaker_id.clone(),
                source_language: recognized.source_language.clone(),
                target_language: recognized.target_language.clone(),
                recognized_text: recognized.source_text.clone(),
                segments: recognized
                    .segments
                    .iter()
                    .map(|segment| segment.translation_text.clone())
                    .collect(),
                budgets: context_budgets,
            })
            .await
        {
            Ok(context) => context,
            Err(message) => {
                let fallback_segments = segment_contexts(
                    &recognized.segments,
                    &[],
                    wire_turn_id.clone(),
                    speaker_id.clone(),
                    stream_window,
                );
                // Corpus is optional enrichment. Preserve successful ASR even
                // when context selection is temporarily unavailable.
                let _ = events
                    .send(InferenceEvent::Recognized {
                        generation: job.generation,
                        recognized,
                        segments: fallback_segments,
                        reference_samples: Some(job.utterance.samples.clone()),
                    })
                    .await;
                if events
                    .send(InferenceEvent::Error {
                        generation: job.generation,
                        message: message.to_string(),
                        configuration_required: false,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        let source_rewrite = rewrite_recognition_terms(
            &recognized.source_text,
            &translation_context.source_corrections,
        );
        if source_rewrite.corrected_text != recognized.source_text {
            info!(
                before = %recognized.source_text,
                after = %source_rewrite.corrected_text,
                "applied XR Corpus ASR terminology correction"
            );
            recognized.apply_source_correction(source_rewrite.corrected_text.clone());
        }
        if job.revisable {
            recognized.prepare_revisable_snapshot();
        }
        let translation_context_segments =
            align_translation_contexts(&recognized.segments, &translation_context.segments);
        for (segment, context) in recognized
            .segments
            .iter_mut()
            .zip(&translation_context_segments)
        {
            let rewrite =
                rewrite_recognition_terms(&segment.translation_text, &context.source_corrections);
            segment.translation_text = rewrite.corrected_text;
            segment.source_text.clone_from(&segment.translation_text);
        }
        previous_transcript = Some((job.generation, recognized.source_text.clone()));
        info!(
            asr_ms = recognized.asr_elapsed.as_millis(),
            segments = recognized.segments.len(),
            "ASR completed an utterance"
        );
        let asr_elapsed = recognized.asr_elapsed;
        let source_language = recognized.source_language.clone();
        let target_language = recognized.target_language.clone();
        let segments = recognized.segments.clone();
        let segment_contexts = segment_contexts(
            &segments,
            &translation_context_segments,
            wire_turn_id,
            speaker_id.clone(),
            stream_window,
        );
        if events
            .send(InferenceEvent::Recognized {
                generation: job.generation,
                recognized,
                segments: segment_contexts.clone(),
                reference_samples: Some(job.utterance.samples.clone()),
            })
            .await
            .is_err()
        {
            break;
        }
        let translation_generation = generation.clone();
        let event_generation = job.generation;
        let workload = job.workload;
        let turn_started_at = job.enqueued_at;
        let queue_elapsed = job_queue_elapsed;
        let context_id = translation_context.context_id;
        let corpus_session = corpus_session.clone();
        let inference = inference.clone();
        let scheduler = scheduler.clone();
        let logical_turn_id = job.topic_turn_id.clone();
        let history_speaker_id = speaker_id;
        let history_source_language = source_language.clone();
        let history_target_language = target_language.clone();
        // One immutable graph drives every provider page in this revision.
        let prompt_graph_for_turn = prompt_graph_for_revision;
        let prompt_mode_for_turn = if job.revisable {
            PromptMode::PseudoStreaming
        } else {
            PromptMode::Ordinary
        };
        let prompt_graph_fingerprint = prompt_graph_for_turn.fingerprint();
        let stable_translation_cache = Arc::clone(&stable_translation_cache);
        let cache_turn_id = logical_turn_id.clone();
        pending_translation = Some(tokio::spawn(async move {
            let translations = futures_util::stream::iter(
                segments
                    .into_iter()
                    .zip(segment_contexts)
                    .zip(translation_context_segments),
            )
            .map(|((segment, segment_context), corpus_context)| {
                let inference = inference.clone();
                let scheduler = scheduler.clone();
                let source_language = source_language.clone();
                let target_language = target_language.clone();
                let source_for_terms = segment.translation_text.clone();
                let prompt_terms = corpus_context.prompt_terms.clone();
                let prompt_graph = prompt_graph_for_turn.clone();
                let stable_translation_cache = Arc::clone(&stable_translation_cache);
                let cache_turn_id = cache_turn_id.clone();
                let prompt_graph_fingerprint = prompt_graph_fingerprint.clone();
                async move {
                    let _permit = scheduler.acquire_translation(workload).await;
                    let mut prompt_context = prompt_context_for_segment(
                        &source_language,
                        &target_language,
                        &corpus_context,
                    );
                    prompt_context.mode = prompt_mode_for_turn;
                    let cache_key = format!(
                        "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{:?}\x1f{:?}\x1f{:?}\x1f{:?}",
                        cache_turn_id,
                        source_for_terms,
                        source_language,
                        target_language,
                        prompt_graph_fingerprint,
                        prompt_context.language_order,
                        prompt_context.terminology_rows,
                        prompt_context.recent_turns,
                        prompt_context.mode,
                    );
                    let cached = if prompt_mode_for_turn == PromptMode::PseudoStreaming
                        && !segment_context.revisable
                    {
                        stable_translation_cache
                            .lock()
                            .await
                            .get(&cache_key)
                            .cloned()
                    } else {
                        None
                    };
                    let output = if let Some(output) = cached {
                        Ok(output)
                    } else {
                        let output = inference
                            .translate_segment(
                                &segment,
                                &source_language,
                                &target_language,
                                prompt_graph,
                                prompt_context,
                            )
                            .await;
                        if prompt_mode_for_turn == PromptMode::PseudoStreaming
                            && !segment_context.revisable
                        {
                            if let Ok(translated) = &output {
                                let mut cache = stable_translation_cache.lock().await;
                                if cache.len() >= 128 {
                                    if let Some(oldest) = cache.keys().next().cloned() {
                                        cache.remove(&oldest);
                                    }
                                }
                                cache.insert(cache_key, translated.clone());
                            }
                        }
                        output
                    };
                    (segment_context, source_for_terms, prompt_terms, output)
                }
            })
            .buffered(TRANSLATION_CONCURRENCY_PER_SESSION);
            tokio::pin!(translations);
            let mut batch = Vec::new();
            let mut completed_pairs = Vec::new();
            let mut cancelled = false;
            while let Some((segment_context, source_for_terms, prompt_terms, mut output)) =
                translations.next().await
            {
                if *translation_generation.borrow() != event_generation {
                    cancelled = true;
                    break;
                }
                if let Ok(translated) = &mut output {
                    let rewrite = rewrite_translation_terms(
                        &source_for_terms,
                        &translated.translated_text,
                        &translated.target_language,
                        &prompt_terms,
                    );
                    translated.translated_text = rewrite.translated_text;
                    translated.term_matches = rewrite.term_matches;
                    completed_pairs.push((
                        translated.source_text.clone(),
                        translated.translated_text.clone(),
                    ));
                }
                batch.push(InferenceEvent::Translation {
                    generation: event_generation,
                    target_language: target_language.clone(),
                    queue_elapsed,
                    asr_elapsed,
                    total_elapsed: turn_started_at.elapsed(),
                    context: segment_context,
                    output,
                });
            }
            if !cancelled
                && let Some(request) = (LogicalTurnRecord {
                    context_id,
                    turn_id: logical_turn_id,
                    speaker_id: history_speaker_id,
                    source_language: &history_source_language,
                    target_language: &history_target_language,
                    completed_pairs: &completed_pairs,
                })
                .into_request()
            {
                if let Err(message) = corpus_session.record_translation(&request).await {
                    warn!(%message, "could not record XR Corpus translation context");
                }
            }
            batch
        }));
    }
}

async fn send_inference_batch(
    events: &mpsc::Sender<InferenceEvent>,
    batch: Vec<InferenceEvent>,
) -> bool {
    for event in batch {
        if events.send(event).await.is_err() {
            return false;
        }
    }
    true
}

async fn handle_inference_event(
    writer: &mpsc::Sender<OutboundMessage>,
    session: &mut SessionAdapter,
    current_generation: PipelineGeneration,
    event: InferenceEvent,
    tts: Option<&NativeTtsAdapter>,
    tts_jobs: Option<&mpsc::Sender<TtsSynthesisJob>>,
    voice_name: &str,
    voice_ready: bool,
    max_input_chars: usize,
) -> Result<bool, axum::Error> {
    if event.generation() != current_generation {
        return Ok(false);
    }
    let mut tts_queued = false;
    match event {
        InferenceEvent::WindowObserved { .. } => {}
        InferenceEvent::Recognized {
            generation,
            recognized,
            segments,
            reference_samples: _,
        } => {
            if let Some(target_lang) = &recognized.route_switched {
                send_event(
                    writer,
                    Some(generation),
                    ServerEvent::RouteChanged(RouteChanged {
                        source_lang: "auto".to_string(),
                        target_lang: target_lang.clone(),
                    }),
                )
                .await?;
            }
            let turn_id = segments
                .first()
                .map(|context| context.turn_id.clone())
                .unwrap_or_else(|| session.turn_id());
            let asr_prompt_trace = recognized.prompt_trace.clone();
            if !session
                .submit_recognized_for_route_and_turn(
                    generation.route_epoch,
                    recognized.source_text,
                    true,
                    turn_id,
                )
                .map_err(axum::Error::new)?
            {
                return Ok(false);
            }
            send_session_output(writer, session, generation).await?;
            for (segment, context) in recognized.segments.into_iter().zip(segments) {
                send_event(
                    writer,
                    Some(generation),
                    session.source_segment_ready_for_turn(
                        segment.source_text,
                        context,
                        asr_prompt_trace.clone(),
                    ),
                )
                .await?;
            }
        }
        InferenceEvent::Translation {
            generation,
            target_language,
            queue_elapsed,
            asr_elapsed,
            total_elapsed,
            context,
            output,
        } => {
            let output = match output {
                Ok(output) => output,
                Err(error) if generation.route_epoch == session.route_epoch() => {
                    send_scoped_error(
                        writer,
                        generation,
                        error.message,
                        error.configuration_required,
                    )
                    .await?;
                    return Ok(false);
                }
                Err(_) => return Ok(false),
            };
            let tts_text = output.translated_text.clone();
            let tts_revisable = context.revisable;
            if session
                .submit_translation_segment_for_route_and_turn(
                    generation.route_epoch,
                    output.source_text,
                    output.translated_text,
                    output.term_matches,
                    output.prompt_trace,
                    LatencyMetrics {
                        queue_ms: millis(queue_elapsed),
                        asr_ms: millis(asr_elapsed),
                        mt_ms: millis(output.mt_elapsed),
                        tts_ms: 0,
                        total_ms: millis(total_elapsed),
                    },
                    context,
                )
                .map_err(axum::Error::new)?
            {
                send_session_output(writer, session, generation).await?;
                if session.tts_enabled()
                    && voice_ready
                    && !tts_revisable
                    && tts.is_some_and(|adapter| adapter.supports_language(&target_language))
                    && let Some(tts_jobs) = tts_jobs
                {
                    let text_chunks = split_tts_text(&tts_text, max_input_chars);
                    if !text_chunks.is_empty() {
                        info!(
                            generation = ?generation,
                            voice = voice_name,
                            input_chars = tts_text.chars().count(),
                            chunk_count = text_chunks.len(),
                            "TTS synthesis queued"
                        );
                        tts_jobs
                            .send(TtsSynthesisJob {
                                generation,
                                tts_epoch: session.tts_epoch(),
                                text_chunks,
                                voice_name: voice_name.to_owned(),
                                target_language,
                            })
                            .await
                            .map_err(axum::Error::new)?;
                        tts_queued = true;
                    }
                }
            }
        }
        InferenceEvent::StreamEnded { turn_id, .. } => {
            send_event(
                writer,
                Some(current_generation),
                ServerEvent::RecognitionStreamEnded(RecognitionStreamEnded { turn_id }),
            )
            .await?;
        }
        InferenceEvent::Drained { .. } => {}
        InferenceEvent::Error {
            generation,
            message,
            configuration_required,
        } if generation.route_epoch == session.route_epoch() => {
            send_scoped_error(writer, generation, message, configuration_required).await?
        }
        InferenceEvent::Error { .. } => {}
    }
    Ok(tts_queued)
}

fn text_density_units(text: &str) -> usize {
    let mut units = 0;
    let mut in_word = false;
    for character in text.chars() {
        if matches!(
            character as u32,
            0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
        ) {
            units += 1;
            in_word = false;
        } else if character.is_alphanumeric() || character == '\'' {
            if !in_word {
                units += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    units
}

/// Keeps post-correction segmentation total with the corpus response.
///
/// XR Corpus selects context against the ASR segmentation that was submitted
/// with the request. A terminology correction can change sentence boundaries
/// before the authoritative snapshot is emitted. Never let a plain `zip`
/// silently drop a newly created segment; retain positional context where it
/// still exists and give unmatched segments an explicit empty context.
fn align_translation_contexts(
    segments: &[xrtranslate_engine::TranslationSegmentPair],
    contexts: &[CorpusSegmentContext],
) -> Vec<CorpusSegmentContext> {
    if segments.len() == contexts.len() {
        return contexts.to_vec();
    }
    warn!(
        segment_count = segments.len(),
        context_count = contexts.len(),
        "translation context count changed after source correction; preserving every segment"
    );
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            contexts
                .get(index)
                .cloned()
                .unwrap_or_else(|| CorpusSegmentContext {
                    corrected_text: segment.translation_text.clone(),
                    prompt_terms: Vec::new(),
                    context_data: Default::default(),
                    source_corrections: Vec::new(),
                    activation_matches: Vec::new(),
                    context_matches: Vec::new(),
                })
        })
        .collect()
}

fn segment_contexts(
    segments: &[xrtranslate_engine::TranslationSegmentPair],
    corpus_contexts: &[CorpusSegmentContext],
    turn_id: String,
    speaker_id: String,
    window: StreamWindowContext,
) -> Vec<SegmentContext> {
    let timing = if segments.len() == 1 {
        SegmentTiming::UtteranceWindow
    } else {
        SegmentTiming::EstimatedTextPartition
    };
    let weights = segments
        .iter()
        // Latin text duration tracks spoken words better than punctuation and
        // byte/character length, while CJK text naturally uses one unit per
        // ideograph or kana. This is still an estimate and is labelled as such
        // in SegmentTiming rather than pretending to be word alignment.
        .map(|segment| text_density_units(&segment.source_text).max(1))
        .collect::<Vec<_>>();
    let total_weight = weights.iter().sum::<usize>().max(1) as f64;
    let duration = (window.end_ms - window.start_ms).max(0.0);
    let mut consumed = 0usize;
    weights
        .into_iter()
        .enumerate()
        .map(|(index, weight)| {
            let start = window.start_ms + duration * consumed as f64 / total_weight;
            consumed += weight;
            let end = if index + 1 == segments.len() {
                window.end_ms
            } else {
                window.start_ms + duration * consumed as f64 / total_weight
            };
            let corpus_context = corpus_contexts.get(index);
            SegmentContext {
                turn_id: turn_id.clone(),
                segment_index: u32::try_from(index + 1).unwrap_or(u32::MAX),
                segment_count: u32::try_from(segments.len()).unwrap_or(u32::MAX),
                speaker_id: speaker_id.clone(),
                source_start_ms: start,
                source_end_ms: end,
                timing,
                boundary: window.boundary,
                // Only the final segment is still mutable. Earlier sentence
                // boundaries are committed subtitle/meeting units while the
                // whole list remains one atomic snapshot for consumers.
                revisable: window.revisable && index + 1 == segments.len(),
                overlap_ratio: window.overlap_ratio,
                authoritative_snapshot: window.authoritative_snapshot,
                revision: window.revision,
                activation_matches: corpus_context
                    .map(|context| context.activation_matches.clone())
                    .unwrap_or_default(),
                context_matches: corpus_context.map_or_else(Vec::new, |context| {
                    let mut matches = context.context_matches.clone();
                    let rewrite = rewrite_recognition_terms(
                        &context.corrected_text,
                        &context.source_corrections,
                    );
                    matches.extend(rewrite.term_matches);
                    matches
                }),
            }
        })
        .collect()
}

fn segment_boundary(reason: &UtteranceEndReason) -> SegmentBoundary {
    match reason {
        UtteranceEndReason::Silence => SegmentBoundary::Silence,
        UtteranceEndReason::AdaptiveSilence => SegmentBoundary::AdaptiveSilence,
        UtteranceEndReason::MaxActiveFrames => SegmentBoundary::DurationLimit,
        UtteranceEndReason::SpeakerChange => SegmentBoundary::SpeakerChange,
        UtteranceEndReason::Flushed => SegmentBoundary::InputBoundary,
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

async fn send_session_output(
    writer: &mpsc::Sender<OutboundMessage>,
    session: &mut SessionAdapter,
    generation: PipelineGeneration,
) -> Result<(), axum::Error> {
    debug_assert_eq!(generation.route_epoch, session.route_epoch());
    for output in session.drain_wire_output() {
        match output {
            WireOutput::Event(event) => send_event(writer, Some(generation), event).await?,
            WireOutput::Pcm(pcm) => writer
                .send(OutboundMessage::current(
                    generation,
                    Message::Binary(pcm.into()),
                ))
                .await
                .map_err(axum::Error::new)?,
        }
    }
    Ok(())
}

async fn send_error(
    writer: &mpsc::Sender<OutboundMessage>,
    message: String,
) -> Result<(), axum::Error> {
    send_event(
        writer,
        None,
        ServerEvent::Error(ErrorEvent {
            message,
            configuration_required: false,
        }),
    )
    .await
}

async fn send_scoped_error(
    writer: &mpsc::Sender<OutboundMessage>,
    generation: PipelineGeneration,
    message: String,
    configuration_required: bool,
) -> Result<(), axum::Error> {
    send_event(
        writer,
        Some(generation),
        ServerEvent::Error(ErrorEvent {
            message,
            configuration_required,
        }),
    )
    .await
}

async fn send_event(
    writer: &mpsc::Sender<OutboundMessage>,
    generation: Option<PipelineGeneration>,
    event: ServerEvent,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(&event).expect("protocol DTO must serialize");
    writer
        .send(OutboundMessage {
            generation,
            message: Message::Text(json.into()),
        })
        .await
        .map_err(axum::Error::new)
}

/// The only task allowed to write to this session's WebSocket sink.
async fn run_websocket_writer(
    mut writer: futures_util::stream::SplitSink<WebSocket, Message>,
    mut messages: mpsc::Receiver<OutboundMessage>,
    mut generation: watch::Receiver<PipelineGeneration>,
) {
    while let Some(outbound) = messages.recv().await {
        if !outbound_is_current(outbound.generation, *generation.borrow_and_update()) {
            continue;
        }
        if writer.send(outbound.message).await.is_err() {
            break;
        }
    }
}

fn outbound_is_current(
    event_generation: Option<PipelineGeneration>,
    current_generation: PipelineGeneration,
) -> bool {
    event_generation.is_none_or(|event_generation| event_generation == current_generation)
}

async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("shutdown signal received"),
        Err(error) => {
            warn!(%error, "Ctrl+C listener is unavailable; keeping backend running");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioEpoch, PipelineGeneration, SessionInputState, StreamWindowContext, VoiceCloneCapture,
        align_translation_contexts, health_url, local_endpoint_port, model_alias_is_advertised,
        models_url, outbound_is_current, segment_contexts, split_tts_text,
    };
    use xrtranslate_engine::{
        EngineConfig, Language, LanguageRoute, SessionEngine, TranslationSegmentPair,
    };
    use xrtranslate_prompt::PromptNodeGraph;
    use xrtranslate_protocol::{PromptGraphSet, SegmentBoundary, SegmentTiming};

    #[test]
    fn every_mode_uses_the_same_graph_snapshot() {
        let graphs = PromptGraphSet {
            graph: PromptNodeGraph::builtin_default(),
        };
        graphs.graph.validate_for_activation().unwrap();
    }

    #[test]
    fn source_correction_context_alignment_never_drops_new_segments() {
        let segments = vec![
            TranslationSegmentPair {
                source_text: "First.".into(),
                translation_text: "First.".into(),
            },
            TranslationSegmentPair {
                source_text: "Second.".into(),
                translation_text: "Second.".into(),
            },
        ];
        let aligned = align_translation_contexts(&segments, &[]);
        assert_eq!(aligned.len(), segments.len());
        assert_eq!(aligned[0].corrected_text, "First.");
        assert_eq!(aligned[1].corrected_text, "Second.");
    }

    #[test]
    fn each_voice_clone_attempt_starts_with_an_empty_bounded_capture() {
        let mut capture = VoiceCloneCapture {
            armed: false,
            ready: true,
            samples: vec![1; 32_000],
            transcript: vec!["old recording".into()],
            minimum_samples: 8_000,
            maximum_samples: 480_000,
        };
        capture.arm();
        assert!(capture.armed);
        assert!(capture.samples.is_empty());
        assert!(capture.transcript.is_empty());
    }

    #[test]
    fn tts_text_chunks_are_unicode_safe_and_provider_bounded() {
        let chunks = split_tts_text("你好，世界。这是一段语音。", 6);
        assert_eq!(chunks.concat(), "你好，世界。这是一段语音。");
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 6));
    }

    #[test]
    fn paused_sessions_keep_controls_but_reject_binary_audio() {
        assert!(SessionInputState::Running.accepts_audio());
        assert!(SessionInputState::Running.accepts_controls());
        assert!(!SessionInputState::Paused.accepts_audio());
        assert!(SessionInputState::Paused.accepts_controls());
        assert!(!SessionInputState::Draining.accepts_audio());
        assert!(!SessionInputState::Draining.accepts_controls());
    }

    #[test]
    fn managed_model_urls_must_be_local_and_have_distinct_explicit_ports() {
        assert_eq!(
            local_endpoint_port("http://127.0.0.1:8001/v1/chat/completions").unwrap(),
            8001
        );
        assert!(local_endpoint_port("https://example.com:8001/v1/chat/completions").is_err());
        assert!(local_endpoint_port("http://localhost/v1/chat/completions").is_err());
    }

    #[test]
    fn health_url_keeps_the_local_port_and_replaces_unspecified_bind_address() {
        assert_eq!(
            health_url("http://0.0.0.0:8002/v1/chat/completions").unwrap(),
            "http://127.0.0.1:8002/health"
        );
    }

    #[test]
    fn models_url_uses_the_local_openai_models_endpoint() {
        assert_eq!(
            models_url("http://0.0.0.0:8001/v1/chat/completions").unwrap(),
            "http://127.0.0.1:8001/v1/models"
        );
    }

    #[test]
    fn readiness_requires_the_expected_model_alias_not_just_a_healthy_server() {
        let ready = r#"{"object":"list","data":[{"id":"hy-mt2"}]}"#;
        assert!(model_alias_is_advertised(ready, "hy-mt2").is_ok());
        assert!(model_alias_is_advertised(ready, "qwen3-asr").is_err());
        assert!(model_alias_is_advertised("{}", "hy-mt2").is_err());
    }

    #[test]
    fn writer_drops_queued_events_for_an_old_route_but_keeps_controls() {
        let mut engine = SessionEngine::new(
            LanguageRoute::new(Language::new("en").unwrap(), Language::new("zh").unwrap()),
            EngineConfig::default(),
        );
        let initial = PipelineGeneration {
            route_epoch: engine.route_epoch(),
            audio_epoch: AudioEpoch::INITIAL,
        };
        let current_route = engine.set_route(LanguageRoute::new(
            Language::new("ja").unwrap(),
            Language::new("en").unwrap(),
        ));
        let current = PipelineGeneration {
            route_epoch: current_route,
            audio_epoch: initial.audio_epoch,
        };
        assert!(outbound_is_current(Some(initial), initial));
        assert!(!outbound_is_current(Some(initial), current));
        assert!(outbound_is_current(None, current));
    }

    #[test]
    fn writer_drops_queued_events_after_audio_reset_with_the_same_route() {
        let route_epoch = SessionEngine::new(
            LanguageRoute::new(Language::new("en").unwrap(), Language::new("zh").unwrap()),
            EngineConfig::default(),
        )
        .route_epoch();
        let before_reset = PipelineGeneration {
            route_epoch,
            audio_epoch: AudioEpoch::INITIAL,
        };
        let mut audio_epoch = before_reset.audio_epoch;
        audio_epoch.advance();
        let after_reset = PipelineGeneration {
            route_epoch,
            audio_epoch,
        };

        assert!(!outbound_is_current(Some(before_reset), after_reset));
        assert!(outbound_is_current(Some(after_reset), after_reset));
        assert!(outbound_is_current(None, after_reset));
    }

    #[test]
    fn source_timeline_is_monotonic_and_shared_with_one_speaker() {
        let segments = vec![
            TranslationSegmentPair {
                source_text: "Hi.".into(),
                translation_text: "Hi.".into(),
            },
            TranslationSegmentPair {
                source_text: "This is longer.".into(),
                translation_text: "This is longer.".into(),
            },
        ];
        let metadata = segment_contexts(
            &segments,
            &[],
            "turn-7".into(),
            "speaker-02".into(),
            StreamWindowContext {
                start_ms: 1_000.0,
                end_ms: 3_000.0,
                revisable: false,
                overlap_ratio: 0.0,
                boundary: SegmentBoundary::Silence,
                authoritative_snapshot: false,
                revision: 0,
            },
        );
        assert_eq!(metadata.len(), 2);
        assert_eq!(metadata[0].speaker_id, "speaker-02");
        assert_eq!(metadata[0].turn_id, "turn-7");
        assert_eq!(metadata[0].segment_index, 1);
        assert_eq!(metadata[1].segment_index, 2);
        assert_eq!(metadata[1].segment_count, 2);
        assert_eq!(metadata[0].source_start_ms, 1_000.0);
        assert_eq!(metadata[0].source_end_ms, metadata[1].source_start_ms);
        assert_eq!(metadata[1].source_end_ms, 3_000.0);
        assert_eq!(metadata[0].timing, SegmentTiming::EstimatedTextPartition);
        assert_eq!(metadata[0].boundary, SegmentBoundary::Silence);
        assert!(metadata[0].source_end_ms < 2_000.0);
    }
}
