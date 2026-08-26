//! Background runtime, worker loop, and settings for the SteamVR overlay plugin.

use std::sync::Arc;
use std::time::{Duration, Instant};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::openvr::{OpenVrApi, OpenVrOverlay, OpenVrSession};
use super::renderer::{VrOverlayRenderer, VrSubtitleCard};

const RENDER_WIDTH: u32 = 1024;
const RENDER_HEIGHT: u32 = 512;
const STEAMVR_RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VrOverlaySettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
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

    #[allow(dead_code)]
    pub fn auto_pitch_degrees(&self) -> f32 {
        (-self.vertical_offset_meters).atan2(self.distance_meters.max(0.1)).to_degrees()
    }
}

impl Default for VrOverlaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
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
    pub last_error: Option<String>,
    pub active_card_count: usize,
    pub latest_caption_preview: Option<String>,
}

#[derive(Debug)]
pub enum VrCommand {
    Caption {
        stream_id: u64,
        source: String,
        translated: String,
        speaker: String,
        live: bool,
    },
    RollStream {
        stream_id: u64,
        source: String,
        translated: String,
        speaker: String,
    },
    EndStream(u64),
    Clear,
    UpdateSettings(VrOverlaySettings),
    Shutdown,
}

#[derive(Clone, Debug)]
struct StreamEntry {
    stream_id: u64,
    source: String,
    translated: String,
    speaker: String,
    live: bool,
    updated_at: Instant,
}

pub struct VrOverlayManager {
    command_tx: Sender<VrCommand>,
    status: Arc<Mutex<VrRuntimeStatus>>,
}

impl VrOverlayManager {
    pub fn new(settings: VrOverlaySettings) -> Self {
        let (command_tx, command_rx) = unbounded();
        let status = Arc::new(Mutex::new(VrRuntimeStatus::default()));
        let worker_status = Arc::clone(&status);

        std::thread::Builder::new()
            .name("vr-overlay-worker".into())
            .spawn(move || run_vr_worker(command_rx, settings, worker_status))
            .expect("failed to spawn VR overlay worker");

        Self { command_tx, status }
    }

    pub fn handle(&self) -> VrOverlayHandle {
        VrOverlayHandle {
            command_tx: self.command_tx.clone(),
        }
    }

    pub fn status(&self) -> VrRuntimeStatus {
        self.status.lock().clone()
    }

    pub fn update_settings(&self, settings: VrOverlaySettings) {
        let _ = self.command_tx.send(VrCommand::UpdateSettings(settings));
    }
}

impl Drop for VrOverlayManager {
    fn drop(&mut self) {
        let _ = self.command_tx.send(VrCommand::Shutdown);
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
        source: &str,
        translated: &str,
        speaker: &str,
        live: bool,
    ) {
        let _ = self.command_tx.send(VrCommand::Caption {
            stream_id,
            source: source.to_owned(),
            translated: translated.to_owned(),
            speaker: speaker.to_owned(),
            live,
        });
    }

    pub fn roll_stream(
        &self,
        stream_id: u64,
        source: &str,
        translated: &str,
        speaker: &str,
    ) {
        let _ = self.command_tx.send(VrCommand::RollStream {
            stream_id,
            source: source.to_owned(),
            translated: translated.to_owned(),
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

fn run_vr_worker(
    command_rx: Receiver<VrCommand>,
    mut settings: VrOverlaySettings,
    status: Arc<Mutex<VrRuntimeStatus>>,
) {
    let renderer = VrOverlayRenderer::new(RENDER_WIDTH, RENDER_HEIGHT);
    let mut openvr_api: Option<Arc<OpenVrApi>> = OpenVrApi::try_load();
    let mut vr_session: Option<OpenVrSession> = None;
    let mut vr_overlay: Option<OpenVrOverlay> = None;

    let mut entries: Vec<StreamEntry> = Vec::new();
    let mut last_reconnect_attempt = Instant::now() - STEAMVR_RECONNECT_INTERVAL;
    let mut needs_redraw = false;
    let mut is_overlay_visible = false;

    loop {
        let timeout = Duration::from_millis(100);
        match command_rx.recv_timeout(timeout) {
            Ok(VrCommand::Caption {
                stream_id,
                source,
                translated,
                speaker,
                live,
            }) => {
                if live {
                    // Ongoing speech turn: update currently live turn, or create new turn
                    if let Some(existing) = entries
                        .iter_mut()
                        .find(|e| e.stream_id == stream_id && e.live)
                    {
                        existing.source = source;
                        existing.translated = translated;
                        existing.speaker = speaker;
                        existing.updated_at = Instant::now();
                    } else {
                        entries.push(StreamEntry {
                            stream_id,
                            source,
                            translated,
                            speaker,
                            live: true,
                            updated_at: Instant::now(),
                        });
                    }
                } else {
                    // Finalized/static caption: finalize currently live turn or append static
                    if let Some(existing) = entries
                        .iter_mut()
                        .find(|e| e.stream_id == stream_id && e.live)
                    {
                        existing.source = source;
                        existing.translated = translated;
                        existing.speaker = speaker;
                        existing.live = false;
                        existing.updated_at = Instant::now();
                    } else {
                        entries.push(StreamEntry {
                            stream_id,
                            source,
                            translated,
                            speaker,
                            live: false,
                            updated_at: Instant::now(),
                        });
                    }
                }
                clamp_entries(&mut entries, settings.max_items);
                needs_redraw = true;
            }
            Ok(VrCommand::RollStream {
                stream_id,
                source,
                translated,
                speaker,
            }) => {
                if let Some(existing) = entries
                    .iter_mut()
                    .find(|e| e.stream_id == stream_id && e.live)
                {
                    existing.live = false;
                    existing.updated_at = Instant::now();
                }
                entries.push(StreamEntry {
                    stream_id,
                    source,
                    translated,
                    speaker,
                    live: false,
                    updated_at: Instant::now(),
                });
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
            Ok(VrCommand::UpdateSettings(updated)) => {
                let enabled_toggled = settings.enabled != updated.enabled;
                settings = updated;
                if enabled_toggled && !settings.enabled {
                    if let Some(overlay) = &vr_overlay {
                        overlay.hide();
                    }
                    is_overlay_visible = false;
                }
                // Refresh updated_at on active entries so adjusting settings keeps subtitles visible
                let now = Instant::now();
                for entry in &mut entries {
                    entry.updated_at = now;
                }
                clamp_entries(&mut entries, settings.max_items);
                needs_redraw = true;
            }
            Ok(VrCommand::Shutdown) => {
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Timeout expiry check
                let now = Instant::now();
                let prev_len = entries.len();
                entries.retain(|e| {
                    e.live
                        || now.duration_since(e.updated_at).as_secs_f32()
                            < settings.display_timeout_seconds
                });
                if entries.len() != prev_len {
                    needs_redraw = true;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                break;
            }
        }

        // 1. Maintain SteamVR Connection
        if settings.enabled {
            if openvr_api.is_none() && last_reconnect_attempt.elapsed() >= STEAMVR_RECONNECT_INTERVAL {
                last_reconnect_attempt = Instant::now();
                openvr_api = OpenVrApi::try_load();
            }

            if let Some(api) = &openvr_api {
                let installed = api.is_runtime_installed();
                {
                    let mut st = status.lock();
                    st.steamvr_installed = installed;
                }

                if vr_session.is_none()
                    && installed
                    && last_reconnect_attempt.elapsed() >= STEAMVR_RECONNECT_INTERVAL
                {
                    last_reconnect_attempt = Instant::now();
                    match api.init_overlay() {
                        Ok(session) => match session.create_overlay("xrtranslate.vr_overlay", "XRTranslate Subtitles") {
                            Ok(overlay) => {
                                overlay.set_auto_hmd_hud_transform(
                                    settings.distance_meters,
                                    settings.vertical_offset_meters,
                                );
                                overlay.set_width(settings.overlay_width_meters);
                                overlay.set_alpha(settings.opacity);
                                vr_overlay = Some(overlay);
                                vr_session = Some(session);
                                needs_redraw = true;
                                let mut st = status.lock();
                                st.steamvr_connected = true;
                                st.last_error = None;
                            }
                            Err(e) => {
                                let mut st = status.lock();
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
        } else {
            vr_overlay = None;
            vr_session = None;
            let mut st = status.lock();
            st.steamvr_connected = false;
        }

        // 2. Redraw and submit overlay frame
        if needs_redraw {
            needs_redraw = false;

            let cards: Vec<VrSubtitleCard> = entries
                .iter()
                .map(|e| VrSubtitleCard {
                    source: e.source.clone(),
                    translated: e.translated.clone(),
                    speaker: e.speaker.clone(),
                    live: e.live,
                })
                .collect();

            // Update status preview
            {
                let mut st = status.lock();
                st.active_card_count = cards.len();
                st.latest_caption_preview = cards.last().map(|c| {
                    if c.translated.is_empty() {
                        c.source.clone()
                    } else {
                        format!("{} | {}", c.source, c.translated)
                    }
                });
            }

            if let Some(overlay) = &vr_overlay {
                if cards.is_empty() {
                    if is_overlay_visible {
                        overlay.hide();
                        is_overlay_visible = false;
                    }
                } else {
                    let rgba_buffer = renderer.render(
                        &cards,
                        settings.bilingual,
                        settings.font_size,
                        settings.opacity,
                    );
                    overlay.set_auto_hmd_hud_transform(
                        settings.distance_meters,
                        settings.vertical_offset_meters,
                    );
                    overlay.set_width(settings.overlay_width_meters);
                    overlay.set_alpha(settings.opacity);

                    if let Err(e) = overlay.set_raw_rgba(&rgba_buffer, RENDER_WIDTH, RENDER_HEIGHT) {
                        let mut st = status.lock();
                        st.last_error = Some(e);
                    } else if !is_overlay_visible {
                        overlay.show();
                        is_overlay_visible = true;
                    }
                }
            }
        }
    }
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
    fn vr_overlay_settings_has_expected_defaults() {
        let settings = VrOverlaySettings::default();
        assert!(settings.enabled);
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
                source: format!("source {i}"),
                translated: format!("trans {i}"),
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
        let renderer = VrOverlayRenderer::new(1024, 512);
        let cards = vec![
            VrSubtitleCard {
                source: "Hello world".into(),
                translated: "你好，世界".into(),
                speaker: "User1".into(),
                live: false,
            },
            VrSubtitleCard {
                source: "Second sentence".into(),
                translated: "第二句话".into(),
                speaker: String::new(),
                live: true,
            },
        ];
        let buf = renderer.render(&cards, true, 20.0, 0.85);
        assert_eq!(buf.len(), 1024 * 512 * 4);
    }
}
