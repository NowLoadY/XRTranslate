//! Background runtime, worker loop, and settings for the SteamVR overlay plugin.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::avatar::AvatarOverlay;
use super::graphics;
use super::openvr::{OpenVrApi, OpenVrOverlay, OpenVrSession, OverlayError};
use super::renderer::{VrOverlayRenderer, VrSubtitleCard};
use crate::client_settings::CaptureSource;
use crate::session_coordinator::AudioSourceFilter;
use crate::ui::components::avatar::Presentation;

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
    pub last_error: Option<String>,
    pub active_card_count: usize,
    pub latest_caption_preview: Option<String>,
    pub cards: Vec<VrSubtitleCard>,
}

#[derive(Debug)]
pub enum VrCommand {
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
}

impl VrOverlayManager {
    pub fn new(settings: VrOverlaySettings) -> Self {
        let (command_tx, command_rx) = unbounded();
        let status = Arc::new(Mutex::new(VrRuntimeStatus::default()));
        let worker_status = Arc::clone(&status);

        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let companion = Arc::new(Mutex::new(Presentation::default()));
        let worker_companion = Arc::clone(&companion);
        let worker = std::thread::Builder::new()
            .name("vr-overlay-worker".into())
            .spawn(move || {
                run_vr_worker(
                    command_rx,
                    settings,
                    worker_status,
                    worker_shutdown,
                    worker_companion,
                )
            })
            .expect("failed to spawn VR overlay worker");

        Self {
            command_tx,
            status,
            shutdown,
            worker: Some(worker),
            companion,
        }
    }

    pub fn handle(&self) -> VrOverlayHandle {
        VrOverlayHandle {
            command_tx: self.command_tx.clone(),
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
}

impl VrOverlayHandle {
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
) {
    settings.normalize();
    let mut renderer = VrOverlayRenderer::new(RENDER_WIDTH, RENDER_HEIGHT)
        .expect("fixed subtitle dimensions are valid");
    let mut openvr_api: Option<Arc<OpenVrApi>> = OpenVrApi::try_load();
    let mut vr_session: Option<OpenVrSession> = None;
    let mut vr_overlay: Option<OpenVrOverlay> = None;
    let mut graphics = None;
    let mut avatar: Option<AvatarOverlay> = None;
    let mut avatar_initialized = false;
    let mut next_avatar_frame = Instant::now();
    let mut next_session_check = Instant::now();

    let mut entries: Vec<StreamEntry> = Vec::new();
    let mut needs_redraw = false;
    let mut is_overlay_visible = false;

    if let Some(api) = &openvr_api {
        status.lock().steamvr_installed = api.is_runtime_installed();
    }

    while !shutdown.load(Ordering::Acquire) {
        let timeout = if settings.avatar_enabled && status.lock().avatar_available {
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
                    if let Some(existing) = entries
                        .iter_mut()
                        .find(|e| e.stream_id == stream_id && e.live)
                    {
                        existing.live = false;
                        existing.updated_at = Instant::now();
                        needs_redraw = true;
                    }
                }
                Ok(VrCommand::Clear) => {
                    entries.clear();
                    needs_redraw = true;
                }
                Ok(VrCommand::UpdateSettings(mut updated)) => {
                    updated.normalize();
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

        // 2. Redraw and submit overlay frame
        if needs_redraw {
            needs_redraw = false;

            let cards: Vec<VrSubtitleCard> = entries
                .iter()
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
        // Create lazily once per session: OpenVR retains its GPU resources until
        // shutdown. Toggling only suspends the same companion, never the captions.
        if settings.avatar_enabled {
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
        // Captions have already drained and rendered; companion dialogue timing
        // never gates subtitle consumption. Avoid command bursts starving frames.
        if status.lock().avatar_available && Instant::now() >= next_avatar_frame {
            // One start-to-start clock; native/GPU waits must not add another frame.
            next_avatar_frame = Instant::now() + Duration::from_secs_f64(1.0 / 60.0);
            let presentation = companion.lock().clone();
            if let Some(current) = avatar.as_mut() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_settings_normalize_before_geometry_or_expiry() {
        let mut s = VrOverlaySettings {
            max_items: usize::MAX,
            font_size: f32::NAN,
            opacity: f32::INFINITY,
            distance_meters: f32::NEG_INFINITY,
            vertical_offset_meters: f32::NAN,
            pitch_degrees: f32::NAN,
            overlay_width_meters: 0.0,
            display_timeout_seconds: f32::INFINITY,
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.max_items, 5);
        assert_eq!(s.font_size, 20.0);
        assert_eq!(s.opacity, 0.85);
        assert_eq!(s.overlay_width_meters, 0.2);
        assert_eq!(s.display_timeout_seconds, 12.0);
        assert!(s.distance_meters.is_finite() && s.vertical_offset_meters.is_finite());
        let once = s.clone();
        s.normalize();
        assert_eq!(s, once);
    }

    #[test]
    fn failed_show_and_hide_never_change_visibility_and_can_retry() {
        use super::super::openvr::tests::{fail_operation, fake_session};
        let (_lock, session) = fake_session();
        let mut overlay = session.create_overlay("test", "test").unwrap();
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let cards = [VrSubtitleCard {
            source: "text".into(),
            translated: String::new(),
            additional_translations: Vec::new(),
            speaker: String::new(),
            live: true,
        }];
        let mut visible = false;
        fail_operation("ShowOverlay", 23);
        assert!(
            redraw_overlay(
                &mut overlay,
                &mut renderer,
                &cards,
                &VrOverlaySettings::default(),
                &mut visible,
                |_, _| Ok(()),
            )
            .is_err()
        );
        assert!(!visible);
        fail_operation("Unused", 23);
        redraw_overlay(
            &mut overlay,
            &mut renderer,
            &cards,
            &VrOverlaySettings::default(),
            &mut visible,
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(visible);
        fail_operation("HideOverlay", 23);
        assert!(
            redraw_overlay(
                &mut overlay,
                &mut renderer,
                &[],
                &VrOverlaySettings::default(),
                &mut visible,
                |_, _| Ok(()),
            )
            .is_err()
        );
        assert!(visible);
        fail_operation("Unused", 23);
        redraw_overlay(
            &mut overlay,
            &mut renderer,
            &[],
            &VrOverlaySettings::default(),
            &mut visible,
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(!visible);
    }

    #[test]
    fn rapid_settings_captions_and_disconnect_shutdown_join_worker() {
        let manager = VrOverlayManager::new(VrOverlaySettings::default());
        let handle = manager.handle();
        let status = Arc::clone(&manager.status);
        let stopped = Arc::clone(&manager.shutdown);
        let producer = std::thread::spawn(move || {
            for i in 0..500 {
                handle.add_caption(
                    i,
                    CaptureSource::Microphone,
                    false,
                    "caption",
                    "字幕",
                    &[],
                    "",
                    true,
                );
            }
            handle
        });
        for i in 0..100 {
            manager.update_settings(VrOverlaySettings {
                max_items: i % 5 + 1,
                bilingual: i % 2 == 0,
                ..Default::default()
            });
            manager.disconnect();
        }
        let handle = producer.join().unwrap();
        drop(manager);
        assert!(stopped.load(Ordering::Acquire));
        assert!(!status.lock().steamvr_connected);
        assert!(
            handle.command_tx.send(VrCommand::Clear).is_err(),
            "receiver must be released before drop returns"
        );
    }

    #[test]
    fn revisions_rollover_and_concurrent_streams_keep_one_live_card_each() {
        let now = Instant::now();
        let mut entries = Vec::new();
        upsert_caption(
            &mut entries,
            1,
            CaptureSource::Microphone,
            false,
            "partial".into(),
            "初稿".into(),
            Vec::new(),
            "A".into(),
            true,
            now,
        );
        upsert_caption(
            &mut entries,
            2,
            CaptureSource::Microphone,
            false,
            "other".into(),
            "另一任务".into(),
            Vec::new(),
            "B".into(),
            true,
            now,
        );
        for i in 0..100 {
            upsert_caption(
                &mut entries,
                1,
                CaptureSource::Microphone,
                false,
                format!("revision {i}"),
                "更新".into(),
                Vec::new(),
                "A".into(),
                true,
                now,
            );
        }
        assert_eq!(entries.len(), 2);
        // Finalized correction replaces the live card, not a duplicated rollover.
        upsert_caption(
            &mut entries,
            1,
            CaptureSource::Microphone,
            false,
            "final".into(),
            "定稿".into(),
            Vec::new(),
            "A".into(),
            false,
            now,
        );
        upsert_caption(
            &mut entries,
            1,
            CaptureSource::Microphone,
            false,
            "next".into(),
            "下一句".into(),
            Vec::new(),
            "A".into(),
            true,
            now,
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.iter().filter(|e| e.live).count(), 2);
        assert_eq!(entries[0].source, "final");
        assert_eq!(entries[1].stream_id, 2);
    }

    #[test]
    fn deadlines_expire_during_updates_and_lost_end_events() {
        let start = Instant::now();
        let mut entries = Vec::new();
        upsert_caption(
            &mut entries,
            1,
            CaptureSource::Microphone,
            false,
            "stale live".into(),
            String::new(),
            Vec::new(),
            String::new(),
            true,
            start,
        );
        upsert_caption(
            &mut entries,
            2,
            CaptureSource::Microphone,
            false,
            "history".into(),
            String::new(),
            Vec::new(),
            String::new(),
            false,
            start,
        );
        upsert_caption(
            &mut entries,
            3,
            CaptureSource::Microphone,
            false,
            "fresh".into(),
            String::new(),
            Vec::new(),
            String::new(),
            true,
            start + Duration::from_secs(4),
        );
        assert!(expire_entries(
            &mut entries,
            start + Duration::from_secs(5),
            5.0
        ));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].stream_id, 3);
    }

    #[test]
    fn vr_overlay_settings_has_expected_defaults() {
        let settings = VrOverlaySettings::default();
        assert!(settings.enabled);
        assert!(!settings.avatar_enabled);
        assert!(settings.captions_enabled);
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy.as_object_mut().unwrap().remove("avatar_enabled");
        legacy.as_object_mut().unwrap().remove("captions_enabled");
        assert_eq!(
            serde_json::from_value::<VrOverlaySettings>(legacy).unwrap(),
            settings
        );
        assert_eq!(settings.max_items, 3);
        assert!(settings.bilingual);
        assert_eq!(settings.font_size, 20.0);
        assert_eq!(settings.distance_meters, 1.0);
        assert_eq!(settings.vertical_offset_meters, 0.0);
        assert_eq!(settings.overlay_width_meters, 0.5);
    }

    #[test]
    fn clamp_entries_bounds_list_size() {
        let mut entries = Vec::new();
        for i in 0..10 {
            entries.push(StreamEntry {
                stream_id: i,
                audio_source: CaptureSource::Microphone,
                is_typing: false,
                source: format!("source {i}"),
                translated: format!("trans {i}"),
                additional_translations: Vec::new(),
                speaker: String::new(),
                live: false,
                updated_at: Instant::now(),
            });
        }
        clamp_entries(&mut entries, 3);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].stream_id, 7);
        assert_eq!(entries[2].stream_id, 9);
    }

    #[test]
    fn renderer_handles_multiple_cards_cleanly() {
        let mut renderer = VrOverlayRenderer::new(640, 320).unwrap();
        let cards = vec![
            VrSubtitleCard {
                source: "Hello world".into(),
                translated: "你好，世界".into(),
                additional_translations: Vec::new(),
                speaker: "User1".into(),
                live: false,
            },
            VrSubtitleCard {
                source: "Second sentence".into(),
                translated: "第二句话".into(),
                additional_translations: Vec::new(),
                speaker: String::new(),
                live: true,
            },
        ];
        let buf = renderer.render(&cards, true, 20.0).unwrap();
        assert_eq!(buf.len(), 640 * 320 * 4);
    }

    #[test]
    fn manager_connect_and_disconnect_lifecycle() {
        let manager = VrOverlayManager::new(VrOverlaySettings::default());

        // Wait for the worker result, not a 50 ms guess at native startup time.
        manager.connect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !manager.status().steamvr_connected && manager.status().last_error.is_none() {
            assert!(Instant::now() < deadline, "connect did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }

        // Disconnect request via manager
        manager.disconnect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.status().steamvr_connected {
            assert!(Instant::now() < deadline, "disconnect did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!manager.status().steamvr_connected);
    }
}
