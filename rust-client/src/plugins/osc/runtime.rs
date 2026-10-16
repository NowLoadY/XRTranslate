use super::chatbox::{
    HistoryMessage, ManualMessage, build_chatbox_text, render_entry, sanitize_chatbox_segment,
};
use crate::client_settings::CaptureSource;
pub use crate::plugins::text_output::{
    BannerConfig, BannerContentType, MessageFormat as OscFormatMode,
};
use crate::plugins::text_output::{LyricsText, TextOutputHandle};
use crate::session_coordinator::AudioSourceFilter;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;
use rosc::{OscType, encoder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// VRChat recommends 1.5 seconds between Chatbox updates. Coalesce revisions
// during this interval; both native outputs use the same quiet cadence.
const COOLDOWN: Duration = Duration::from_millis(1500);
const BANNER_REFRESH_INTERVAL: Duration = Duration::from_millis(1050);
pub const MAX_PREFIX_LENGTH: usize = 24;

/// Acknowledges local UDP delivery, not receipt by the game.
pub(crate) type VoicePublishReply = Receiver<Result<(), String>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OscInputSource {
    #[default]
    Unknown,
    Microphone,
    SystemAudio,
    Typing,
}

impl From<CaptureSource> for OscInputSource {
    fn from(source: CaptureSource) -> Self {
        match source {
            CaptureSource::Microphone => Self::Microphone,
            CaptureSource::SystemAudio => Self::SystemAudio,
            CaptureSource::Both => Self::Unknown,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OscMessageSeparator {
    #[default]
    NewLine,
    Space,
}

impl OscMessageSeparator {
    pub fn label(self, language: crate::i18n::UiLanguage) -> &'static str {
        match self {
            Self::NewLine => crate::i18n::tr(language, "New line"),
            Self::Space => crate::i18n::tr(language, "Same line"),
        }
    }

    pub fn layout_label(
        self,
        language: crate::i18n::UiLanguage,
        target_only: bool,
    ) -> &'static str {
        if target_only {
            return self.label(language);
        }
        crate::i18n::tr(
            language,
            match self {
                Self::NewLine => "Separate",
                Self::Space => "Merge",
            },
        )
    }

    pub(super) const fn value(self) -> &'static str {
        match self {
            Self::NewLine => "\n",
            Self::Space => " ",
        }
    }
}

fn default_typing_source_lang() -> String {
    "zh".to_string()
}

fn default_typing_target_lang() -> String {
    "en".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscSettings {
    #[serde(skip)]
    pub game: super::OscGame,
    pub enabled: bool,
    #[serde(default)]
    pub audio_source_filter: AudioSourceFilter,
    pub ip: String,
    pub send_port: u16,
    pub listen_port: u16,
    pub max_text_length: usize,
    pub history_ttl_seconds: f64,
    pub header_config: BannerConfig,
    pub footer_config: BannerConfig,
    /// Positive values delay lyrics; negative values advance them.
    #[serde(default = "default_lyric_offset_seconds")]
    pub lyric_offset_seconds: f64,
    /// Keeps configured header/footer content visible after message TTL expiry.
    #[serde(default)]
    pub persistent_banners: bool,
    pub format_mode: OscFormatMode,
    #[serde(default)]
    pub message_separator: OscMessageSeparator,
    pub show_speaker_number: bool,
    // Persisted compatibility only. OSC's composer edits the host language route.
    #[serde(default = "default_typing_source_lang")]
    pub typing_source_lang: String,
    #[serde(default = "default_typing_target_lang")]
    pub typing_target_lang: String,
    #[serde(default = "default_microphone_prefix")]
    pub microphone_prefix: String,
    #[serde(default = "default_system_audio_prefix")]
    pub system_audio_prefix: String,
    #[serde(default = "default_typing_prefix")]
    pub typing_prefix: String,
}

fn default_microphone_prefix() -> String {
    "🎤".into()
}
fn default_system_audio_prefix() -> String {
    "🔊".into()
}
fn default_typing_prefix() -> String {
    "💬".into()
}

pub(super) fn default_lyric_offset_seconds() -> f64 {
    // A small initial allowance for remote voice playback, adjustable per setup.
    0.3
}

impl Default for OscSettings {
    fn default() -> Self {
        Self {
            game: super::OscGame::VRChat,
            enabled: true,
            audio_source_filter: AudioSourceFilter::default(),
            ip: "127.0.0.1".into(),
            send_port: 9000,
            listen_port: 9001,
            max_text_length: 144,
            history_ttl_seconds: 15.0,
            header_config: BannerConfig::default(),
            footer_config: BannerConfig::default(),
            lyric_offset_seconds: default_lyric_offset_seconds(),
            persistent_banners: false,
            format_mode: OscFormatMode::BilingualSourceFirst,
            message_separator: OscMessageSeparator::default(),
            show_speaker_number: false,
            typing_source_lang: default_typing_source_lang(),
            typing_target_lang: default_typing_target_lang(),
            microphone_prefix: default_microphone_prefix(),
            system_audio_prefix: default_system_audio_prefix(),
            typing_prefix: default_typing_prefix(),
        }
    }
}

#[derive(Default, Deserialize)]
struct ProjectConfig {
    osc: Option<OscConfig>,
    frontend: Option<FrontendConfig>,
}
#[derive(Default, Deserialize)]
struct FrontendConfig {
    show_speaker: Option<bool>,
}
#[derive(Default, Deserialize)]
struct OscConfig {
    enabled: Option<bool>,
    ip: Option<String>,
    send_port: Option<u16>,
    listen_port: Option<u16>,
    max_text_length: Option<usize>,
    history_ttl_seconds: Option<f64>,
}

impl OscSettings {
    pub fn for_game(game: super::OscGame) -> Self {
        Self {
            game,
            ..Self::default()
        }
    }

    pub fn text_limit(&self) -> usize {
        self.max_text_length.min(self.game.max_text_length())
    }

    pub fn line_limit(&self) -> usize {
        match self.game {
            super::OscGame::VRChat => 9,
            super::OscGame::ChilloutVR => self.text_limit(),
        }
    }

    pub(crate) fn prepare_voice_text(&self, text: &str) -> Result<String, String> {
        let text = sanitize_chatbox_segment(text);
        self.validate_voice_text(&text)?;
        Ok(text)
    }

    fn validate_voice_text(&self, text: &str) -> Result<(), String> {
        if !self.enabled {
            return Err("OSC Chatbox output is disabled".into());
        }
        if text.trim().is_empty() {
            return Err("There is no voice text to send".into());
        }
        let length = text.chars().count();
        let limit = self.text_limit();
        if length > limit {
            return Err(format!(
                "Voice text has {length} characters; the Chatbox limit is {limit}. Shorten it before sending."
            ));
        }
        let line_limit = self.line_limit();
        if text.lines().count() > line_limit {
            return Err(format!(
                "Voice text exceeds the Chatbox limit of {line_limit} lines. Shorten it before sending."
            ));
        }
        Ok(())
    }

    /// Reserve at most one third of the chatbox for lyrics, capped at 48
    /// characters. The remaining space belongs to speech and other banners.
    pub(crate) fn lyric_budget(&self) -> Option<usize> {
        let slots = [&self.header_config, &self.footer_config]
            .into_iter()
            .filter(|banner| banner.content_type == BannerContentType::Lyrics)
            .count();
        (self.enabled && slots > 0).then(|| (self.text_limit() / 3).min(48) / slots)
    }

    pub(crate) fn lyric_offset_ms(&self) -> i32 {
        (self.lyric_offset_seconds.clamp(-1.0, 1.0) * 1000.0).round() as i32
    }

    /// Retains the established config.json OSC section, but only the native client reads it.
    pub fn from_project_config() -> Self {
        let mut settings = Self::default();
        for path in project_config_candidates() {
            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(config) = serde_json::from_str::<ProjectConfig>(&contents) else {
                continue;
            };
            if let Some(value) = config.frontend.and_then(|frontend| frontend.show_speaker) {
                settings.show_speaker_number = value;
            }
            let Some(config) = config.osc else { continue };
            if let Some(value) = config.enabled {
                settings.enabled = value;
            }
            if let Some(value) = config.ip.filter(|v| !v.trim().is_empty()) {
                settings.ip = value;
            }
            if let Some(value) = config.send_port {
                settings.send_port = value;
            }
            if let Some(value) = config.listen_port {
                settings.listen_port = value;
            }
            if let Some(value) = config.max_text_length {
                settings.max_text_length = value;
            }
            if let Some(value) = config.history_ttl_seconds {
                settings.history_ttl_seconds = value;
            }
            break;
        }
        settings
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.ip.trim().is_empty() {
            return Err("OSC IP address cannot be empty".into());
        }
        if !(1..=self.game.max_text_length()).contains(&self.max_text_length) {
            return Err(format!(
                "OSC text budget must be between 1 and {}",
                self.game.max_text_length()
            ));
        }
        if !self.history_ttl_seconds.is_finite() || self.history_ttl_seconds < 0.0 {
            return Err("OSC history TTL must be non-negative".into());
        }
        if self.microphone_prefix.chars().count() > MAX_PREFIX_LENGTH
            || self.system_audio_prefix.chars().count() > MAX_PREFIX_LENGTH
            || self.typing_prefix.chars().count() > MAX_PREFIX_LENGTH
        {
            return Err(format!(
                "OSC prefixes must be at most {MAX_PREFIX_LENGTH} characters"
            ));
        }
        Ok(())
    }

    pub fn prefix_for(&self, source: OscInputSource) -> &str {
        match source {
            OscInputSource::Microphone => &self.microphone_prefix,
            OscInputSource::SystemAudio => &self.system_audio_prefix,
            OscInputSource::Typing => &self.typing_prefix,
            OscInputSource::Unknown => "",
        }
    }

    fn accepts_source(&self, source: OscInputSource) -> bool {
        match source {
            OscInputSource::Microphone => self
                .audio_source_filter
                .allows(CaptureSource::Microphone, false),
            OscInputSource::SystemAudio => self
                .audio_source_filter
                .allows(CaptureSource::SystemAudio, false),
            OscInputSource::Unknown => self.audio_source_filter.allows(CaptureSource::Both, false),
            OscInputSource::Typing => true,
        }
    }
}

fn project_config_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for start in [std::env::current_dir().ok(), std::env::current_exe().ok()] {
        let Some(start) = start else { continue };
        let directory = if start.is_dir() {
            start
        } else {
            start.parent().map(PathBuf::from).unwrap_or(start)
        };
        for ancestor in directory.ancestors() {
            let path = ancestor.join("config.json");
            if path.exists() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

struct QueuedMessage {
    text: String,
    typing: bool,
    notify: bool,
    expires_at: Option<Instant>,
}

struct VoicePublication {
    operation: u64,
    review_generation: u64,
    text: String,
    reply: Sender<Result<(), String>>,
}

struct PublishedVoice {
    text: String,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct MicrophoneReview {
    enabled: bool,
    generation: u64,
    last_exit_generation: u64,
}

impl MicrophoneReview {
    fn accepts(&self, source: OscInputSource, generation: u64) -> bool {
        source != OscInputSource::Microphone || (!self.enabled && self.generation == generation)
    }
}

enum Command {
    Lyrics(LyricsText),
    Previews(
        u64,
        bool,
        Vec<crate::session_coordinator::TranslationPreview>,
        u64,
    ),
    Message {
        stream_id: u64,
        audio_source: OscInputSource,
        microphone_generation: u64,
        is_typing: bool,
        source: String,
        translated: String,
        additional_translations: Vec<(String, String)>,
        speaker_id: String,
        ongoing: bool,
        /// Optional lifetime for this message.
        ttl: Option<Duration>,
    },
    ManualMessage {
        text: String,
        ttl: Option<Duration>,
    },
    PublishVoice(VoicePublication),
    RollStream {
        stream_id: u64,
        audio_source: OscInputSource,
        microphone_generation: u64,
        is_typing: bool,
        source: String,
        translated: String,
        speaker_id: String,
    },
    EndStream(u64),
    CancelStream(u64),
    MicrophoneReviewChanged,
    Clear,
    Update(OscSettings),
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct OscHandle {
    tx: Sender<Command>,
    pending_cancellations: Arc<Mutex<BTreeSet<u64>>>,
    microphone_review: Arc<Mutex<MicrophoneReview>>,
}

impl OscHandle {
    pub(crate) fn set_microphone_review(&self, enabled: bool) {
        let mut review = self.microphone_review.lock();
        if review.enabled == enabled {
            return;
        }
        review.enabled = enabled;
        review.generation = review.generation.wrapping_add(1);
        if !enabled {
            review.last_exit_generation = review.generation;
        }
        // Wake the worker even when no caption or expiry is pending. The shared
        // state also fences output already queued before this mode change.
        let _ = self.tx.send(Command::MicrophoneReviewChanged);
    }

    pub(crate) fn set_lyrics(&self, text: LyricsText) {
        let _ = self.tx.send(Command::Lyrics(text));
    }

    pub fn set_previews(
        &self,
        stream: u64,
        is_typing: bool,
        entries: &[crate::session_coordinator::TranslationPreview],
    ) {
        let generation = self.microphone_review.lock().generation;
        let _ = self.tx.send(Command::Previews(
            stream,
            is_typing,
            entries.to_vec(),
            generation,
        ));
    }

    pub fn add_message_for_stream(
        &self,
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &str,
        translated: &str,
        additional_translations: &[xrtranslate_protocol::AdditionalTranslation],
        speaker_id: &str,
        ongoing: bool,
    ) {
        let microphone_generation = self.microphone_review.lock().generation;
        let _ = self.tx.send(Command::Message {
            stream_id,
            audio_source: if is_typing {
                OscInputSource::Typing
            } else {
                audio_source.into()
            },
            microphone_generation,
            is_typing,
            source: sanitize_chatbox_segment(source),
            translated: sanitize_chatbox_segment(translated),
            additional_translations: additional_translations
                .iter()
                .filter(|result| !result.translated_text.trim().is_empty())
                .map(|result| {
                    (
                        result.target_lang.clone(),
                        sanitize_chatbox_segment(&result.translated_text),
                    )
                })
                .collect(),
            speaker_id: speaker_id.trim().into(),
            ongoing,
            ttl: None,
        });
    }

    pub fn send_manual_message(&self, text: &str) {
        let _ = self.tx.send(Command::ManualMessage {
            text: text.trim().into(),
            ttl: None,
        });
    }

    /// Publishes an already prepared body exactly once for this monotonic
    /// operation number. A single pending publication bounds explicit work.
    pub(crate) fn publish_voice(&self, operation: u64, text: &str) -> VoicePublishReply {
        let (reply, receiver) = bounded(1);
        let review_generation = self.microphone_review.lock().generation;
        if let Err(error) = self.tx.send(Command::PublishVoice(VoicePublication {
            operation,
            review_generation,
            text: text.to_owned(),
            reply,
        })) && let Command::PublishVoice(publication) = error.0
        {
            let _ = publication.reply.send(Err("OSC output has stopped".into()));
        }
        receiver
    }

    pub fn cancel_stream(&self, stream_id: u64) {
        // Fence immediately, before the dispatcher reaches this queued command.
        self.pending_cancellations.lock().insert(stream_id);
        let _ = self.tx.send(Command::CancelStream(stream_id));
    }

    pub fn end_stream_for(&self, stream_id: u64) {
        let _ = self.tx.send(Command::EndStream(stream_id));
    }

    #[allow(dead_code)] // Legacy next-caption rollover API; host events finalize via Message.
    pub fn roll_stream_for(
        &self,
        stream_id: u64,
        audio_source: CaptureSource,
        is_typing: bool,
        source: &str,
        translated: &str,
        speaker_id: &str,
    ) {
        let microphone_generation = self.microphone_review.lock().generation;
        let _ = self.tx.send(Command::RollStream {
            stream_id,
            audio_source: if is_typing {
                OscInputSource::Typing
            } else {
                audio_source.into()
            },
            microphone_generation,
            is_typing,
            source: sanitize_chatbox_segment(source),
            translated: sanitize_chatbox_segment(translated),
            speaker_id: speaker_id.trim().into(),
        });
    }

    pub fn clear_chatbox(&self) {
        let _ = self.tx.send(Command::Clear);
    }
}

#[derive(Default)]
pub(super) struct RuntimeStatus {
    pub(super) last_error: Option<String>,
    chatbox_text: String,
    chatbox_typing: bool,
    next_message_expires_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct ChatboxPreview {
    pub text: String,
    pub typing: bool,
    pub next_message_expires_in: Option<Duration>,
}

/// One game's coalescing Chatbox writer. Receive/control has a separate owner.
pub struct OscManager {
    status: Arc<Mutex<RuntimeStatus>>,
    tx: Sender<Command>,
    worker: Option<JoinHandle<()>>,
    send_permitted: Arc<Mutex<bool>>,
    pending_cancellations: Arc<Mutex<BTreeSet<u64>>>,
    microphone_review: Arc<Mutex<MicrophoneReview>>,
}

impl OscManager {
    pub fn new(settings: OscSettings, text_output: TextOutputHandle) -> Self {
        let (tx, rx) = unbounded();
        let status = Arc::new(Mutex::new(RuntimeStatus::default()));
        let dispatch_settings = settings.clone();
        let send_permitted = Arc::new(Mutex::new(true));
        let dispatch_permission = send_permitted.clone();
        let dispatch_status = Arc::clone(&status);
        let pending_cancellations = Arc::new(Mutex::new(BTreeSet::new()));
        let dispatch_cancellations = pending_cancellations.clone();
        let microphone_review = Arc::new(Mutex::new(MicrophoneReview::default()));
        let dispatch_review = Arc::clone(&microphone_review);
        let worker = thread::Builder::new()
            .name("osc-chatbox-dispatch".into())
            .spawn(move || {
                dispatch_loop(
                    rx,
                    dispatch_settings,
                    dispatch_status,
                    dispatch_cancellations,
                    dispatch_review,
                    dispatch_permission,
                    text_output,
                )
            })
            .expect("failed to start OSC dispatcher");
        Self {
            status,
            tx,
            worker: Some(worker),
            send_permitted,
            pending_cancellations,
            microphone_review,
        }
    }

    /// Transfer an endpoint without an old queued clear erasing the new writer.
    pub fn relinquish_endpoint(&self) {
        *self.send_permitted.lock() = false;
    }

    pub fn take_error(&self) -> Option<String> {
        self.status.lock().last_error.take()
    }
    pub fn chatbox_preview(&self) -> ChatboxPreview {
        let status = self.status.lock();
        ChatboxPreview {
            text: status.chatbox_text.clone(),
            typing: status.chatbox_typing,
            next_message_expires_in: status
                .next_message_expires_at
                .map(|expires_at| expires_at.saturating_duration_since(Instant::now())),
        }
    }
    pub fn clear_chatbox(&self) {
        let _ = self.tx.send(Command::Clear);
    }
    pub fn send_manual_message(&self, text: &str) {
        self.handle().send_manual_message(text);
    }
    pub fn handle(&self) -> OscHandle {
        OscHandle {
            tx: self.tx.clone(),
            pending_cancellations: self.pending_cancellations.clone(),
            microphone_review: Arc::clone(&self.microphone_review),
        }
    }
    pub fn update_settings(&mut self, settings: OscSettings) -> Result<(), String> {
        settings.validate()?;
        if settings.enabled {
            *self.send_permitted.lock() = true;
        }
        let _ = self.tx.send(Command::Update(settings));
        Ok(())
    }
    pub fn shutdown(&mut self) {
        self.clear_chatbox();
        let _ = self.tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for OscManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(super) fn parse_mute_value(value: &OscType) -> Option<bool> {
    match value {
        OscType::Bool(value) => Some(*value),
        OscType::Int(value) => Some(*value != 0),
        OscType::Long(value) => Some(*value != 0),
        OscType::Float(value) => Some(*value != 0.0),
        OscType::Double(value) => Some(*value != 0.0),
        OscType::String(value) => match value.trim().to_lowercase().as_str() {
            "true" | "1" | "on" | "yes" | "mute" | "muted" => Some(true),
            "false" | "0" | "off" | "no" | "unmute" | "unmuted" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn dispatch_loop(
    rx: Receiver<Command>,
    mut settings: OscSettings,
    status: Arc<Mutex<RuntimeStatus>>,
    pending_cancellations: Arc<Mutex<BTreeSet<u64>>>,
    microphone_review: Arc<Mutex<MicrophoneReview>>,
    send_permitted: Arc<Mutex<bool>>,
    text_output: TextOutputHandle,
) {
    let mut history = Vec::new();
    let mut live = Vec::new();
    let mut previews: Vec<HistoryMessage> = Vec::new();
    let mut manual_message = None;
    let mut pending_voice: Option<VoicePublication> = None;
    let mut published_voice: Option<PublishedVoice> = None;
    let mut last_voice_operation: Option<u64> = None;
    let mut lyrics = LyricsText::default();
    // Keep only the latest snapshot, while preserving a final notification.
    let mut pending: Option<bool> = None;
    let mut clear_pending = false;
    let mut force_send = false;
    let mut last_send = Instant::now() - COOLDOWN;
    let mut next_banner_refresh = banner_refresh_deadline(&settings, Instant::now(), false);
    let mut microphone_generation = 0;
    loop {
        let wait = dispatch_wait(
            pending.is_some() || pending_voice.is_some(),
            last_send,
            next_content_expiry(&history, &live, manual_message.as_ref())
                .into_iter()
                .chain(previews.iter().map(|entry| entry.expires_at))
                .chain(published_voice.iter().map(|entry| entry.expires_at))
                .min(),
            next_banner_refresh,
        );
        let command = rx.recv_timeout(wait);
        let now = Instant::now();
        let mut changed = false;
        let mut notify = false;
        let mut shutdown = false;
        // Hold through packet submission so a mode setter cannot return while
        // the dispatcher is still about to send an old microphone body.
        let review = microphone_review.lock();
        if microphone_generation != review.generation {
            if review.last_exit_generation > microphone_generation {
                // Leaving review immediately restores the normal composer. A
                // successful publication remains acknowledged, but no longer
                // owns the visible body until its original TTL.
                cancel_voice_publication(&mut pending_voice, "Microphone review was disabled");
                published_voice = None;
            }
            history
                .retain(|entry: &HistoryMessage| entry.source_kind != OscInputSource::Microphone);
            live.retain(|entry: &HistoryMessage| entry.source_kind != OscInputSource::Microphone);
            previews.retain(|entry| entry.source_kind != OscInputSource::Microphone);
            microphone_generation = review.generation;
            changed = true;
        }
        match command {
            Ok(Command::Lyrics(text)) => {
                changed |= text != lyrics;
                lyrics = text;
                next_banner_refresh = banner_refresh_deadline(&settings, now, !lyrics.is_empty());
            }
            Ok(Command::ManualMessage { text, ttl }) if settings.enabled => {
                cancel_voice_publication(
                    &mut pending_voice,
                    "A newer manual message replaced this publication",
                );
                published_voice = None;
                clear_pending = false;
                manual_message = Some(ManualMessage {
                    text,
                    updated_at: now,
                    expires_at: now
                        + ttl.unwrap_or_else(|| {
                            Duration::from_secs_f64(settings.history_ttl_seconds)
                        }),
                });
                changed = true;
                notify = true;
            }
            Ok(Command::PublishVoice(publication)) => {
                let validation = settings.validate_voice_text(&publication.text);
                if let Err(error) = validation {
                    let _ = publication.reply.send(Err(error));
                } else if last_voice_operation.is_some_and(|last| publication.operation <= last) {
                    let _ = publication.reply.send(Ok(()));
                } else if publication.review_generation < review.last_exit_generation {
                    let _ = publication.reply.send(Err(
                        "Microphone review was disabled before publication".into(),
                    ));
                } else if pending_voice.is_some() {
                    let _ = publication
                        .reply
                        .send(Err("Another voice publication is still pending".into()));
                } else {
                    pending_voice = Some(publication);
                }
            }
            Ok(Command::Previews(stream_id, is_typing, incoming, generation)) => {
                previews.retain(|entry| entry.stream_id != stream_id);
                if settings.enabled {
                    previews.extend(
                        incoming
                            .into_iter()
                            .map(|entry| HistoryMessage {
                                stream_id,
                                source_kind: if is_typing {
                                    OscInputSource::Typing
                                } else {
                                    entry.audio_source.into()
                                },
                                source: sanitize_chatbox_segment(&entry.source),
                                translated: sanitize_chatbox_segment(
                                    entry.translated.as_deref().unwrap_or_default(),
                                ),
                                additional_translations: Vec::new(),
                                speaker_id: String::new(),
                                updated_at: now,
                                expires_at: now
                                    + Duration::from_secs_f64(settings.history_ttl_seconds),
                            })
                            .filter(|entry| {
                                settings.accepts_source(entry.source_kind)
                                    && review.accepts(entry.source_kind, generation)
                            }),
                    );
                }
                changed = true;
            }
            Ok(Command::Message {
                stream_id,
                audio_source,
                microphone_generation,
                is_typing,
                source,
                translated,
                additional_translations,
                speaker_id,
                ongoing,
                ttl,
            }) if settings.enabled && review.accepts(audio_source, microphone_generation) => {
                let entry = HistoryMessage {
                    stream_id,
                    source_kind: if is_typing {
                        OscInputSource::Typing
                    } else {
                        audio_source
                    },
                    source,
                    translated,
                    additional_translations,
                    speaker_id,
                    updated_at: now,
                    expires_at: now
                        + ttl.unwrap_or_else(|| {
                            Duration::from_secs_f64(settings.history_ttl_seconds)
                        }),
                };
                if settings.accepts_source(entry.source_kind)
                    && !render_entry(&entry, &settings).is_empty()
                {
                    clear_pending = false;
                    live.retain(|item: &HistoryMessage| item.stream_id != stream_id);
                    if ongoing {
                        live.push(entry);
                    } else {
                        history.push(entry);
                    }
                    changed = true;
                    notify = !ongoing;
                }
            }
            Ok(Command::RollStream {
                stream_id,
                audio_source,
                microphone_generation,
                is_typing,
                source,
                translated,
                speaker_id,
            }) if settings.enabled
                && (is_typing || settings.accepts_source(audio_source))
                && review.accepts(audio_source, microphone_generation) =>
            {
                clear_pending = false;
                if let Some(index) = live.iter().position(|entry| entry.stream_id == stream_id) {
                    history.push(live.remove(index));
                }
                live.push(HistoryMessage {
                    stream_id,
                    source_kind: if is_typing {
                        OscInputSource::Typing
                    } else {
                        audio_source
                    },
                    source,
                    translated,
                    additional_translations: Vec::new(),
                    speaker_id,
                    updated_at: now,
                    expires_at: now + Duration::from_secs_f64(settings.history_ttl_seconds),
                });
                changed = true;
                notify = true;
            }
            Ok(Command::EndStream(stream_id)) if settings.enabled => {
                previews.retain(|entry| entry.stream_id != stream_id);
                changed = true;
                if let Some(index) = live.iter().position(|entry| entry.stream_id == stream_id) {
                    history.push(live.remove(index));
                    changed = true;
                    notify = true;
                }
            }
            Ok(Command::CancelStream(stream_id)) => {
                let before = history.len() + live.len() + previews.len();
                history.retain(|entry| entry.stream_id != stream_id);
                live.retain(|entry| entry.stream_id != stream_id);
                previews.retain(|entry| entry.stream_id != stream_id);
                changed |= before != history.len() + live.len() + previews.len();
                pending_cancellations.lock().remove(&stream_id);
            }
            Ok(Command::MicrophoneReviewChanged) => {}
            Ok(Command::Clear) => {
                cancel_voice_publication(&mut pending_voice, "Chatbox output was cleared");
                published_voice = None;
                previews.clear();
                manual_message = None;
                history.clear();
                live.clear();
                pending = None;
                clear_pending = true;
                changed = true;
            }
            Ok(Command::Update(updated)) => {
                force_send |= settings.ip != updated.ip
                    || settings.send_port != updated.send_port
                    || settings.enabled != updated.enabled;
                if settings.enabled && !updated.enabled {
                    cancel_voice_publication(&mut pending_voice, "OSC Chatbox output was disabled");
                    published_voice = None;
                    let _ = send_message(&settings, &clear_message(), &status, &send_permitted);
                    pending = None;
                    manual_message = None;
                    history.clear();
                    live.clear();
                    previews.clear();
                    last_send = now;
                }
                settings = updated;
                if settings.lyric_budget().is_none() {
                    lyrics = LyricsText::default();
                }
                history.retain(|entry| settings.accepts_source(entry.source_kind));
                live.retain(|entry| settings.accepts_source(entry.source_kind));
                previews.retain(|entry| settings.accepts_source(entry.source_kind));
                next_banner_refresh = banner_refresh_deadline(&settings, now, !lyrics.is_empty());
                changed = true;
            }
            Ok(Command::Shutdown) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                shutdown = true;
            }
            _ => {}
        }
        // Cancel may be queued behind many completed captions. Remove fenced
        // streams before constructing the next packet, including cooldown sends.
        let cancelled = pending_cancellations.lock();
        let before_cancel = history.len() + live.len() + previews.len();
        history.retain(|entry| !cancelled.contains(&entry.stream_id));
        live.retain(|entry| !cancelled.contains(&entry.stream_id));
        previews.retain(|entry| !cancelled.contains(&entry.stream_id));
        changed |= before_cancel != history.len() + live.len() + previews.len();
        let previous_preview_count = previews.len();
        previews.retain(|entry| entry.expires_at > now);
        changed |= previews.len() != previous_preview_count;
        // Service deadlines even while incoming commands keep the channel busy.
        changed |= refresh_chatbox_entries(
            &mut history,
            &mut live,
            &mut manual_message,
            Duration::from_secs_f64(settings.history_ttl_seconds),
            now,
        );
        if published_voice
            .as_ref()
            .is_some_and(|entry| entry.expires_at <= now)
        {
            published_voice = None;
            changed = true;
        }
        if next_banner_refresh.is_some_and(|deadline| now >= deadline) {
            next_banner_refresh = banner_refresh_deadline(&settings, now, !lyrics.is_empty());
            force_send = true;
            changed = true;
        }
        if changed {
            if settings.enabled {
                pending = Some(pending.unwrap_or(false) || notify);
            } else {
                clear_runtime_preview(&status);
            }
        }
        if shutdown {
            cancel_voice_publication(&mut pending_voice, "OSC output has stopped");
        } else if pending_voice.is_some() && last_send.elapsed() >= COOLDOWN {
            let publication = pending_voice.take().unwrap();
            let expires_at = Instant::now() + Duration::from_secs_f64(settings.history_ttl_seconds);
            let message = QueuedMessage {
                text: publication.text,
                typing: false,
                notify: true,
                expires_at: Some(expires_at),
            };
            // Validate again because settings may have changed during cooldown.
            // Never trim or reformat a body after the user's send gesture.
            let result = settings
                .validate_voice_text(&message.text)
                .and_then(|()| send_message(&settings, &message, &status, &send_permitted));
            last_send = Instant::now();
            if result.is_ok() {
                last_voice_operation = Some(publication.operation);
                published_voice = Some(PublishedVoice {
                    text: message.text,
                    expires_at,
                });
                clear_pending = false;
            }
            let _ = publication.reply.send(result);
        }
        if pending.is_some() && (last_send.elapsed() >= COOLDOWN || shutdown) {
            let notify = pending.take().unwrap_or(false);
            let message = if std::mem::take(&mut clear_pending) {
                clear_message()
            } else if let Some(voice) = &published_voice {
                // Automatic sources keep updating in the background. The
                // accepted voice body owns the visible Chatbox until its TTL.
                QueuedMessage {
                    text: voice.text.clone(),
                    typing: false,
                    notify: false,
                    expires_at: Some(voice.expires_at),
                }
            } else {
                build_queued_message(
                    &history,
                    &live.iter().chain(&previews).cloned().collect::<Vec<_>>(),
                    manual_message.as_ref(),
                    &settings,
                    &text_output.snapshot(),
                    &lyrics,
                    !live.is_empty() || !previews.is_empty(),
                    notify,
                )
            };
            let unchanged = {
                let mut runtime = status.lock();
                runtime.next_message_expires_at = message.expires_at;
                runtime.chatbox_text == message.text && runtime.chatbox_typing == message.typing
            };
            if !unchanged || message.notify || (force_send && published_voice.is_none()) {
                let _ = send_message(&settings, &message, &status, &send_permitted);
                last_send = Instant::now();
            }
            force_send = false;
        }
        drop(cancelled);
        if shutdown {
            return;
        }
    }
}

fn cancel_voice_publication(pending: &mut Option<VoicePublication>, reason: &str) {
    if let Some(publication) = pending.take() {
        let _ = publication.reply.send(Err(reason.to_owned()));
    }
}

fn clear_message() -> QueuedMessage {
    QueuedMessage {
        text: String::new(),
        typing: false,
        notify: false,
        expires_at: None,
    }
}

fn send_message(
    settings: &OscSettings,
    message: &QueuedMessage,
    status: &Mutex<RuntimeStatus>,
    send_permitted: &Mutex<bool>,
) -> Result<(), String> {
    // Held only across local UDP submission, so handover fences both packets.
    let permission = send_permitted.lock();
    if !*permission {
        return Err("OSC destination was transferred to another output".into());
    }
    let result = send_packets(settings, message);
    let mut runtime = status.lock();
    match &result {
        Ok(()) => {
            runtime.chatbox_text.clone_from(&message.text);
            runtime.chatbox_typing = message.typing;
            runtime.next_message_expires_at = message.expires_at;
        }
        Err(error) => runtime.last_error = Some(error.clone()),
    }
    result
}

fn send_packets(settings: &OscSettings, message: &QueuedMessage) -> Result<(), String> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|error| format!("Cannot create OSC sender: {error}"))?;
    let target = format!("{}:{}", settings.ip, settings.send_port);
    for packet in settings
        .game
        .packets(&message.text, message.typing, message.notify)
    {
        let bytes =
            encoder::encode(&packet).map_err(|error| format!("Cannot encode OSC: {error}"))?;
        socket
            .send_to(&bytes, &target)
            .map_err(|error| format!("Cannot send OSC: {error}"))?;
    }
    Ok(())
}

fn build_queued_message(
    history: &[HistoryMessage],
    live: &[HistoryMessage],
    manual_message: Option<&ManualMessage>,
    settings: &OscSettings,
    metrics: &crate::plugins::text_output::SystemMetrics,
    lyrics: &LyricsText,
    typing: bool,
    notify: bool,
) -> QueuedMessage {
    QueuedMessage {
        text: build_chatbox_text(history, live, manual_message, settings, metrics, lyrics),
        typing,
        notify,
        expires_at: next_content_expiry(history, live, manual_message),
    }
}

fn dispatch_wait(
    pending: bool,
    last_send: Instant,
    content_expiry: Option<Instant>,
    banner_refresh: Option<Instant>,
) -> Duration {
    let now = Instant::now();
    let send_wait = pending.then(|| COOLDOWN.saturating_sub(now.duration_since(last_send)));
    let expiry_wait = content_expiry.map(|expires_at| expires_at.saturating_duration_since(now));
    let banner_wait = banner_refresh.map(|deadline| deadline.saturating_duration_since(now));
    [send_wait, expiry_wait, banner_wait]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(Duration::from_secs(3600))
}

fn banner_refresh_deadline(
    settings: &OscSettings,
    now: Instant,
    has_lyric: bool,
) -> Option<Instant> {
    if !settings.enabled {
        return None;
    }
    let persistent = settings.persistent_banners
        && [&settings.header_config, &settings.footer_config]
            .into_iter()
            .any(|banner| {
                !matches!(
                    banner.content_type,
                    BannerContentType::None | BannerContentType::Lyrics
                )
            });
    if persistent {
        Some(now + BANNER_REFRESH_INTERVAL)
    } else {
        has_lyric.then_some(now + Duration::from_secs(5))
    }
}

fn next_content_expiry(
    history: &[HistoryMessage],
    live: &[HistoryMessage],
    manual_message: Option<&ManualMessage>,
) -> Option<Instant> {
    let asr_expiry = history
        .iter()
        .chain(live.iter())
        .map(|entry| entry.expires_at)
        .min();
    match (asr_expiry, manual_message.map(|m| m.expires_at)) {
        (Some(a), Some(m)) => Some(a.min(m)),
        (Some(a), None) => Some(a),
        (None, Some(m)) => Some(m),
        (None, None) => None,
    }
}

fn refresh_chatbox_entries(
    history: &mut Vec<HistoryMessage>,
    live: &mut Vec<HistoryMessage>,
    manual: &mut Option<ManualMessage>,
    max_ttl: Duration,
    now: Instant,
) -> bool {
    let mut changed = expire_chatbox_entries(history, live, now);
    if manual.as_ref().is_some_and(|entry| now >= entry.expires_at) {
        *manual = None;
        changed = true;
    }
    let mut timeline = history
        .iter_mut()
        .chain(live.iter_mut())
        .map(|entry| (entry.updated_at, &mut entry.expires_at))
        .chain(
            manual
                .iter_mut()
                .map(|entry| (entry.updated_at, &mut entry.expires_at)),
        )
        .collect::<Vec<_>>();
    timeline.sort_by_key(|(updated_at, _)| *updated_at);
    // Newer messages shorten older messages' lifetime. Never postpone an
    // existing deadline, or make new translations wait for an old message's TTL.
    let minimum = Duration::from_secs(2).min(max_ttl);
    for (newer, (updated_at, expires_at)) in timeline.into_iter().rev().enumerate() {
        let ttl = (max_ttl / (newer + 1) as u32).max(minimum);
        let deadline = (*expires_at).min(updated_at + ttl);
        changed |= deadline != *expires_at;
        *expires_at = deadline;
    }
    changed |= expire_chatbox_entries(history, live, now);
    if manual.as_ref().is_some_and(|entry| now >= entry.expires_at) {
        *manual = None;
        changed = true;
    }
    changed
}

fn expire_chatbox_entries(
    history: &mut Vec<HistoryMessage>,
    live: &mut Vec<HistoryMessage>,
    now: Instant,
) -> bool {
    let previous_len = history.len();
    history.retain(|entry| now < entry.expires_at);
    let history_changed = history.len() != previous_len;
    let previous_live_len = live.len();
    live.retain(|entry| now < entry.expires_at);
    history_changed || live.len() != previous_live_len
}

fn clear_runtime_preview(status: &Mutex<RuntimeStatus>) {
    let mut runtime = status.lock();
    runtime.chatbox_text.clear();
    runtime.chatbox_typing = false;
    runtime.next_message_expires_at = None;
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn lyric_offset_default_and_saved_overrides_round_trip_with_osc_preferences() {
        assert_eq!(OscSettings::default().lyric_offset_ms(), 300);
        let mut old = serde_json::to_value(OscSettings::default()).unwrap();
        old.as_object_mut().unwrap().remove("lyric_offset_seconds");
        let mut settings: OscSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.lyric_offset_ms(), 300);
        for (seconds, milliseconds) in [
            (-1.0, -1000),
            (-0.3, -300),
            (0.0, 0),
            (0.1, 100),
            (1.0, 1000),
        ] {
            settings.lyric_offset_seconds = seconds;
            let saved = serde_json::to_string(&settings).unwrap();
            let restored: OscSettings = serde_json::from_str(&saved).unwrap();
            assert_eq!(restored.lyric_offset_ms(), milliseconds);
        }
        settings.lyric_offset_seconds = -10.0;
        assert_eq!(settings.lyric_offset_ms(), -1000);
        settings.lyric_offset_seconds = 10.0;
        assert_eq!(settings.lyric_offset_ms(), 1000);
    }
}
