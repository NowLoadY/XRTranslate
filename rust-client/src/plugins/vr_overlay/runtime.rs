//! Background runtime, worker loop, and settings for the SteamVR overlay plugin.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::avatar::AvatarOverlay;
use super::graphics;
use super::openvr::{OpenVrApi, OpenVrOverlay, OpenVrSession, OverlayError};
use super::renderer::{VrOverlayRenderer, VrSubtitleCard};
use super::spatial::VoiceOverlay;
use crate::client_settings::CaptureSource;
use crate::session_coordinator::AudioSourceFilter;
use crate::ui::components::avatar::Presentation;
use crate::voice_interaction::{VoiceAction, VoiceSnapshot};

const RENDER_WIDTH: u32 = 640;
const RENDER_HEIGHT: u32 = 320;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VrOverlaySettings {
    #[serde(default)]
    pub audio_source_filter: AudioSourceFilter,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub avatar_enabled: bool,
    #[serde(default)]
    pub interaction_enabled: bool,
    #[serde(default)]
    pub review_before_send: bool,
    #[serde(default = "default_true")]
    pub captions_enabled: bool,
    #[serde(default = "default_max_items")]
    pub max_items: usize,
    #[serde(default = "default_true")]
    pub bilingual: bool,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_distance")]
    pub distance_meters: f32,
    #[serde(default = "default_vertical_offset")]
    pub vertical_offset_meters: f32,
    #[serde(default = "default_pitch")]
    pub pitch_degrees: f32,
    #[serde(default = "default_width_meters")]
    pub overlay_width_meters: f32,
    #[serde(default = "default_timeout")]
    pub display_timeout_seconds: f32,
}

fn default_true() -> bool {
    true
}

pub fn default_max_items() -> usize {
    3
}

pub fn default_font_size() -> f32 {
    20.0
}

pub fn default_opacity() -> f32 {
    0.85
}

pub fn default_distance() -> f32 {
    1.00
}

pub fn default_vertical_offset() -> f32 {
    0.00
}

pub fn default_pitch() -> f32 {
    0.0
}

pub fn default_width_meters() -> f32 {
    0.50
}

pub fn default_timeout() -> f32 {
    12.0
}

impl VrOverlaySettings {
    pub const DEFAULT_MAX_ITEMS: usize = 3;
    pub const DEFAULT_FONT_SIZE: f32 = 20.0;
    pub const DEFAULT_OPACITY: f32 = 0.85;
    pub const DEFAULT_DISTANCE: f32 = 1.00;
    pub const DEFAULT_VERTICAL_OFFSET: f32 = 0.00;
    pub const DEFAULT_OVERLAY_WIDTH: f32 = 0.50;
    pub const DEFAULT_TIMEOUT: f32 = 12.0;

    // UI and persisted settings cross the worker boundary as one finite snapshot.
    pub fn normalize(&mut self) {
        fn finite(value: f32, fallback: f32, min: f32, max: f32) -> f32 {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                fallback
            }
        }
        self.max_items = self.max_items.clamp(1, 5);
        self.font_size = finite(self.font_size, default_font_size(), 12.0, 36.0);
        self.opacity = finite(self.opacity, default_opacity(), 0.0, 1.0);
        self.distance_meters = finite(self.distance_meters, default_distance(), 0.2, 5.0);
        self.vertical_offset_meters = finite(
            self.vertical_offset_meters,
            default_vertical_offset(),
            -5.0,
            5.0,
        );
        self.pitch_degrees = finite(self.pitch_degrees, default_pitch(), -90.0, 90.0);
        self.overlay_width_meters =
            finite(self.overlay_width_meters, default_width_meters(), 0.2, 5.0);
        self.display_timeout_seconds =
            finite(self.display_timeout_seconds, default_timeout(), 0.1, 300.0);
    }

    #[allow(dead_code)]
    pub fn auto_pitch_degrees(&self) -> f32 {
        (-self.vertical_offset_meters)
            .atan2(self.distance_meters.max(0.1))
            .to_degrees()
    }
}

impl Default for VrOverlaySettings {
    fn default() -> Self {
        Self {
            audio_source_filter: AudioSourceFilter::default(),
            enabled: true,
            avatar_enabled: false,
            interaction_enabled: false,
            review_before_send: false,
            captions_enabled: true,
            max_items: default_max_items(),
            bilingual: true,
            font_size: default_font_size(),
            opacity: default_opacity(),
            distance_meters: default_distance(),
            vertical_offset_meters: default_vertical_offset(),
            pitch_degrees: default_pitch(),
            overlay_width_meters: default_width_meters(),
            display_timeout_seconds: default_timeout(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VrRuntimeStatus {
    pub steamvr_installed: bool,
    pub steamvr_connected: bool,
    pub avatar_available: bool,
    pub avatar_present: bool,
    pub avatar_attention: bool,
    pub avatar_greeting: bool,
    pub avatar_error: Option<String>,
    pub interaction_available: bool,
    pub interaction_error: Option<String>,
    pub last_error: Option<String>,
    pub active_card_count: usize,
    pub latest_caption_preview: Option<String>,
    pub cards: Vec<VrSubtitleCard>,
}

#[derive(Debug)]
pub enum VrCommand {
    Previews(
        u64,
        bool,
        Vec<crate::session_coordinator::TranslationPreview>,
    ),
    Caption {
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: String,
        translated: String,
        additional_translations: Vec<String>,
        speaker: String,
        live: bool,
    },
    RollStream {
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: String,
        translated: String,
        additional_translations: Vec<String>,
        speaker: String,
    },
    EndStream(u64),
    CancelStream(u64),
    Clear,
    Connect,
    Disconnect,
    UpdateSettings(VrOverlaySettings),
    VisitAvatar,
    ReturnAvatar,
    Shutdown,
}

#[derive(Clone, Debug)]
struct StreamEntry {
    stream_id: u64,
    audio_source: CaptureSource,
    is_typing: bool,
    source: String,
    translated: String,
    additional_translations: Vec<String>,
    speaker: String,
    live: bool,
    updated_at: Instant,
}

pub struct VrOverlayManager {
    command_tx: Sender<VrCommand>,
    status: Arc<Mutex<VrRuntimeStatus>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    companion: Arc<Mutex<Presentation>>,
    pending_cancellations: Arc<Mutex<HashSet<u64>>>,
    voice: Arc<Mutex<VoiceBridge>>,
}

#[derive(Default)]
struct VoiceBridge {
    snapshot: VoiceSnapshot,
    actions: VecDeque<VoiceAction>,
    microphone: bool,
    dock_requested: bool,
}

impl VoiceBridge {
    fn push(&mut self, action: VoiceAction) {
        // A frame already in progress can return an obsolete near event after
        // the host requests docking. Reject it until the worker consumes that request.
        if self.dock_requested && action == VoiceAction::Microphone(true) {
            return;
        }
        if !self.snapshot.enabled && action != VoiceAction::Microphone(false) {
            return;
        }
        if let VoiceAction::Microphone(enabled) = action {
            if enabled == self.microphone {
                return;
            }
            self.microphone = enabled;
        }
        // A stalled UI must not turn a bounded queue into an unbounded command
        // backlog. Overflow fails closed, retaining the mandatory microphone-off.
        if self.actions.len() >= 16 {
            self.actions.clear();
            self.microphone = false;
            self.actions.push_back(VoiceAction::Microphone(false));
        } else {
            self.actions.push_back(action);
        }
    }

    fn dock_microphone(&mut self) {
        self.dock_requested = true;
        self.microphone = false;
        self.actions
            .retain(|action| !matches!(action, VoiceAction::Microphone(_)));
        // Keep other spatial actions, giving microphone-off priority at capacity.
        if self.actions.len() >= 16 {
            self.actions.pop_back();
        }
        self.actions.push_front(VoiceAction::Microphone(false));
    }

    fn stop(&mut self) {
        let active = self.microphone || (self.snapshot.enabled && self.snapshot.recording);
        self.actions.clear();
        self.microphone = false;
        if active {
            self.actions.push_back(VoiceAction::Microphone(false));
        }
    }
}

struct StopVoiceOnDrop(Arc<Mutex<VoiceBridge>>);
impl Drop for StopVoiceOnDrop {
    fn drop(&mut self) {
        self.0.lock().stop();
    }
}

impl VrOverlayManager {
    pub fn new(settings: VrOverlaySettings, input_directory: PathBuf) -> Self {
        let (command_tx, command_rx) = unbounded();
        let status = Arc::new(Mutex::new(VrRuntimeStatus::default()));
        let worker_status = Arc::clone(&status);

        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let companion = Arc::new(Mutex::new(Presentation::default()));
        let worker_companion = Arc::clone(&companion);
        let pending_cancellations = Arc::new(Mutex::new(HashSet::new()));
        let worker_cancellations = Arc::clone(&pending_cancellations);
        let voice = Arc::new(Mutex::new(VoiceBridge::default()));
        let worker_voice = Arc::clone(&voice);
        let worker = std::thread::Builder::new()
            .name("vr-overlay-worker".into())
            .spawn(move || {
                run_vr_worker(
                    command_rx,
                    settings,
                    worker_status,
                    worker_shutdown,
                    worker_companion,
                    worker_cancellations,
                    worker_voice,
                    input_directory,
                )
            })
            .expect("failed to spawn VR overlay worker");

        Self {
            command_tx,
            status,
            shutdown,
            worker: Some(worker),
            companion,
            pending_cancellations,
            voice,
        }
    }

    pub fn handle(&self) -> VrOverlayHandle {
        VrOverlayHandle {
            command_tx: self.command_tx.clone(),
            pending_cancellations: self.pending_cancellations.clone(),
        }
    }

    pub(crate) fn present_companion(&self, presentation: Presentation) {
        *self.companion.lock() = presentation;
    }
    pub fn visit_avatar(&self) {
        let _ = self.command_tx.send(VrCommand::VisitAvatar);
    }
    pub fn return_avatar(&self) {
        let _ = self.command_tx.send(VrCommand::ReturnAvatar);
    }

    pub fn status(&self) -> VrRuntimeStatus {
        self.status.lock().clone()
    }

    pub(crate) fn take_voice_actions(&self) -> Vec<VoiceAction> {
        self.voice.lock().actions.drain(..).collect()
    }

    pub(crate) fn dock_microphone(&self) {
        self.voice.lock().dock_microphone();
    }

    pub(crate) fn set_voice_snapshot(&self, snapshot: VoiceSnapshot) {
        let mut voice = self.voice.lock();
        if !snapshot.enabled {
            voice.stop();
        }
        voice.snapshot = snapshot;
    }

    pub fn update_settings(&self, settings: VrOverlaySettings) {
        let _ = self.command_tx.send(VrCommand::UpdateSettings(settings));
    }

    pub fn connect(&self) {
        let _ = self.command_tx.send(VrCommand::Connect);
    }

    pub fn disconnect(&self) {
        let _ = self.command_tx.send(VrCommand::Disconnect);
    }
}

impl Drop for VrOverlayManager {
    fn drop(&mut self) {
        // Wake the receiver and stop before queued captions/settings. Join prevents
        // an old worker shutting OpenVR down after a replacement manager starts.
        self.shutdown.store(true, Ordering::Release);
        let _ = self.command_tx.send(VrCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                log::warn!("VR overlay worker panicked");
            }
        }
    }
}

#[derive(Clone)]
pub struct VrOverlayHandle {
    command_tx: Sender<VrCommand>,
    pending_cancellations: Arc<Mutex<HashSet<u64>>>,
}

impl VrOverlayHandle {
    pub fn set_previews(
        &self,
        stream: u64,
        is_typing: bool,
        entries: &[crate::session_coordinator::TranslationPreview],
    ) {
        let _ = self
            .command_tx
            .send(VrCommand::Previews(stream, is_typing, entries.to_vec()));
    }

    pub fn add_caption(
        &self,
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &str,
        translated: &str,
        additional_translations: &[xrtranslate_protocol::AdditionalTranslation],
        speaker: &str,
        live: bool,
    ) {
        let _ = self.command_tx.send(VrCommand::Caption {
            stream_id,
            audio_source,
            is_typing,
            source: source.to_owned(),
            translated: translated.to_owned(),
            additional_translations: additional_caption_lines(additional_translations),
            speaker: speaker.to_owned(),
            live,
        });
    }

    pub fn roll_stream(
        &self,
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &str,
        translated: &str,
        additional_translations: &[xrtranslate_protocol::AdditionalTranslation],
        speaker: &str,
    ) {
        let _ = self.command_tx.send(VrCommand::RollStream {
            stream_id,
            audio_source,
            is_typing,
            source: source.to_owned(),
            translated: translated.to_owned(),
            additional_translations: additional_caption_lines(additional_translations),
            speaker: speaker.to_owned(),
        });
    }

    pub fn end_stream(&self, stream_id: u64) {
        let _ = self.command_tx.send(VrCommand::EndStream(stream_id));
    }

    pub fn cancel_stream(&self, stream_id: u64) {
        // Publish cancellation ahead of the worker backlog, which is consumed
        // in bounded batches. The same lock fences frame submission below.
        let mut pending = self.pending_cancellations.lock();
        pending.insert(stream_id);
        if self
            .command_tx
            .send(VrCommand::CancelStream(stream_id))
            .is_err()
        {
            pending.remove(&stream_id);
        }
    }

    pub fn clear(&self) {
        let _ = self.command_tx.send(VrCommand::Clear);
    }
}

fn additional_caption_lines(
    translations: &[xrtranslate_protocol::AdditionalTranslation],
) -> Vec<String> {
    translations
        .iter()
        .filter(|result| !result.translated_text.trim().is_empty())
        .map(|result| result.translated_text.trim().to_owned())
        .collect()
}

fn run_vr_worker(
    command_rx: Receiver<VrCommand>,
    mut settings: VrOverlaySettings,
    status: Arc<Mutex<VrRuntimeStatus>>,
    shutdown: Arc<AtomicBool>,
    companion: Arc<Mutex<Presentation>>,
    pending_cancellations: Arc<Mutex<HashSet<u64>>>,
    voice_bridge: Arc<Mutex<VoiceBridge>>,
    input_directory: PathBuf,
) {
    let _stop_voice = StopVoiceOnDrop(Arc::clone(&voice_bridge));
    settings.normalize();
    let mut renderer = VrOverlayRenderer::new(RENDER_WIDTH, RENDER_HEIGHT)
        .expect("fixed subtitle dimensions are valid");
    let mut openvr_api: Option<Arc<OpenVrApi>> = OpenVrApi::try_load();
    let mut vr_session: Option<OpenVrSession> = None;
    let mut vr_overlay: Option<OpenVrOverlay> = None;
    let mut graphics = None;
    let mut avatar: Option<AvatarOverlay> = None;
    let mut avatar_initialized = false;
    let mut voice: Option<VoiceOverlay> = None;
    let mut voice_initialized = false;
    let mut next_avatar_frame = Instant::now();
    let mut next_session_check = Instant::now();

    let mut entries: Vec<StreamEntry> = Vec::new();
    let mut previews: Vec<StreamEntry> = Vec::new();
    let mut needs_redraw = false;
    let mut is_overlay_visible = false;

    if let Some(api) = &openvr_api {
        status.lock().steamvr_installed = api.is_runtime_installed();
    }

    while !shutdown.load(Ordering::Acquire) {
        let timeout = if settings.interaction_enabled
            || (settings.avatar_enabled && status.lock().avatar_available)
        {
            next_avatar_frame
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100))
        } else {
            Duration::from_millis(100)
        };
        let first = command_rx.recv_timeout(timeout);
        let mut commands = vec![first];
        commands.extend(command_rx.try_iter().take(127).map(Ok));
        for command in commands {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            match command {
                Ok(VrCommand::Connect) => {
                    if openvr_api.is_none() {
                        openvr_api = OpenVrApi::try_load();
                    }
                    match &openvr_api {
                        Some(api) => {
                            let installed = api.is_runtime_installed();
                            let running = super::openvr::is_steamvr_running();
                            {
                                let mut st = status.lock();
                                st.steamvr_installed = installed;
                            }
                            if !installed {
                                let mut st = status.lock();
                                st.steamvr_connected = false;
                                st.last_error =
                                    Some("SteamVR runtime is not installed on this system.".into());
                            } else if !running {
                                let mut st = status.lock();
                                st.steamvr_connected = false;
                                st.last_error = Some(
                                    "SteamVR is not running. Please launch SteamVR first.".into(),
                                );
                            } else if vr_session.is_none() {
                                match api.init_overlay() {
                                    Ok(session) => match session.create_overlay(
                                        "xrtranslate.vr_overlay",
                                        "XRTranslate Subtitles",
                                    ) {
                                        Ok(overlay) => {
                                            if let Err(error) =
                                                configure_overlay(&overlay, &settings)
                                            {
                                                let mut st = status.lock();
                                                st.steamvr_connected = false;
                                                st.last_error = Some(error.to_string());
                                                continue;
                                            }
                                            let surfaces =
                                                session.avatar_tracking().and_then(|tracking| {
                                                    let gpu = graphics::create(&tracking)?;
                                                    let (width, height) = renderer.dimensions();
                                                    let captions = graphics::RgbaTexture::new(
                                                        &tracking,
                                                        gpu.clone(),
                                                        width,
                                                        height,
                                                    )?;
                                                    Ok((gpu, captions))
                                                });
                                            let graphics_error = match surfaces {
                                                Ok(surfaces) => {
                                                    graphics = Some(surfaces);
                                                    None
                                                }
                                                Err(error) => {
                                                    log::warn!(
                                                        "VR captions using raw upload: {error}"
                                                    );
                                                    Some(error)
                                                }
                                            };
                                            is_overlay_visible = false;
                                            avatar_initialized = false;
                                            voice_initialized = false;
                                            status.lock().interaction_error = None;
                                            vr_overlay = Some(overlay);
                                            vr_session = Some(session);
                                            needs_redraw = true;
                                            let mut st = status.lock();
                                            st.steamvr_connected = true;
                                            st.avatar_error = graphics_error;
                                            st.last_error = None;
                                        }
                                        Err(e) => {
                                            let mut st = status.lock();
                                            st.steamvr_connected = false;
                                            st.last_error = Some(e);
                                        }
                                    },
                                    Err(e) => {
                                        let mut st = status.lock();
                                        st.steamvr_connected = false;
                                        st.last_error = Some(e);
                                    }
                                }
                            }
                        }
                        None => {
                            let mut st = status.lock();
                            st.steamvr_installed = false;
                            st.steamvr_connected = false;
                            st.last_error =
                                Some("OpenVR library not found. SteamVR connection is currently supported on Windows.".into());
                        }
                    }
                }
                Ok(VrCommand::Disconnect) => {
                    voice_bridge.lock().stop();
                    voice = None;
                    status.lock().interaction_available = false;
                    avatar = None;
                    vr_overlay = None;
                    graphics = None;
                    vr_session = None;
                    is_overlay_visible = false;
                    let mut st = status.lock();
                    st.steamvr_connected = false;
                    st.avatar_available = false;
                    st.avatar_present = false;
                    st.avatar_attention = false;
                    st.avatar_greeting = false;
                    st.avatar_error = None;
                    st.last_error = None;
                }
                Ok(VrCommand::Previews(stream_id, is_typing, incoming)) => {
                    previews.retain(|entry| entry.stream_id != stream_id);
                    previews.extend(
                        incoming
                            .into_iter()
                            .filter(|entry| {
                                settings
                                    .audio_source_filter
                                    .allows(entry.audio_source, is_typing)
                            })
                            .map(|entry| StreamEntry {
                                stream_id,
                                audio_source: entry.audio_source,
                                is_typing,
                                source: entry.source,
                                translated: entry.translated.unwrap_or_default(),
                                additional_translations: Vec::new(),
                                speaker: String::new(),
                                live: true,
                                updated_at: Instant::now(),
                            }),
                    );
                    needs_redraw = true;
                }
                Ok(VrCommand::Caption {
                    stream_id,
                    audio_source,
                    is_typing,
                    source,
                    translated,
                    additional_translations,
                    speaker,
                    live,
                }) => {
                    if !settings.audio_source_filter.allows(audio_source, is_typing) {
                        continue;
                    }
                    upsert_caption(
                        &mut entries,
                        stream_id,
                        audio_source,
                        is_typing,
                        source,
                        translated,
                        additional_translations,
                        speaker,
                        live,
                        Instant::now(),
                    );
                    clamp_entries(&mut entries, settings.max_items);
                    needs_redraw = true;
                }
                Ok(VrCommand::RollStream {
                    stream_id,
                    audio_source,
                    is_typing,
                    source,
                    translated,
                    additional_translations,
                    speaker,
                }) => {
                    if !settings.audio_source_filter.allows(audio_source, is_typing) {
                        continue;
                    }
                    upsert_caption(
                        &mut entries,
                        stream_id,
                        audio_source,
                        is_typing,
                        source,
                        translated,
                        additional_translations,
                        speaker,
                        false,
                        Instant::now(),
                    );
                    clamp_entries(&mut entries, settings.max_items);
                    needs_redraw = true;
                }
                Ok(VrCommand::EndStream(stream_id)) => {
                    previews.retain(|entry| entry.stream_id != stream_id);
                    needs_redraw = true;
                    if let Some(existing) = entries
                        .iter_mut()
                        .find(|e| e.stream_id == stream_id && e.live)
                    {
                        existing.live = false;
                        existing.updated_at = Instant::now();
                        needs_redraw = true;
                    }
                }
                Ok(VrCommand::CancelStream(stream_id)) => {
                    entries.retain(|entry| entry.stream_id != stream_id);
                    previews.retain(|entry| entry.stream_id != stream_id);
                    // Host cancellation invalidates the scope before publishing
                    // this command; only its already queued captions precede it.
                    pending_cancellations.lock().remove(&stream_id);
                    needs_redraw = true;
                }
                Ok(VrCommand::Clear) => {
                    entries.clear();
                    previews.clear();
                    needs_redraw = true;
                }
                Ok(VrCommand::UpdateSettings(mut updated)) => {
                    updated.normalize();
                    if updated.interaction_enabled && !settings.interaction_enabled {
                        voice_initialized = voice.is_some();
                        status.lock().interaction_error = None;
                    }
                    if !updated.interaction_enabled {
                        voice_bridge.lock().stop();
                        if let Some(objects) = voice.as_mut() {
                            objects.suspend();
                        }
                    }
                    if updated.avatar_enabled && !settings.avatar_enabled {
                        if let Some(avatar) = avatar.as_mut() {
                            avatar.resume();
                            status.lock().avatar_error = None;
                        }
                    }
                    settings = updated;
                    entries.retain(|entry| {
                        settings
                            .audio_source_filter
                            .allows(entry.audio_source, entry.is_typing)
                    });
                    clamp_entries(&mut entries, settings.max_items);
                    needs_redraw = true;
                }
                Ok(VrCommand::VisitAvatar) => {
                    if settings.avatar_enabled
                        && let Some(avatar) = avatar.as_mut()
                    {
                        avatar.request_visit();
                    }
                }
                Ok(VrCommand::ReturnAvatar) => {
                    if let Some(avatar) = avatar.as_mut() {
                        avatar.return_from_visit();
                    }
                }
                Ok(VrCommand::Shutdown) => return,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
        needs_redraw |= expire_entries(
            &mut entries,
            Instant::now(),
            settings.display_timeout_seconds,
        );

        needs_redraw |= expire_entries(
            &mut previews,
            Instant::now(),
            settings.display_timeout_seconds,
        );

        // Passive session monitor: detect if SteamVR was closed by user or if disabled
        if vr_session.is_some() {
            // Process enumeration is a liveness check, not part of every frame.
            let running = if Instant::now() >= next_session_check {
                next_session_check = Instant::now() + Duration::from_secs(1);
                super::openvr::is_steamvr_running()
            } else {
                true
            };
            if !running || !settings.enabled {
                voice_bridge.lock().stop();
                voice = None;
                status.lock().interaction_available = false;
                avatar = None;
                vr_overlay = None;
                graphics = None;
                vr_session = None;
                is_overlay_visible = false;
                let mut st = status.lock();
                st.steamvr_connected = false;
                st.avatar_available = false;
                st.avatar_present = false;
                st.avatar_attention = false;
                st.avatar_greeting = false;
            }
        }

        // Remove cancelled streams even when their command is beyond this batch.
        // Keep the fence through native submission, so cancellation returning to
        // the host cannot be followed by a newly submitted stale caption frame.
        let pending = pending_cancellations.lock();
        let previous_count = entries.len() + previews.len();
        entries.retain(|entry| !pending.contains(&entry.stream_id));
        previews.retain(|entry| !pending.contains(&entry.stream_id));
        needs_redraw |= previous_count != entries.len() + previews.len();

        // 2. Redraw and submit overlay frame
        if needs_redraw {
            needs_redraw = false;

            let cards: Vec<VrSubtitleCard> = entries
                .iter()
                .chain(&previews)
                .filter(|entry| {
                    settings
                        .audio_source_filter
                        .allows(entry.audio_source, entry.is_typing)
                })
                .rev()
                .take(settings.max_items.clamp(1, 5))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|e| VrSubtitleCard {
                    source: e.source.clone(),
                    translated: e.translated.clone(),
                    additional_translations: e.additional_translations.clone(),
                    speaker: e.speaker.clone(),
                    live: e.live,
                })
                .collect();

            // Update status preview
            {
                let mut st = status.lock();
                st.cards.clone_from(&cards);
                st.active_card_count = cards.len();
                st.latest_caption_preview =
                    cards.last().map(|c| c.preview_text(settings.bilingual));
            }

            if let Some(overlay) = vr_overlay.as_mut() {
                let (width, height) = renderer.dimensions();
                match redraw_overlay(
                    overlay,
                    &mut renderer,
                    &cards,
                    &settings,
                    &mut is_overlay_visible,
                    |overlay, pixels| match graphics.as_mut() {
                        Some((_, texture)) => texture.upload(overlay, &pixels),
                        None => overlay.set_raw_rgba(pixels, width, height),
                    },
                ) {
                    Ok(()) => status.lock().last_error = None,
                    Err(error) => {
                        let lost = error.connection_lost();
                        let mut st = status.lock();
                        st.last_error = Some(error.to_string());
                        if lost {
                            // Destroy before shutdown. Captions remain available for reconnect.
                            voice_bridge.lock().stop();
                            voice = None;
                            st.interaction_available = false;
                            avatar = None;
                            vr_overlay = None;
                            graphics = None;
                            vr_session = None;
                            is_overlay_visible = false;
                            st.steamvr_connected = false;
                            st.avatar_available = false;
                            st.avatar_present = false;
                            st.avatar_attention = false;
                            st.avatar_greeting = false;
                        }
                    }
                }
            }
        }
        drop(pending);
        // Create lazily once per session: OpenVR retains its GPU resources until
        // shutdown. Toggling only suspends the same companion, never the captions.
        if settings.avatar_enabled || settings.interaction_enabled {
            if !avatar_initialized
                && let Some(session) = &vr_session
                && let Some((gpu, _)) = &graphics
            {
                avatar_initialized = true;
                match AvatarOverlay::new(session, gpu.clone()) {
                    Ok(created) => avatar = Some(created),
                    Err(error) => status.lock().avatar_error = Some(error),
                }
            }
            let mut st = status.lock();
            st.avatar_available = avatar.is_some() && st.avatar_error.is_none();
        } else {
            if let Some(current) = avatar.as_mut() {
                if let Err(error) = current.hide() {
                    status.lock().avatar_error = Some(error.to_string());
                }
            }
            let mut st = status.lock();
            st.avatar_available = false;
            st.avatar_present = false;
            st.avatar_attention = false;
            st.avatar_greeting = false;
        }
        if settings.interaction_enabled
            && !voice_initialized
            && let Some(session) = &vr_session
            && let Some((gpu, _)) = &graphics
        {
            voice_initialized = true;
            match VoiceOverlay::new(session, gpu.clone(), &input_directory) {
                Ok(created) => voice = Some(created),
                Err(error) => status.lock().interaction_error = Some(error),
            }
        }
        {
            let mut st = status.lock();
            st.interaction_available = settings.interaction_enabled
                && voice.is_some()
                && st.interaction_error.is_none()
                && st.avatar_available;
        }
        // Input and release cleanup keep ticking even when the companion has
        // faded out. Captions and host commands continue on this same worker.
        if Instant::now() >= next_avatar_frame {
            next_avatar_frame = Instant::now() + Duration::from_secs_f64(1.0 / 60.0);
            let (mut snapshot, dock_microphone) = {
                let mut bridge = voice_bridge.lock();
                (
                    bridge.snapshot.clone(),
                    std::mem::take(&mut bridge.dock_requested),
                )
            };
            snapshot.review_enabled &= settings.review_before_send;
            let active = settings.interaction_enabled
                && snapshot.enabled
                && status.lock().interaction_available;
            let presentation = companion.lock().clone();
            if status.lock().avatar_available
                && let Some(current) = avatar.as_mut()
            {
                current.freeze_controls(
                    active && voice.as_ref().is_some_and(VoiceOverlay::freeze_controls),
                );
                current
                    .voice_snapshot((active && snapshot.review_enabled).then(|| snapshot.clone()));
                let result = current.frame(&presentation);
                let mut st = status.lock();
                match result {
                    Ok(()) => {
                        st.avatar_present = current.visible();
                        st.avatar_attention = current.attentive();
                        st.avatar_greeting = current.greeting();
                    }
                    Err(error) => {
                        let _ = current.hide();
                        st.avatar_available = false;
                        st.avatar_present = false;
                        st.avatar_attention = false;
                        st.avatar_greeting = false;
                        st.avatar_error = Some(error.to_string());
                    }
                }
            }
            let control_root = active
                .then(|| avatar.as_ref().and_then(AvatarOverlay::control_root))
                .flatten();
            if let Some(objects) = voice.as_mut() {
                if dock_microphone {
                    objects.dock_microphone();
                }
                if control_root.is_none() {
                    objects.suspend();
                    voice_bridge.lock().stop();
                } else {
                    match objects.frame(control_root, &snapshot) {
                        Ok(actions) => {
                            let mut bridge = voice_bridge.lock();
                            for action in actions {
                                bridge.push(action);
                            }
                        }
                        Err(error) => {
                            objects.suspend();
                            voice_bridge.lock().stop();
                            let mut st = status.lock();
                            st.interaction_available = false;
                            st.interaction_error = Some(error.to_string());
                        }
                    }
                }
            } else {
                voice_bridge.lock().stop();
            }
        }
    }
}

fn configure_overlay(
    overlay: &OpenVrOverlay,
    settings: &VrOverlaySettings,
) -> Result<(), OverlayError> {
    overlay
        .set_auto_hmd_hud_transform(settings.distance_meters, settings.vertical_offset_meters)?;
    overlay.set_width(settings.overlay_width_meters)?;
    overlay.set_alpha(settings.opacity)
}

fn redraw_overlay(
    overlay: &mut OpenVrOverlay,
    renderer: &mut VrOverlayRenderer,
    cards: &[VrSubtitleCard],
    settings: &VrOverlaySettings,
    visible: &mut bool,
    upload: impl FnOnce(&mut OpenVrOverlay, Vec<u8>) -> Result<(), OverlayError>,
) -> Result<(), OverlayError> {
    if !settings.captions_enabled || cards.is_empty() {
        if *visible {
            overlay.hide()?;
            *visible = false;
        }
    } else {
        let buffer = renderer
            .render(cards, settings.bilingual, settings.font_size)
            .map_err(OverlayError::InvalidFrame)?;
        configure_overlay(overlay, settings)?;
        upload(overlay, buffer)?;
        if !*visible {
            overlay.show()?;
            *visible = true;
        }
    }
    Ok(())
}

fn upsert_caption(
    entries: &mut Vec<StreamEntry>,
    stream_id: u64,
    audio_source: CaptureSource,
    is_typing: bool,
    source: String,
    translated: String,
    additional_translations: Vec<String>,
    speaker: String,
    live: bool,
    now: Instant,
) {
    if source.trim().is_empty()
        && translated.trim().is_empty()
        && additional_translations.is_empty()
    {
        return;
    }
    let entry = StreamEntry {
        stream_id,
        audio_source,
        is_typing,
        source,
        translated,
        additional_translations,
        speaker,
        live,
        updated_at: now,
    };
    if let Some(current) = entries
        .iter_mut()
        .find(|e| e.stream_id == stream_id && e.live)
    {
        *current = entry;
    } else {
        entries.push(entry);
    }
}

fn expire_entries(entries: &mut Vec<StreamEntry>, now: Instant, timeout: f32) -> bool {
    let previous = entries.len();
    // Live captions refresh their own deadline on each revision. Lost stream-end
    // events and unrelated settings changes must not keep stale subtitles forever.
    entries
        .retain(|e| now.saturating_duration_since(e.updated_at).as_secs_f32() < timeout.max(0.1));
    entries.len() != previous
}

fn clamp_entries(entries: &mut Vec<StreamEntry>, max_items: usize) {
    let max = max_items.clamp(1, 5);
    if entries.len() > max {
        let excess = entries.len() - max;
        entries.drain(0..excess);
    }
}
