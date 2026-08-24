use crossbeam_channel::{Receiver, Sender};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, Message},
};
use xrtranslate_prompt::PromptExecutionTrace;
use xrtranslate_protocol::{
    CorpusTermMatch, DrainReason, InferenceWorkload, PromptGraphSet, SegmentBoundary,
    SegmentTiming, ServerEvent as ProtocolServerEvent,
};

use crate::client_settings::CaptureSource;

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

/// A host-provided gate that can suppress outbound audio without coupling the
/// translation session to the plugin that owns the external state.
#[derive(Clone, Debug)]
pub struct ExternalAudioGate {
    pub active: Arc<AtomicBool>,
    pub enabled: Arc<AtomicBool>,
}

impl ExternalAudioGate {
    pub fn new(active: Arc<AtomicBool>, enabled: Arc<AtomicBool>) -> Self {
        Self { active, enabled }
    }

    fn blocks_audio(&self) -> bool {
        self.enabled.load(Ordering::Acquire) && self.active.load(Ordering::Acquire)
    }
}

impl Default for ExternalAudioGate {
    fn default() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected,
    Disconnected(String),
    Status(String),
    VadActivity {
        source: CaptureSource,
        active: bool,
    },
    Asr {
        stream_id: u64,
        continuous: bool,
        publish_to_host_outputs: bool,
        kind: String,
        text: String,
        turn_id: String,
    },
    SourceSegment {
        stream_id: u64,
        audio_source: CaptureSource,
        continuous: bool,
        publish_to_host_outputs: bool,
        text: String,
        prompt_trace: Option<PromptExecutionTrace>,
        activation_matches: Vec<CorpusTermMatch>,
        context_matches: Vec<CorpusTermMatch>,
        turn_id: String,
        speaker_id: String,
        source_start_ms: f64,
        source_end_ms: f64,
        timing: SegmentTiming,
        boundary: SegmentBoundary,
        segment_index: u32,
        segment_count: u32,
        revisable: bool,
        overlap_ratio: f32,
        authoritative_snapshot: bool,
        revision: u64,
    },
    Translation {
        stream_id: u64,
        audio_source: CaptureSource,
        continuous: bool,
        publish_to_host_outputs: bool,
        source: String,
        translated: String,
        turn_id: String,
        segment_index: u32,
        #[allow(dead_code)]
        segment_count: u32,
        speaker_id: String,
        source_start_ms: f64,
        source_end_ms: f64,
        timing: SegmentTiming,
        boundary: SegmentBoundary,
        term_matches: Vec<CorpusTermMatch>,
        prompt_trace: Option<PromptExecutionTrace>,
        revisable: bool,
        overlap_ratio: f32,
        authoritative_snapshot: bool,
        revision: u64,
    },
    StreamEnded {
        stream_id: u64,
        publish_to_host_outputs: bool,
    },
    RouteChanged {
        source_lang: String,
        target_lang: String,
    },
    TtsRuntime {
        backend: String,
        cuda_version: Option<String>,
    },
    TtsAudio(Vec<u8>),
    VoiceCloneState {
        source: CaptureSource,
        status: xrtranslate_protocol::VoiceCloneState,
    },
    BackendError {
        message: String,
        configuration_required: bool,
    },
    Error(String),
}

pub struct SessionHandle {
    stop_requested: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    command_tx: mpsc::Sender<SessionCommand>,
}

enum SessionCommand {
    UpdateLanguageRoute {
        source_lang: String,
        target_lang: String,
    },
    ResetAudioPipeline {
        source_lang: String,
        target_lang: String,
        audio_source: CaptureSource,
        vad_threshold: f32,
        vad_silence_ms: u32,
        continuous_recognition: bool,
    },
    UpdateAudioSegmentation {
        vad_threshold: f32,
        vad_silence_ms: u32,
        continuous_recognition: bool,
        source_lang: String,
        target_lang: String,
    },
    SetTtsEnabled(bool),
    BeginVoiceClone,
    UpdatePromptTemplates {
        graphs: PromptGraphSet,
    },
    TranslateText {
        text: String,
        source_lang: Option<String>,
        target_lang: Option<String>,
    },
    Pause,
    Resume,
    Finish,
    Cancel,
}

impl SessionCommand {
    fn continuous_recognition(&self) -> Option<bool> {
        match self {
            Self::UpdateAudioSegmentation {
                continuous_recognition,
                ..
            }
            | Self::ResetAudioPipeline {
                continuous_recognition,
                ..
            } => Some(*continuous_recognition),
            _ => None,
        }
    }

    fn resets_recognition_stream(&self) -> bool {
        matches!(
            self,
            Self::UpdateLanguageRoute { .. }
                | Self::ResetAudioPipeline { .. }
                | Self::UpdateAudioSegmentation { .. }
        )
    }

    fn audio_source(&self) -> Option<CaptureSource> {
        match self {
            Self::ResetAudioPipeline { audio_source, .. } => Some(*audio_source),
            _ => None,
        }
    }
}

impl SessionHandle {
    /// Immediately cancels pending inference. Finite inputs use [`Self::finish`]
    /// at EOF when their complete ordered output is required.
    pub fn stop(&self) {
        self.cancel();
    }

    /// Flushes the current VAD turn while retaining the live WebSocket,
    /// timeline, and backend speaker state.
    pub fn pause(&self) {
        if self.stop_requested.load(Ordering::Acquire) {
            return;
        }
        self.paused.store(true, Ordering::Release);
        if self.command_tx.try_send(SessionCommand::Pause).is_err() {
            self.paused.store(false, Ordering::Release);
        }
    }

    pub fn resume(&self) {
        if self.stop_requested.load(Ordering::Acquire) {
            return;
        }
        let _ = self.command_tx.try_send(SessionCommand::Resume);
    }

    /// Ends audio input and waits for the backend's ordered drain
    /// acknowledgement before closing the WebSocket.
    pub fn finish(&self) {
        if self.stop_requested.swap(true, Ordering::AcqRel) {
            return;
        }
        self.paused.store(true, Ordering::Release);
        let _ = self.command_tx.try_send(SessionCommand::Finish);
    }

    pub fn cancel(&self) {
        if self.stop_requested.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel_requested.store(true, Ordering::Release);
        self.paused.store(true, Ordering::Release);
        let _ = self.command_tx.try_send(SessionCommand::Cancel);
    }

    pub fn update_language_route(&self, source_lang: String, target_lang: String) {
        let _ = self
            .command_tx
            .try_send(SessionCommand::UpdateLanguageRoute {
                source_lang,
                target_lang,
            });
    }

    pub fn set_tts_enabled(&self, enabled: bool) {
        let _ = self
            .command_tx
            .try_send(SessionCommand::SetTtsEnabled(enabled));
    }

    pub fn begin_voice_clone(&self) {
        let _ = self.command_tx.try_send(SessionCommand::BeginVoiceClone);
    }

    pub fn update_prompt_templates(&self, graphs: PromptGraphSet) {
        let _ = self
            .command_tx
            .try_send(SessionCommand::UpdatePromptTemplates { graphs });
    }

    /// Submits a direct text turn to the standard translation pipeline.
    pub fn translate_text(
        &self,
        text: impl Into<String>,
        source_lang: Option<String>,
        target_lang: Option<String>,
    ) {
        if self.stop_requested.load(Ordering::Acquire) {
            return;
        }
        let _ = self.command_tx.try_send(SessionCommand::TranslateText {
            text: text.into(),
            source_lang,
            target_lang,
        });
    }

    /// Reconfigure the backend audio stream after replacing the local capture
    /// source. This clears any partially accumulated VAD utterance.
    pub fn reset_audio_pipeline(
        &self,
        source_lang: String,
        target_lang: String,
        audio_source: CaptureSource,
        vad_threshold: f32,
        vad_silence_ms: u32,
        continuous_recognition: bool,
    ) {
        let _ = self
            .command_tx
            .try_send(SessionCommand::ResetAudioPipeline {
                source_lang,
                target_lang,
                audio_source,
                vad_threshold,
                vad_silence_ms,
                continuous_recognition,
            });
    }

    pub fn update_audio_segmentation(
        &self,
        vad_threshold: f32,
        vad_silence_ms: u32,
        continuous_recognition: bool,
        source_lang: String,
        target_lang: String,
    ) {
        let _ = self
            .command_tx
            .try_send(SessionCommand::UpdateAudioSegmentation {
                vad_threshold,
                vad_silence_ms,
                continuous_recognition,
                source_lang,
                target_lang,
            });
    }
}

pub struct SessionConfig {
    pub server_url: String,
    pub source_lang: String,
    pub target_lang: String,
    pub external_audio_gate: ExternalAudioGate,
    /// Controls presentation in the host translation UI and external caption
    /// plugins. Domain plugins still receive the typed segment stream.
    pub publish_to_host_outputs: bool,
    pub tts: Option<crate::audio::TtsPlayerHandle>,
    pub egui_ctx: Option<eframe::egui::Context>,
    pub vad_threshold: f32,
    pub vad_silence_ms: u32,
    pub continuous_recognition: bool,
    pub audio_source: CaptureSource,
    /// Finish only after every locally queued audio frame has reached the
    /// websocket writer. This is required for file import and graceful live
    /// capture shutdown, where a producer disconnect marks end-of-input.
    pub finish_when_audio_ends: bool,
    pub prompt_graphs: Option<PromptGraphSet>,
}

pub fn start_session(
    audio_rx: Receiver<Vec<f32>>,
    event_tx: Sender<SessionEvent>,
    config: SessionConfig,
) -> SessionHandle {
    let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
    let stop_requested = Arc::new(AtomicBool::new(false));
    let runtime_stop = Arc::clone(&stop_requested);
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let runtime_cancel = Arc::clone(&cancel_requested);
    let paused = Arc::new(AtomicBool::new(false));
    let runtime_paused = Arc::clone(&paused);
    // Bound pending configuration updates.
    let (command_tx, command_rx) = mpsc::channel(16);
    thread::Builder::new()
        .name("translation-session".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = event_tx.send(SessionEvent::Error(format!(
                        "Failed to start network runtime: {error}"
                    )));
                    return;
                }
            };
            runtime.block_on(run_session(
                audio_rx,
                event_tx,
                config,
                runtime_stop,
                runtime_cancel,
                runtime_paused,
                command_rx,
                stream_id,
            ));
        })
        .expect("failed to start translation session thread");

    SessionHandle {
        stop_requested,
        cancel_requested,
        paused,
        command_tx,
    }
}

async fn run_session(
    audio_rx: Receiver<Vec<f32>>,
    event_tx: Sender<SessionEvent>,
    config: SessionConfig,
    stop_requested: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    mut command_rx: mpsc::Receiver<SessionCommand>,
    stream_id: u64,
) {
    let SessionConfig {
        server_url,
        source_lang,
        target_lang,
        external_audio_gate,
        publish_to_host_outputs,
        tts: tts_handle,
        egui_ctx,
        vad_threshold,
        vad_silence_ms,
        mut continuous_recognition,
        mut audio_source,
        finish_when_audio_ends,
        prompt_graphs,
    } = config;
    let workload = if finish_when_audio_ends {
        InferenceWorkload::Offline
    } else {
        InferenceWorkload::Realtime
    };
    let _ = event_tx.send(SessionEvent::Status("Connecting to backend…".into()));
    let (stream, _) = match connect_async(&server_url).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = event_tx.send(SessionEvent::Error(format!(
                "Cannot connect to {server_url}: {error}"
            )));
            return;
        }
    };
    let (mut write, mut read) = stream.split();
    if let Err(error) = send_json(
        &mut write,
        json!({
            "action": "session_config",
            "source_lang": source_lang,
            "target_lang": target_lang,
            "sample_rate": 16_000,
            "prompt_graphs": prompt_graphs,
            "vad_threshold": vad_threshold,
            "vad_silence_ms": vad_silence_ms,
            "continuous_recognition": continuous_recognition,
        }),
    )
    .await
    {
        let _ = event_tx.send(SessionEvent::Error(format!(
            "Cannot configure session: {error}"
        )));
        return;
    }
    if let Err(error) = send_json(
        &mut write,
        json!({
            "action": "toggle_feature",
            "feature": "speaker_recognition",
            "enabled": true,
        }),
    )
    .await
    {
        let _ = event_tx.send(SessionEvent::Error(format!(
            "Cannot configure speaker recognition: {error}"
        )));
        return;
    }
    if let Err(error) = send_json(
        &mut write,
        json!({
            "event": "config_audio",
            "sample_rate": 16_000,
            "source_lang": source_lang,
            "target_lang": target_lang,
            "vad_threshold": vad_threshold,
            "vad_silence_ms": vad_silence_ms,
            "continuous_recognition": continuous_recognition,
            "audio_source": audio_source_name(audio_source),
            "workload": workload,
        }),
    )
    .await
    {
        let _ = event_tx.send(SessionEvent::Error(format!(
            "Cannot configure audio: {error}"
        )));
        return;
    }
    let _ = event_tx.send(SessionEvent::Connected);

    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<u8>>(32);
    let producer_stop = Arc::clone(&stop_requested);
    let producer_paused = Arc::clone(&paused);
    thread::spawn(move || {
        while !producer_stop.load(Ordering::Acquire) {
            match audio_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(samples) => {
                    if producer_paused.load(Ordering::Acquire) {
                        continue;
                    }
                    let pcm = f32_to_pcm16le(samples);
                    if pcm_tx.blocking_send(pcm).is_err() {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut turn_started = false;
    let mut finish_sent = false;
    let mut pcm_input_closed = false;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if cancel_requested.load(Ordering::Acquire) {
                    let _ = write.close().await;
                    let _ = event_tx.send(SessionEvent::StreamEnded {
                        stream_id,
                        publish_to_host_outputs,
                    });
                    let _ = event_tx.send(SessionEvent::Disconnected("Cancelled".into()));
                    return;
                }
                if stop_requested.load(Ordering::Acquire) {
                    if !finish_sent {
                        if let Err(error) = send_json(&mut write, json!({"event": "finish"})).await {
                            let _ = event_tx.send(SessionEvent::Error(format!("Failed to finish session: {error}")));
                            break;
                        }
                        finish_sent = true;
                    }
                }
            }
            Some(command) = command_rx.recv() => {
                if matches!(command, SessionCommand::Cancel) {
                    let _ = write.close().await;
                    let _ = event_tx.send(SessionEvent::StreamEnded {
                        stream_id,
                        publish_to_host_outputs,
                    });
                    let _ = event_tx.send(SessionEvent::Disconnected("Cancelled".into()));
                    return;
                }
                let is_finish = matches!(&command, SessionCommand::Finish);
                let is_resume = matches!(&command, SessionCommand::Resume);
                if command.resets_recognition_stream() {
                    let _ = event_tx.send(SessionEvent::StreamEnded {
                        stream_id,
                        publish_to_host_outputs,
                    });
                }
                if let Some(updated) = command.continuous_recognition() {
                    continuous_recognition = updated;
                }
                if let Some(updated) = command.audio_source() {
                    audio_source = updated;
                }
                if is_finish && finish_sent {
                    continue;
                }
                if let Err(error) = send_session_command(&mut write, command, audio_source).await {
                    let _ = event_tx.send(SessionEvent::Error(format!("Failed to update session: {error}")));
                    return;
                }
                if is_finish {
                    finish_sent = true;
                }
                if is_resume {
                    paused.store(false, Ordering::Release);
                    let _ = event_tx.send(SessionEvent::Status("Connected — listening".into()));
                }
            }
            pcm = pcm_rx.recv(), if !pcm_input_closed => {
                let Some(pcm) = pcm else {
                    pcm_input_closed = true;
                    if finish_when_audio_ends && !finish_sent {
                        if let Err(error) = send_json(&mut write, json!({"event": "input_ended"})).await {
                            let _ = event_tx.send(SessionEvent::Error(format!("Failed to finish audio input: {error}")));
                            break;
                        }
                        finish_sent = true;
                    }
                    continue;
                };
                if paused.load(Ordering::Acquire) || finish_sent {
                    continue;
                }
                // System loopback would otherwise feed synthesized speech
                // straight back into ASR. Microphone capture remains live so
                // headphones and virtual-microphone routing still work.
                if audio_source == CaptureSource::SystemAudio
                    && tts_handle.as_ref().is_some_and(|tts| tts.is_playing())
                {
                    continue;
                }
                if external_audio_gate.blocks_audio() {
                    continue;
                }
                if !turn_started {
                    turn_started = true;
                    if let Err(error) = send_json(&mut write, json!({"event": "turn_started", "turn_id": "native-1"})).await {
                        let _ = event_tx.send(SessionEvent::Error(format!("Failed to begin audio turn: {error}")));
                        break;
                    }
                }
                if let Err(error) = write.send(Message::Binary(pcm.into())).await {
                    let _ = event_tx.send(SessionEvent::Error(format!("Failed to send microphone audio: {error}")));
                    break;
                }
            }
            message = read.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let drained = pipeline_drain_reason(&text);
                        forward_server_event(
                            &event_tx,
                            &text,
                            stream_id,
                            continuous_recognition,
                            publish_to_host_outputs,
                            audio_source,
                        );
                        if let Some(reason) = drained {
                            if reason == DrainReason::Paused {
                                if paused.load(Ordering::Acquire) {
                                    let _ = event_tx.send(SessionEvent::Status("Paused".into()));
                                }
                            } else {
                                let _ = write.close().await;
                                let _ = event_tx.send(SessionEvent::Disconnected("Finished".into()));
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(audio))) => {
                        if let Some(tts) = &tts_handle {
                            if let Err(error) = tts.play_pcm(&audio) {
                                let _ = event_tx.send(SessionEvent::BackendError {
                                    message: format!("TTS playback failed: {error}"),
                                    configuration_required: false,
                                });
                            }
                        }
                        let _ = event_tx.send(SessionEvent::TtsAudio(audio.to_vec()));
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = event_tx.send(SessionEvent::Disconnected("Backend closed the connection".into()));
                        if let Some(ctx) = &egui_ctx { ctx.request_repaint(); }
                        break;
                    }
                    Some(Err(error)) => {
                        let _ = event_tx.send(SessionEvent::Error(format!("Backend connection failed: {error}")));
                        if let Some(ctx) = &egui_ctx { ctx.request_repaint(); }
                        break;
                    }
                    _ => {}
                }
                if let Some(ctx) = &egui_ctx {
                    ctx.request_repaint();
                }
            }
        }
    }
}

async fn send_session_command<S>(
    write: &mut S,
    command: SessionCommand,
    audio_source: CaptureSource,
) -> Result<(), tungstenite::Error>
where
    S: futures::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    match command {
        SessionCommand::UpdateLanguageRoute {
            source_lang,
            target_lang,
        } => {
            send_json(
                write,
                json!({
                    "action": "session_config",
                    "source_lang": source_lang,
                    "target_lang": target_lang,
                }),
            )
            .await
        }
        SessionCommand::SetTtsEnabled(enabled) => {
            send_json(
                write,
                json!({
                    "action": "toggle_feature",
                    "feature": "tts",
                    "enabled": enabled,
                }),
            )
            .await
        }
        SessionCommand::BeginVoiceClone => {
            send_json(write, json!({"action": "begin_voice_clone"})).await
        }
        SessionCommand::UpdatePromptTemplates { graphs } => {
            send_json(
                write,
                json!({
                    "action": "set_prompt_graphs",
                    "prompt_graphs": graphs,
                }),
            )
            .await
        }
        SessionCommand::TranslateText {
            text,
            source_lang,
            target_lang,
        } => {
            send_json(
                write,
                json!({
                    "action": "translate_text",
                    "text": text,
                    "source_lang": source_lang,
                    "target_lang": target_lang,
                }),
            )
            .await
        }
        SessionCommand::ResetAudioPipeline {
            source_lang,
            target_lang,
            audio_source,
            vad_threshold,
            vad_silence_ms,
            continuous_recognition,
        } => {
            log::info!("Resetting backend audio pipeline after capture-source switch");
            send_json(
                write,
                json!({
                    "event": "config_audio",
                    "sample_rate": 16_000,
                    "source_lang": source_lang,
                    "target_lang": target_lang,
                    "audio_source": audio_source_name(audio_source),
                    "vad_threshold": vad_threshold,
                    "vad_silence_ms": vad_silence_ms,
                    "continuous_recognition": continuous_recognition,
                }),
            )
            .await
        }
        SessionCommand::UpdateAudioSegmentation {
            vad_threshold,
            vad_silence_ms,
            continuous_recognition,
            source_lang,
            target_lang,
        } => {
            send_json(
                write,
                json!({
                    "event": "config_audio", "sample_rate": 16_000,
                    "source_lang": source_lang, "target_lang": target_lang,
                    "vad_threshold": vad_threshold, "vad_silence_ms": vad_silence_ms,
                    "continuous_recognition": continuous_recognition,
                    "audio_source": audio_source_name(audio_source),
                }),
            )
            .await
        }
        SessionCommand::Pause => send_json(write, json!({"event": "pause"})).await,
        SessionCommand::Resume => send_json(write, json!({"event": "resume"})).await,
        SessionCommand::Finish => send_json(write, json!({"event": "finish"})).await,
        SessionCommand::Cancel => unreachable!("cancel closes the WebSocket before dispatch"),
    }
}

fn audio_source_name(source: CaptureSource) -> &'static str {
    match source {
        CaptureSource::Microphone => "microphone",
        CaptureSource::SystemAudio => "system_audio",
        CaptureSource::Both => unreachable!("Both expands into individual audio sessions"),
    }
}

async fn send_json<S>(write: &mut S, value: Value) -> Result<(), tungstenite::Error>
where
    S: futures::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    write.send(Message::Text(value.to_string().into())).await
}

fn pipeline_drain_reason(text: &str) -> Option<DrainReason> {
    match serde_json::from_str::<ProtocolServerEvent>(text).ok()? {
        ProtocolServerEvent::PipelineDrained(drained) => Some(drained.reason),
        _ => None,
    }
}

fn event_metadata<T>(data: Option<&serde_json::Map<String, Value>>, key: &str) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    data.and_then(|data| data.get(key))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn forward_server_event(
    event_tx: &Sender<SessionEvent>,
    text: &str,
    stream_id: u64,
    continuous_recognition: bool,
    publish_to_host_outputs: bool,
    audio_source: CaptureSource,
) {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let data = payload.get("data").and_then(Value::as_object);
    match payload.get("action").and_then(Value::as_str) {
        Some("session_ready") => {
            let _ = event_tx.send(SessionEvent::Status("Connected — listening".into()));
            if let Some(backend) = data
                .and_then(|data| data.get("tts_backend"))
                .and_then(Value::as_str)
            {
                let _ = event_tx.send(SessionEvent::TtsRuntime {
                    backend: backend.to_owned(),
                    cuda_version: data
                        .and_then(|data| data.get("tts_cuda_version"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
        Some("vad_activity") => {
            let active = data
                .and_then(|data| data.get("active"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let _ = event_tx.send(SessionEvent::VadActivity {
                source: audio_source,
                active,
            });
        }
        Some("asr_result") => {
            let kind: String = data
                .and_then(|d| d.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("partial")
                .into();
            let text_val: String = data
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();

            let _ = event_tx.send(SessionEvent::Asr {
                stream_id,
                continuous: continuous_recognition,
                publish_to_host_outputs,
                kind,
                text: text_val,
                turn_id: data
                    .and_then(|d| d.get("turn_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            });
        }
        Some("source_segment_ready") => {
            let Some(revisable) = data
                .and_then(|d| d.get("revisable"))
                .and_then(Value::as_bool)
            else {
                return;
            };
            let Some(overlap_ratio) = data
                .and_then(|d| d.get("overlap_ratio"))
                .and_then(Value::as_f64)
            else {
                return;
            };
            let _ = event_tx.send(SessionEvent::SourceSegment {
                stream_id,
                audio_source,
                continuous: continuous_recognition,
                publish_to_host_outputs,
                text: data
                    .and_then(|d| d.get("source_text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                prompt_trace: event_metadata(data, "prompt_trace"),
                activation_matches: data
                    .and_then(|d| d.get("activation_matches"))
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default(),
                context_matches: data
                    .and_then(|d| d.get("context_matches"))
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default(),
                turn_id: data
                    .and_then(|d| d.get("turn_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                speaker_id: data
                    .and_then(|d| d.get("speaker_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                source_start_ms: data
                    .and_then(|d| d.get("source_start_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                source_end_ms: data
                    .and_then(|d| d.get("source_end_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                timing: event_metadata(data, "timing"),
                boundary: event_metadata(data, "boundary"),
                segment_index: data
                    .and_then(|d| d.get("segment_index"))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1),
                segment_count: data
                    .and_then(|d| d.get("segment_count"))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1),
                revisable,
                overlap_ratio: overlap_ratio as f32,
                authoritative_snapshot: data
                    .and_then(|d| d.get("authoritative_snapshot"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                revision: data
                    .and_then(|d| d.get("revision"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            });
        }
        Some("translation_ready") => {
            let source: String = data
                .and_then(|d| d.get("source_text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            let translated: String = data
                .and_then(|d| d.get("translated_text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            let speaker_id: String = data
                .and_then(|d| d.get("speaker_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            let Some(revisable) = data
                .and_then(|d| d.get("revisable"))
                .and_then(Value::as_bool)
            else {
                return;
            };
            let Some(overlap_ratio) = data
                .and_then(|d| d.get("overlap_ratio"))
                .and_then(Value::as_f64)
            else {
                return;
            };

            let _ = event_tx.send(SessionEvent::Translation {
                stream_id,
                audio_source,
                continuous: continuous_recognition,
                publish_to_host_outputs,
                source,
                translated,
                turn_id: data
                    .and_then(|d| d.get("turn_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                segment_index: data
                    .and_then(|d| d.get("segment_index"))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1),
                segment_count: data
                    .and_then(|d| d.get("segment_count"))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1),
                speaker_id,
                source_start_ms: data
                    .and_then(|d| d.get("source_start_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                source_end_ms: data
                    .and_then(|d| d.get("source_end_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                timing: event_metadata(data, "timing"),
                boundary: event_metadata(data, "boundary"),
                term_matches: data
                    .and_then(|d| d.get("term_matches"))
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default(),
                prompt_trace: event_metadata(data, "prompt_trace"),
                revisable,
                overlap_ratio: overlap_ratio as f32,
                authoritative_snapshot: data
                    .and_then(|d| d.get("authoritative_snapshot"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                revision: data
                    .and_then(|d| d.get("revision"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            });
        }
        Some("recognition_stream_ended") => {
            let _ = event_tx.send(SessionEvent::StreamEnded {
                stream_id,
                publish_to_host_outputs,
            });
        }
        Some("route_changed") => {
            if let Some(data) = data {
                let source_lang = data
                    .get("source_lang")
                    .and_then(Value::as_str)
                    .unwrap_or("auto")
                    .to_string();
                let target_lang = data
                    .get("target_lang")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !target_lang.is_empty() {
                    let _ = event_tx.send(SessionEvent::RouteChanged {
                        source_lang,
                        target_lang,
                    });
                }
            }
        }
        Some("voice_clone_state") => {
            if let Some(data) = data
                && let Ok(status) = serde_json::from_value(Value::Object(data.clone()))
            {
                let _ = event_tx.send(SessionEvent::VoiceCloneState {
                    source: audio_source,
                    status,
                });
            }
        }
        Some("error") => {
            let _ = event_tx.send(SessionEvent::BackendError {
                message: data
                    .and_then(|d| d.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown backend error")
                    .into(),
                configuration_required: data
                    .and_then(|d| d.get("configuration_required"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        _ => {}
    }
}

fn f32_to_pcm16le(samples: Vec<f32>) -> Vec<u8> {
    samples
        .into_iter()
        .flat_map(|sample| {
            let sample = sample.clamp(-1.0, 1.0);
            let pcm = if sample < 0.0 {
                (sample * 32768.0) as i16
            } else {
                (sample * 32767.0) as i16
            };
            pcm.to_le_bytes()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_drain_reason_distinguishes_pause_from_terminal_eof() {
        assert_eq!(
            pipeline_drain_reason(r#"{"action":"pipeline_drained","data":{"reason":"paused"}}"#),
            Some(DrainReason::Paused)
        );
        assert_eq!(
            pipeline_drain_reason(
                r#"{"action":"pipeline_drained","data":{"reason":"input_ended"}}"#
            ),
            Some(DrainReason::InputEnded)
        );
        assert_eq!(
            pipeline_drain_reason(r#"{"action":"vad_activity","data":{"active":false}}"#),
            None
        );
    }

    #[test]
    fn external_audio_gate_requires_both_enablement_and_active_state() {
        let gate = ExternalAudioGate::default();
        assert!(!gate.blocks_audio());
        gate.active.store(true, Ordering::Release);
        assert!(!gate.blocks_audio());
        gate.enabled.store(true, Ordering::Release);
        assert!(gate.blocks_audio());
    }

    #[test]
    fn asr_event_retains_stream_and_continuous_metadata() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"asr_result","data":{"type":"partial","text":"hello","turn_id":"turn-1"}}"#,
            41,
            true,
            true,
            CaptureSource::Microphone,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::Asr {
                stream_id: 41,
                continuous: true,
                publish_to_host_outputs: true,
                kind,
                text,
                turn_id,
            } if kind == "partial" && text == "hello" && turn_id == "turn-1"
        ));
    }

    #[test]
    fn session_ready_forwards_actual_tts_runtime() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"session_ready","data":{"session_id":"s1","source_lang":"en","target_lang":"zh","tts_backend":"cuda","tts_cuda_version":"13.3"}}"#,
            0,
            false,
            true,
            CaptureSource::Microphone,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::Status(_)
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::TtsRuntime { backend, cuda_version }
                if backend == "cuda" && cuda_version.as_deref() == Some("13.3")
        ));
    }

    #[test]
    fn vad_activity_keeps_its_audio_source() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let stream_id = 42;
        forward_server_event(
            &sender,
            r#"{"action":"vad_activity","data":{"active":true}}"#,
            stream_id,
            true,
            true,
            CaptureSource::SystemAudio,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::VadActivity {
                source: CaptureSource::SystemAudio,
                active: true
            }
        ));
    }

    #[test]
    fn source_segment_event_retains_speaker_and_timeline_metadata() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let stream_id = 42;
        forward_server_event(
            &sender,
            r#"{"action":"source_segment_ready","data":{"source_text":"hello","speaker_id":"speaker-03","source_start_ms":125.0,"source_end_ms":875.0,"timing":"estimated_text_partition","boundary":"speaker_change","segment_index":2,"segment_count":2,"revisable":true,"overlap_ratio":0.34}}"#,
            stream_id,
            false,
            true,
            CaptureSource::Microphone,
        );

        let event = receiver.try_recv().unwrap();
        let SessionEvent::SourceSegment {
            stream_id,
            text,
            activation_matches,
            speaker_id,
            source_start_ms,
            source_end_ms,
            timing,
            boundary,
            segment_index,
            ..
        } = event
        else {
            panic!("expected source-segment event");
        };
        assert_eq!(stream_id, 42);
        assert_eq!(text, "hello");
        assert!(activation_matches.is_empty());
        assert_eq!(speaker_id, "speaker-03");
        assert_eq!(source_start_ms, 125.0);
        assert_eq!(source_end_ms, 875.0);
        assert_eq!(timing, SegmentTiming::EstimatedTextPartition);
        assert_eq!(boundary, SegmentBoundary::SpeakerChange);
        assert_eq!(segment_index, 2);
    }

    #[test]
    fn backend_feature_errors_are_nonfatal_session_events() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let stream_id = 42;
        forward_server_event(
            &sender,
            r#"{"action":"error","data":{"message":"speaker recognition is unavailable"}}"#,
            stream_id,
            false,
            true,
            CaptureSource::Microphone,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::BackendError { message, configuration_required: false }
                if message == "speaker recognition is unavailable"
        ));
    }

    #[test]
    fn provider_configuration_errors_keep_the_redirect_signal() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"error","data":{"message":"HTTP 401","configuration_required":true}}"#,
            42,
            false,
            true,
            CaptureSource::Microphone,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::BackendError {
                message,
                configuration_required: true
            } if message == "HTTP 401"
        ));
    }

    #[test]
    fn translation_event_retains_term_provenance() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let stream_id = 42;
        forward_server_event(
            &sender,
            r#"{"action":"translation_ready","data":{"source_text":"I love Mercy.","translated_text":"我喜欢天使。","speaker_id":"","revisable":false,"overlap_ratio":0.0,"term_matches":[{"start_byte":9,"end_byte":15,"text":"天使","sources":[{"corpus_id":"games.overwatch.heroes","domain":"games","subdomain":"overwatch","title":"Overwatch Heroes"}]}]}}"#,
            stream_id,
            false,
            true,
            CaptureSource::Microphone,
        );

        let SessionEvent::Translation { term_matches, .. } = receiver.try_recv().unwrap() else {
            panic!("expected translation event");
        };
        assert_eq!(term_matches[0].text, "天使");
        assert_eq!(
            term_matches[0].sources[0].corpus_id,
            "games.overwatch.heroes"
        );
    }

    #[test]
    fn authoritative_translation_retains_its_backend_revision() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"translation_ready","data":{"source_text":"corrected","translated_text":"final","speaker_id":"","revisable":true,"overlap_ratio":0.34,"authoritative_snapshot":true,"revision":17}}"#,
            42,
            true,
            true,
            CaptureSource::Microphone,
        );

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SessionEvent::Translation {
                authoritative_snapshot: true,
                revision: 17,
                ..
            }
        ));
    }

    #[test]
    fn translation_event_retains_the_prompt_execution_trace() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"translation_ready","data":{"source_text":"hello","translated_text":"你好","speaker_id":"","revisable":false,"overlap_ratio":0.0,"prompt_trace":{"target":"hunyuan","nodes":[{"node_id":"current-input","output":"hello"}]}}}"#,
            42,
            false,
            true,
            CaptureSource::Microphone,
        );

        let SessionEvent::Translation {
            prompt_trace: Some(trace),
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected translation trace");
        };
        assert_eq!(
            trace.target,
            xrtranslate_prompt::PromptProviderTarget::Hunyuan
        );
        assert_eq!(trace.node("current-input").unwrap().output, "hello");
    }

    #[test]
    fn source_segment_event_retains_activation_provenance() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let stream_id = 42;
        forward_server_event(
            &sender,
            r#"{"action":"source_segment_ready","data":{"source_text":"论文写没？","segment_index":1,"segment_count":1,"revisable":false,"overlap_ratio":0.0,"activation_matches":[{"start_byte":0,"end_byte":6,"text":"论文","sources":[{"corpus_id":"education-and-science.research.common","domain":"education-and-science","subdomain":"research","title":"研究与学术交流"}]}]}}"#,
            stream_id,
            false,
            true,
            CaptureSource::Microphone,
        );

        let SessionEvent::SourceSegment {
            activation_matches, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected source-segment event");
        };
        assert_eq!(activation_matches[0].text, "论文");
        assert_eq!(activation_matches[0].sources[0].subdomain, "research");
    }

    #[test]
    fn source_segment_event_retains_the_asr_prompt_execution_trace() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"source_segment_ready","data":{"source_text":"hello","segment_index":1,"segment_count":1,"revisable":false,"overlap_ratio":0.0,"prompt_trace":{"target":"asr_instruction","nodes":[{"node_id":"asr-instruction-request","output":"Transcribe accurately."}]}}}"#,
            42,
            false,
            true,
            CaptureSource::Microphone,
        );

        let SessionEvent::SourceSegment {
            prompt_trace: Some(trace),
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected ASR prompt trace");
        };
        assert_eq!(
            trace.target,
            xrtranslate_prompt::PromptProviderTarget::AsrInstruction
        );
        assert_eq!(
            trace.node("asr-instruction-request").unwrap().output,
            "Transcribe accurately."
        );
    }

    #[test]
    fn route_changed_event_forwards_language_pair() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        forward_server_event(
            &sender,
            r#"{"action":"route_changed","data":{"source_lang":"auto","target_lang":"ja,zh"}}"#,
            42,
            false,
            true,
            CaptureSource::Microphone,
        );

        let SessionEvent::RouteChanged {
            source_lang,
            target_lang,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected route_changed event");
        };
        assert_eq!(source_lang, "auto");
        assert_eq!(target_lang, "ja,zh");
    }
}
