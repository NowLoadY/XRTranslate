// This is a desktop GUI executable. Use the Windows GUI subsystem in every
// build so double-clicking the executable never creates a transient console.
#![cfg_attr(windows, windows_subsystem = "windows")]

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use eframe::egui;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

mod app_update;
mod audio;
mod backend;
mod child_process;
mod client_settings;
pub(crate) mod contributors;
mod feature_access;
mod history;
mod i18n;
pub(crate) mod media_import;
mod model_install;
mod network;
mod onboarding;
mod overlay_ipc;
mod overlay_manager;
#[cfg(windows)]
mod overlay_native;
mod plugins;
mod presentation;
mod runtime_install;
mod service_config;
pub mod session_coordinator;
mod streaming;
mod ui;
pub mod version;
mod window_backdrop;

use audio::{AudioSystem, InputConfigInfo, InputDevice};
use client_settings::{CaptureSource, ClientSettings, RecognitionSettings};
use history::{
    PendingFinalAsr, PendingRecognitionWindow, RecognitionHistoryEntry, TranslationHistoryEntry,
    collect_recognition_window, merge_stream_recognition, merge_stream_translation,
    upsert_completed_translation,
};
use i18n::UiLanguage;
use network::{ExternalAudioGate, SessionConfig, SessionEvent, SessionHandle, start_session};
use plugins::meeting::{
    MeetingAction, MeetingAudioSource, MeetingInputRequest, MeetingPlugin, MeetingUiSnapshot,
};
use plugins::osc::{OscPageContext, OscPlugin, OscUiAction};
use plugins::{PluginId, PluginPreferences, PluginRegistry, PluginScrollPolicy};
pub(crate) use presentation::speaker::compact_speaker_label;
use session_coordinator::{
    CaptionUpdate, HostOutputEvent, HostOutputSubscriber, PluginSessionBinding,
    SessionEventSubscriber, TranslationSessionOwner, TranslationSessionPlugin,
};
use ui::{NavigationState, Page};
use xrtranslate_prompt::{PromptExecutionTrace, PromptProviderTarget, PromptTemplateLibrary};

pub const LANGUAGE_OPTIONS: &[(&str, &str)] = &[
    ("zh", "Chinese"),
    ("zh-TW", "Traditional Chinese"),
    ("en", "English"),
    ("fr", "French"),
    ("pt", "Portuguese"),
    ("es", "Spanish"),
    ("ja", "Japanese"),
    ("ru", "Russian"),
    ("ko", "Korean"),
    ("th", "Thai"),
    ("hi", "Hindi"),
    ("it", "Italian"),
    ("de", "German"),
    ("vi", "Vietnamese"),
    ("id", "Indonesian"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("nl", "Dutch"),
];

/// Capture callbacks never block. A bounded handoff prevents an overloaded
/// network/model path from turning old audio into ever-growing live latency.
const LIVE_AUDIO_QUEUE_CAPACITY: usize = 64;

pub(crate) fn language_label(ui_language: UiLanguage, code: &str) -> &'static str {
    if code == "auto" {
        return i18n::tr(ui_language, "Auto (bidirectional)");
    }
    LANGUAGE_OPTIONS
        .iter()
        .find(|(value, _)| *value == code)
        .map(|(_, label)| i18n::tr(ui_language, label))
        .unwrap_or_else(|| i18n::tr(ui_language, "Unknown language"))
}

/// Returns true if the two language codes are mutually exclusive and should not
/// both be selectable at the same time (e.g. zh and zh-TW are the same base
/// language and would produce a no-op or circular translation route).
pub fn languages_conflict(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    fn chinese_variant(code: &str) -> bool {
        let c = code.trim().to_ascii_lowercase();
        c == "zh" || c.starts_with("zh-")
    }
    chinese_variant(a) && chinese_variant(b)
}

fn capture_source_to_meeting(source: CaptureSource) -> MeetingAudioSource {
    match source {
        CaptureSource::Microphone => MeetingAudioSource::Microphone,
        CaptureSource::SystemAudio => MeetingAudioSource::SystemAudio,
        CaptureSource::Both => MeetingAudioSource::Both,
    }
}

fn meeting_source_to_capture(source: MeetingAudioSource) -> CaptureSource {
    match source {
        MeetingAudioSource::Microphone => CaptureSource::Microphone,
        MeetingAudioSource::SystemAudio => CaptureSource::SystemAudio,
        MeetingAudioSource::Both => CaptureSource::Both,
    }
}

fn meeting_source_name_to_capture(source: &str) -> CaptureSource {
    match source {
        "system_audio" => CaptureSource::SystemAudio,
        "both" => CaptureSource::Both,
        _ => CaptureSource::Microphone,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingResourceDeletion {
    Model(xrtranslate_assets::ModelAssetId),
    Runtime,
}

struct XRTranslateApp {
    audio_system: AudioSystem,
    devices: Vec<InputDevice>,
    device_refresh_rx: Option<Receiver<AudioDeviceSnapshot>>,
    selected_device_id: String,
    loopback_devices: Vec<InputDevice>,
    selected_loopback_device_id: String,
    tts_output_devices: Vec<InputDevice>,
    selected_tts_output_device_id: String,
    capture_source: CaptureSource,
    microphone_recognition: RecognitionSettings,
    loopback_recognition: RecognitionSettings,
    selected_input_config: Option<InputConfigInfo>,
    is_translating: bool,
    pub(crate) session_owner: TranslationSessionOwner,
    audio_txs: Vec<Sender<Vec<f32>>>,
    input_level: Arc<AtomicU32>,
    loopback_level: Arc<AtomicU32>,
    microphone_vad_active: Arc<AtomicBool>,
    loopback_vad_active: Arc<AtomicBool>,
    sessions: Vec<SessionHandle>,
    meeting_audio_routers: Vec<std::thread::JoinHandle<()>>,
    event_tx: Sender<SessionEvent>,
    connection_status: String,
    partial_text: String,
    recognition_history: Vec<RecognitionHistoryEntry>,
    translations: Vec<TranslationHistoryEntry>,
    last_error: Option<String>,
    server_url: String,
    download_proxy_url: String,
    update_channel: client_settings::UpdateChannel,
    source_lang: String,
    target_lang: String,
    denoise_enabled: bool,
    tts_enabled: bool,
    microphone_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    loopback_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    tts_runtime_backend: Option<String>,
    tts_runtime_cuda_version: Option<String>,
    osc_plugin: OscPlugin,
    meeting_plugin: MeetingPlugin,
    player_plugin: plugins::player::VideoPlayerPlugin,
    host_audio_import: Option<media_import::AudioImportHandle>,
    pending_audio_import: Option<(
        std::path::PathBuf,
        RecognitionSettings,
        Vec<usize>,
        media_import::AudioImportPacing,
    )>,
    plugin_preferences: PluginPreferences,
    service_config: service_config::ServiceConfigEditor,
    backend_manager: backend::BackendManager,
    model_task_manager: model_install::NativeModelTaskManager,
    runtime_installer: runtime_install::RuntimeInstaller,
    app_update_manager: app_update::AppUpdateManager,
    notified_update_version: Option<String>,
    notified_ready_update_version: Option<String>,
    backend_start_deadline: Option<std::time::Instant>,
    pub settings_section: ui::pages::settings::SettingsSection,
    pub prompt_library: PromptTemplateLibrary,
    pub prompt_studio: ui::pages::prompt_studio::PromptStudioController,
    pub modal_dialog: ui::modal::ModalDialog,
    pending_resource_deletion: Option<PendingResourceDeletion>,
    pub first_run: bool,
    pub onboarding_page: usize,
    pub ui_language: UiLanguage,
    navigation: NavigationState,
    window_backdrop: window_backdrop::WindowBackdrop,
    mute_self_pauses_translation: Arc<AtomicBool>,
    pub floating_subtitles_enabled: bool,
    pub floating_subtitles_max_count: usize,
    pub floating_subtitles_font_size: f64,
    pub overlay_manager: Arc<Mutex<overlay_manager::OverlayManager>>,
    shared_session_state: Arc<Mutex<SharedSessionState>>,
    overlay_enabled_atomic: Arc<AtomicBool>,
    overlay_max_count_atomic: Arc<AtomicUsize>,
    overlay_font_size_atomic: Arc<AtomicU32>,
}

struct AudioDeviceSnapshot {
    devices: Vec<InputDevice>,
    loopback_devices: Vec<InputDevice>,
}

#[derive(Default)]
struct SharedSessionState {
    connection_status: String,
    partial_text: String,
    pending_final_asr: Vec<PendingFinalAsr>,
    pending_recognition_windows: Vec<PendingRecognitionWindow>,
    recognition_history: Vec<RecognitionHistoryEntry>,
    translations: Vec<TranslationHistoryEntry>,
    last_error: Option<String>,
    is_translating: bool,
    pending_route_change: Option<(String, String)>,
    latest_asr_prompt_trace: Option<PromptExecutionTrace>,
    latest_translation_prompt_trace: Option<PromptExecutionTrace>,
    provider_configuration_required: bool,
    microphone_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    loopback_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    tts_runtime_backend: Option<String>,
    tts_runtime_cuda_version: Option<String>,
}

fn publish_host_output(subscribers: &[Box<dyn HostOutputSubscriber>], event: HostOutputEvent<'_>) {
    for subscriber in subscribers {
        subscriber.on_host_output(event);
    }
}

/// Initializes output-side session dependencies before activating live input.
///
/// On Windows the session configuration can create the default-output TTS
/// stream. WASAPI loopback must be opened after that output stream is ready;
/// otherwise the first loopback client can remain silent until it is rebuilt
/// by a device selection change.
fn initialize_live_audio<Host, Dependencies>(
    host: &mut Host,
    initialize_dependencies: impl FnOnce(&mut Host) -> Dependencies,
    activate_capture: impl FnOnce(&mut Host) -> Result<(), String>,
) -> Result<Dependencies, String> {
    let dependencies = initialize_dependencies(host);
    activate_capture(host)?;
    Ok(dependencies)
}

impl Default for XRTranslateApp {
    fn default() -> Self {
        let audio_system = AudioSystem::new();
        let devices = audio_system.available_devices();
        let loopback_devices = audio_system.available_loopback_devices();
        let tts_output_devices = audio_system.available_output_devices();
        let (event_tx, event_rx) = unbounded();
        let backend_manager = backend::BackendManager::load();
        let service_config = service_config::ServiceConfigEditor::load();
        let mut settings = ClientSettings::load(&backend_manager.project_root());
        settings.sanitize_devices(&devices, &loopback_devices);

        let osc_plugin = OscPlugin::new(
            settings.osc_settings.clone(),
            settings.plugin_preferences.is_enabled(PluginId::OSC),
        );
        let meeting_plugin = MeetingPlugin::open(&backend_manager.project_root());
        let player_plugin = plugins::player::VideoPlayerPlugin::new();
        let mut model_task_manager = model_install::NativeModelTaskManager::default();
        model_task_manager.set_proxy_url(&settings.download_proxy_url);
        let mut runtime_installer = runtime_install::RuntimeInstaller::default();
        runtime_installer.set_proxy_url(&settings.download_proxy_url);
        let mut app_update_manager = app_update::AppUpdateManager::default();
        app_update_manager.set_proxy_url(&settings.download_proxy_url);
        app_update_manager.set_channel(settings.update_channel);

        let selected_input_config = match settings.capture_source {
            CaptureSource::Microphone => {
                audio_system.input_config(&settings.selected_device_id).ok()
            }
            CaptureSource::SystemAudio => audio_system
                .loopback_config(&settings.selected_loopback_device_id)
                .ok(),
            CaptureSource::Both => audio_system.input_config(&settings.selected_device_id).ok(),
        };

        let shared_session_state = Arc::new(Mutex::new(SharedSessionState {
            connection_status: "Ready".into(),
            ..Default::default()
        }));
        let overlay_manager = Arc::new(Mutex::new(overlay_manager::OverlayManager::new()));
        let overlay_enabled_atomic = Arc::new(AtomicBool::new(settings.floating_subtitles_enabled));
        let overlay_max_count_atomic =
            Arc::new(AtomicUsize::new(settings.floating_subtitles_max_count));
        let overlay_font_size_atomic =
            Arc::new(AtomicU32::new(settings.floating_subtitles_font_size as u32));
        let microphone_vad_active = Arc::new(AtomicBool::new(false));
        let loopback_vad_active = Arc::new(AtomicBool::new(false));
        let input_level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let loopback_level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));

        // Background session event pump thread
        let shared_state_clone = Arc::clone(&shared_session_state);
        let overlay_mgr_clone = Arc::clone(&overlay_manager);
        let overlay_enabled_clone = Arc::clone(&overlay_enabled_atomic);
        let overlay_max_count_clone = Arc::clone(&overlay_max_count_atomic);
        let overlay_font_size_clone = Arc::clone(&overlay_font_size_atomic);
        let microphone_vad_active_clone = Arc::clone(&microphone_vad_active);
        let loopback_vad_active_clone = Arc::clone(&loopback_vad_active);
        let rx = event_rx.clone();
        let session_event_subscribers: Vec<Box<dyn SessionEventSubscriber>> =
            vec![Box::new(meeting_plugin.event_sink.clone())];
        let host_output_subscribers: Vec<Box<dyn HostOutputSubscriber>> =
            vec![Box::new(osc_plugin.publisher())];

        std::thread::Builder::new()
            .name("session-event-pump".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    for subscriber in &session_event_subscribers {
                        subscriber.on_session_event(&event);
                    }
                    let mut state = shared_state_clone.lock().unwrap();
                    match event {
                        SessionEvent::Connected => {
                            state.connection_status = "Connected - listening".into();
                            state.is_translating = true;
                            state.tts_runtime_backend = None;
                            state.tts_runtime_cuda_version = None;
                        }
                        SessionEvent::Disconnected(reason) => {
                            publish_host_output(&host_output_subscribers, HostOutputEvent::Clear);
                            state.connection_status = reason;
                            state.is_translating = false;
                            for entry in &mut state.translations {
                                entry.live = false;
                            }
                            for entry in &mut state.recognition_history {
                                entry.live = false;
                            }
                            state.pending_recognition_windows.clear();
                            state.tts_runtime_backend = None;
                            state.tts_runtime_cuda_version = None;
                            microphone_vad_active_clone.store(false, Ordering::Relaxed);
                            loopback_vad_active_clone.store(false, Ordering::Relaxed);
                        }
                        SessionEvent::Status(status) => state.connection_status = status,
                        SessionEvent::VadActivity { source, active } => match source {
                            CaptureSource::Microphone => {
                                microphone_vad_active_clone.store(active, Ordering::Relaxed)
                            }
                            CaptureSource::SystemAudio => {
                                loopback_vad_active_clone.store(active, Ordering::Relaxed)
                            }
                            CaptureSource::Both => {}
                        },
                        SessionEvent::Asr {
                            stream_id,
                            continuous,
                            publish_to_host_outputs,
                            kind,
                            text,
                            turn_id,
                        } => {
                            if !publish_to_host_outputs {
                                continue;
                            }
                            if kind == "partial" && !continuous && !text.is_empty() {
                                publish_host_output(
                                    &host_output_subscribers,
                                    HostOutputEvent::Caption {
                                        stream_id,
                                        source: &text,
                                        translated: "",
                                        speaker: "",
                                        update: CaptionUpdate::Replace,
                                    },
                                );
                            }
                            if kind == "final" && !text.is_empty() {
                                state.pending_final_asr.push(PendingFinalAsr {
                                    text: text.clone(),
                                    turn_id: turn_id.clone(),
                                });
                                if state.pending_final_asr.len() > 100 {
                                    state.pending_final_asr.remove(0);
                                }
                                let is_duplicate =
                                    state.recognition_history.last().is_some_and(|entry| {
                                        entry.text == text
                                            && entry.turn_id == turn_id
                                            && entry.speaker_id.is_empty()
                                    });
                                if !is_duplicate {
                                    state.recognition_history.push(RecognitionHistoryEntry {
                                        stream_id: None,
                                        live: false,
                                        text: text.clone(),
                                        turn_id,
                                        speaker_id: String::new(),
                                        source_start_ms: 0.0,
                                        source_end_ms: 0.0,
                                        timing: xrtranslate_protocol::SegmentTiming::Unknown,
                                        boundary: xrtranslate_protocol::SegmentBoundary::Unknown,
                                        activation_matches: Vec::new(),
                                        context_matches: Vec::new(),
                                        revisable: false,
                                        overlap_ratio: 0.0,
                                        revision: None,
                                    });
                                    if state.recognition_history.len() > 100 {
                                        state.recognition_history.remove(0);
                                    }
                                }
                            }
                            state.partial_text = if kind == "partial" || kind == "blank" {
                                text
                            } else {
                                String::new()
                            };
                        }
                        SessionEvent::SourceSegment {
                            stream_id,
                            audio_source: _,
                            continuous,
                            publish_to_host_outputs,
                            text,
                            prompt_trace,
                            activation_matches,
                            context_matches,
                            turn_id,
                            speaker_id,
                            source_start_ms,
                            source_end_ms,
                            timing,
                            boundary,
                            segment_index,
                            segment_count,
                            revisable,
                            overlap_ratio,
                        } => {
                            if text.is_empty() {
                                continue;
                            }
                            if !publish_to_host_outputs {
                                continue;
                            }
                            state.latest_asr_prompt_trace = prompt_trace;
                            if segment_index == 1 {
                                let pending_index = state
                                    .pending_final_asr
                                    .iter()
                                    .position(|pending| {
                                        (!turn_id.is_empty() && pending.turn_id == turn_id)
                                            || (turn_id.is_empty() && pending.turn_id.is_empty())
                                    })
                                    .or_else(|| {
                                        state
                                            .pending_final_asr
                                            .iter()
                                            .position(|pending| pending.turn_id.is_empty())
                                    });
                                if let Some(pending_index) = pending_index {
                                    let pending = state.pending_final_asr.remove(pending_index);
                                    let temporary_index =
                                        state.recognition_history.iter().rposition(|entry| {
                                            entry.speaker_id.is_empty()
                                                && if pending.turn_id.is_empty() {
                                                    entry.turn_id.is_empty()
                                                        && entry.text == pending.text
                                                } else {
                                                    entry.turn_id == pending.turn_id
                                                }
                                        });
                                    if let Some(temporary_index) = temporary_index {
                                        state.recognition_history.remove(temporary_index);
                                    }
                                }
                            }
                            let entry = RecognitionHistoryEntry {
                                stream_id: continuous.then_some(stream_id),
                                live: continuous,
                                text,
                                turn_id,
                                speaker_id,
                                source_start_ms,
                                source_end_ms,
                                timing,
                                boundary,
                                activation_matches,
                                context_matches,
                                revisable,
                                overlap_ratio,
                                revision: None,
                            };
                            let complete = collect_recognition_window(
                                &mut state.pending_recognition_windows,
                                stream_id,
                                continuous,
                                segment_index,
                                segment_count,
                                entry,
                            );
                            if let Some(entry) = complete {
                                if continuous {
                                    merge_stream_recognition(
                                        &mut state.recognition_history,
                                        stream_id,
                                        entry,
                                    );
                                } else if state.recognition_history.last() != Some(&entry) {
                                    state.recognition_history.push(entry);
                                }
                                if state.recognition_history.len() > 100 {
                                    state.recognition_history.remove(0);
                                }
                            }
                        }
                        SessionEvent::Translation {
                            stream_id,
                            audio_source,
                            continuous,
                            publish_to_host_outputs,
                            source,
                            translated,
                            turn_id,
                            segment_index,
                            segment_count: _,
                            speaker_id,
                            source_start_ms,
                            source_end_ms,
                            timing,
                            boundary,
                            term_matches,
                            prompt_trace,
                            revisable,
                            overlap_ratio,
                        } => {
                            if !publish_to_host_outputs {
                                continue;
                            }
                            state.latest_translation_prompt_trace = prompt_trace;
                            let fragment = TranslationHistoryEntry {
                                turn_id: turn_id.clone(),
                                segment_index,
                                stream_id: continuous.then_some(stream_id),
                                audio_source,
                                live: continuous,
                                source,
                                translated,
                                speaker_id,
                                source_start_ms,
                                source_end_ms,
                                timing,
                                boundary,
                                term_matches,
                                revisable,
                                overlap_ratio,
                                source_revision: None,
                                translated_revision: None,
                            };
                            if continuous {
                                let merged = merge_stream_translation(
                                    &mut state.translations,
                                    stream_id,
                                    fragment,
                                );
                                if merged.rolled_over {
                                    publish_host_output(
                                        &host_output_subscribers,
                                        HostOutputEvent::Caption {
                                            stream_id,
                                            source: &merged.entry.source,
                                            translated: &merged.entry.translated,
                                            speaker: &merged.entry.speaker_id,
                                            update: CaptionUpdate::RollOver,
                                        },
                                    );
                                } else if merged.changed {
                                    publish_host_output(
                                        &host_output_subscribers,
                                        HostOutputEvent::Caption {
                                            stream_id,
                                            source: &merged.entry.source,
                                            translated: &merged.entry.translated,
                                            speaker: &merged.entry.speaker_id,
                                            update: CaptionUpdate::Replace,
                                        },
                                    );
                                }
                            } else {
                                publish_host_output(
                                    &host_output_subscribers,
                                    HostOutputEvent::Caption {
                                        stream_id,
                                        source: &fragment.source,
                                        translated: &fragment.translated,
                                        speaker: &fragment.speaker_id,
                                        update: CaptionUpdate::Append,
                                    },
                                );
                                upsert_completed_translation(&mut state.translations, fragment);
                            }
                            if state.translations.len() > 100 {
                                state.translations.remove(0);
                            }
                        }
                        SessionEvent::StreamEnded {
                            stream_id,
                            publish_to_host_outputs,
                        } => {
                            if !publish_to_host_outputs {
                                continue;
                            }
                            if let Some(entry) = state
                                .translations
                                .iter_mut()
                                .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
                            {
                                entry.live = false;
                            }
                            if let Some(entry) = state
                                .recognition_history
                                .iter_mut()
                                .rfind(|entry| entry.stream_id == Some(stream_id) && entry.live)
                            {
                                entry.live = false;
                            }
                            state
                                .pending_recognition_windows
                                .retain(|window| window.stream_id != stream_id);
                            publish_host_output(
                                &host_output_subscribers,
                                HostOutputEvent::StreamEnded(stream_id),
                            );
                        }
                        SessionEvent::RouteChanged {
                            source_lang,
                            target_lang,
                        } => {
                            state.pending_route_change = Some((source_lang, target_lang));
                        }
                        SessionEvent::TtsRuntime {
                            backend,
                            cuda_version,
                        } => {
                            state.tts_runtime_backend = Some(backend);
                            state.tts_runtime_cuda_version = cuda_version;
                        }
                        SessionEvent::TtsAudio(_audio) => {}
                        SessionEvent::VoiceCloneState { source, status } => match source {
                            CaptureSource::Microphone => {
                                state.microphone_clone_state = Some(status)
                            }
                            CaptureSource::SystemAudio => state.loopback_clone_state = Some(status),
                            CaptureSource::Both => {}
                        },
                        SessionEvent::BackendError {
                            message,
                            configuration_required,
                        } => {
                            state.last_error = Some(message);
                            state.provider_configuration_required |= configuration_required;
                        }
                        SessionEvent::Error(error) => {
                            publish_host_output(&host_output_subscribers, HostOutputEvent::Clear);
                            state.last_error = Some(error);
                            state.connection_status = "Connection error".into();
                            state.is_translating = false;
                        }
                    }

                    // Send state to overlay process immediately (unblocked by main window minimization)
                    if overlay_enabled_clone.load(Ordering::Relaxed) {
                        let max_items = overlay_max_count_clone.load(Ordering::Relaxed);
                        let font_size = overlay_font_size_clone.load(Ordering::Relaxed);
                        let total = state.translations.len();
                        let start = total.saturating_sub(max_items);
                        let visible = state.translations[start..]
                            .iter()
                            .map(|translation| overlay_ipc::OverlayEntry {
                                source: translation.source.clone(),
                                translated: translation.translated.clone(),
                                live: translation.live,
                                vad_active: match translation.audio_source {
                                    CaptureSource::Microphone => {
                                        microphone_vad_active_clone.load(Ordering::Relaxed)
                                    }
                                    CaptureSource::SystemAudio => {
                                        loopback_vad_active_clone.load(Ordering::Relaxed)
                                    }
                                    CaptureSource::Both => false,
                                },
                            })
                            .collect();

                        let overlay_state = overlay_ipc::OverlayState {
                            font_size,
                            max_items,
                            visible_entries: visible,
                            partial_text: if state.partial_text.is_empty() {
                                None
                            } else {
                                Some(state.partial_text.clone())
                            },
                            vad_active: microphone_vad_active_clone.load(Ordering::Relaxed)
                                || loopback_vad_active_clone.load(Ordering::Relaxed),
                        };

                        if let Ok(mut mgr) = overlay_mgr_clone.lock() {
                            mgr.send_state(&overlay_state);
                        }
                    }
                }
            })
            .expect("failed to spawn session-event-pump thread");

        let prompt_provider = service_config.translation_prompt_target();
        let (first_run, onboarding_page) = onboarding::resolve_startup_onboarding_state(
            settings.first_run,
            &backend_manager.project_root(),
            &service_config,
            &backend_manager,
            &model_task_manager,
            &runtime_installer,
        );
        let mut app = Self {
            audio_system,
            devices,
            device_refresh_rx: None,
            selected_device_id: settings.selected_device_id,
            loopback_devices,
            selected_loopback_device_id: settings.selected_loopback_device_id,
            tts_output_devices,
            selected_tts_output_device_id: settings.selected_tts_output_device_id,
            capture_source: settings.capture_source,
            microphone_recognition: settings.microphone_recognition,
            loopback_recognition: settings.loopback_recognition,
            selected_input_config,
            is_translating: false,
            session_owner: TranslationSessionOwner::None,
            audio_txs: Vec::new(),
            input_level,
            loopback_level,
            microphone_vad_active,
            loopback_vad_active,
            sessions: Vec::new(),
            meeting_audio_routers: Vec::new(),
            event_tx,
            connection_status: "Ready".into(),
            partial_text: String::new(),
            recognition_history: Vec::new(),
            translations: Vec::new(),
            last_error: None,
            server_url: settings.server_url,
            download_proxy_url: settings.download_proxy_url,
            update_channel: settings.update_channel,
            source_lang: settings.source_lang,
            target_lang: settings.target_lang,
            denoise_enabled: settings.denoise_enabled,
            tts_enabled: settings.tts_enabled && service_config.tts_is_configured(),
            microphone_clone_state: None,
            loopback_clone_state: None,
            tts_runtime_backend: None,
            tts_runtime_cuda_version: None,
            osc_plugin,
            meeting_plugin,
            player_plugin,
            host_audio_import: None,
            pending_audio_import: None,
            plugin_preferences: settings.plugin_preferences,
            service_config,
            backend_manager,
            model_task_manager,
            runtime_installer,
            app_update_manager,
            notified_update_version: None,
            notified_ready_update_version: None,
            backend_start_deadline: None,
            settings_section: ui::pages::settings::SettingsSection::default(),
            prompt_library: settings.prompt_library,
            prompt_studio: ui::pages::prompt_studio::PromptStudioController::for_provider(
                prompt_provider,
            ),
            modal_dialog: ui::modal::ModalDialog::default(),
            pending_resource_deletion: None,
            first_run,
            onboarding_page,
            ui_language: settings.ui_language,
            navigation: NavigationState {
                collapsed: settings.sidebar_collapsed,
                page: settings.active_page,
            },
            window_backdrop: window_backdrop::WindowBackdrop::default(),
            mute_self_pauses_translation: Arc::new(AtomicBool::new(
                settings.mute_self_pauses_translation,
            )),
            floating_subtitles_enabled: settings.floating_subtitles_enabled,
            floating_subtitles_max_count: settings.floating_subtitles_max_count,
            floating_subtitles_font_size: settings.floating_subtitles_font_size,
            overlay_manager,
            shared_session_state,
            overlay_enabled_atomic,
            overlay_max_count_atomic,
            overlay_font_size_atomic,
        };
        app.check_for_updates();
        app
    }
}

impl XRTranslateApp {
    pub fn project_root(&self) -> std::path::PathBuf {
        self.backend_manager.project_root()
    }

    fn request_model_resource_deletion(&mut self, asset_id: xrtranslate_assets::ModelAssetId) {
        let label = xrtranslate_assets::manifest_for(asset_id).label;
        self.pending_resource_deletion = Some(PendingResourceDeletion::Model(asset_id));
        self.modal_dialog =
            ui::modal::ModalDialog::confirm_resource_deletion(label, self.ui_language);
    }

    fn request_runtime_resource_deletion(&mut self) {
        self.pending_resource_deletion = Some(PendingResourceDeletion::Runtime);
        self.modal_dialog = ui::modal::ModalDialog::confirm_resource_deletion(
            i18n::tr(
                self.ui_language,
                "Inference Runtime & Hardware Acceleration",
            ),
            self.ui_language,
        );
    }

    fn confirm_pending_resource_deletion(&mut self) {
        let Some(resource) = self.pending_resource_deletion.take() else {
            return;
        };
        let project_root = self.project_root();
        self.backend_manager.shutdown();
        let result = match resource {
            PendingResourceDeletion::Model(asset_id) => {
                self.model_task_manager.delete(&project_root, asset_id)
            }
            PendingResourceDeletion::Runtime => self
                .runtime_installer
                .delete_managed_resources(&project_root),
        };
        match result {
            Ok(()) => self.last_error = None,
            Err(error) => self.last_error = Some(error),
        }
    }

    fn render_modal_layer(&mut self, ctx: &egui::Context) {
        self.modal_dialog.render(ctx, self.ui_language);
        let modal_action = self.modal_dialog.take_action();
        match modal_action {
            Some(ui::modal::ModalAction::DownloadUpdate) => self.download_update(),
            Some(ui::modal::ModalAction::InstallUpdate) => self.install_update_and_restart(),
            Some(ui::modal::ModalAction::ConfirmResourceDeletion) => {
                self.confirm_pending_resource_deletion()
            }
            None => {}
        }
        if !self.modal_dialog.open && modal_action.is_none() {
            self.pending_resource_deletion = None;
        }
    }

    pub(crate) fn plugin_enabled(&self, id: PluginId) -> bool {
        PluginRegistry::builtin().is_enabled(&self.plugin_preferences, id)
    }

    /// Selects the first plugin currently requesting the exclusive translation
    /// capability. Concrete plugins implement the same neutral contract; the
    /// session infrastructure consumes only the returned binding.
    fn active_plugin_session(&self) -> Option<PluginSessionBinding> {
        let plugins: [&dyn TranslationSessionPlugin; 2] =
            [&self.meeting_plugin, &self.player_plugin];
        plugins
            .into_iter()
            .find_map(TranslationSessionPlugin::translation_session_binding)
    }

    fn session_config(
        &mut self,
        plugin: Option<&PluginSessionBinding>,
        recognition: &RecognitionSettings,
        audio_source: CaptureSource,
        vad_threshold: f32,
        ctx: Option<eframe::egui::Context>,
    ) -> SessionConfig {
        let publish_to_host_outputs =
            plugin.is_none_or(PluginSessionBinding::publish_to_host_outputs);
        let host_tts = plugin.is_none_or(|binding| binding.host_tts);
        let external_audio_gate = plugin.is_none_or(|binding| binding.external_audio_gate);
        let finish_when_audio_ends = plugin.is_some_and(|binding| binding.finish_when_audio_ends);
        let tts = if host_tts
            && crate::feature_access::is_available(crate::feature_access::Feature::TtsPlayback)
        {
            match self.audio_system.tts_handle(
                self.service_config.tts_sample_rate(),
                &self.selected_tts_output_device_id,
            ) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    self.last_error = Some(format!("TTS output is unavailable: {error}"));
                    None
                }
            }
        } else {
            None
        };

        SessionConfig {
            server_url: self.server_url.clone(),
            source_lang: self.source_lang.clone(),
            target_lang: self.target_lang.clone(),
            external_audio_gate: if external_audio_gate {
                ExternalAudioGate::new(
                    self.osc_plugin.mute_state(),
                    Arc::clone(&self.mute_self_pauses_translation),
                )
            } else {
                ExternalAudioGate::default()
            },
            publish_to_host_outputs,
            tts,
            egui_ctx: ctx,
            vad_threshold,
            vad_silence_ms: pause_tolerance_to_ms(recognition.pause_tolerance),
            continuous_recognition: recognition.continuous_recognition,
            audio_source,
            finish_when_audio_ends,
            prompt_graph: Some(self.prompt_library.active_graph()),
        }
    }

    pub(crate) fn activate_prompt_template(&mut self, id: String) {
        let Some(graph) = self
            .prompt_library
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .map(|profile| profile.graph.clone())
        else {
            return;
        };
        if let Err(error) = graph.validate_for_activation() {
            log::error!("Cannot activate invalid prompt graph: {error}");
            return;
        }
        self.prompt_library.active_id = id;
        self.prompt_studio.set_runtime_trace(None);
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.latest_asr_prompt_trace = None;
            state.latest_translation_prompt_trace = None;
        }
        for session in &self.sessions {
            session.update_prompt_template(graph.clone());
        }
        self.save_settings();
    }

    fn apply_prompt_studio_actions(
        &mut self,
        actions: Vec<ui::pages::prompt_studio::PromptStudioAction>,
    ) {
        use ui::pages::prompt_studio::PromptStudioAction;

        for action in actions {
            match action {
                PromptStudioAction::SelectProfile(id) => {
                    self.prompt_studio.select_profile(id, &self.prompt_library);
                }
                PromptStudioAction::CreateProfile(profile) => {
                    let id = profile.id.clone();
                    self.commit_prompt_profile(profile);
                    self.prompt_studio.select_profile(id, &self.prompt_library);
                }
                PromptStudioAction::CloneProfile(profile) => {
                    let id = profile.id.clone();
                    self.commit_prompt_profile(profile);
                    self.prompt_studio.select_profile(id, &self.prompt_library);
                }
                PromptStudioAction::DeleteProfile(id) => {
                    if self.prompt_library.profiles.len() > 1
                        && !self
                            .prompt_library
                            .profiles
                            .iter()
                            .any(|profile| profile.id == id && profile.read_only)
                    {
                        self.prompt_library
                            .profiles
                            .retain(|profile| profile.id != id);
                        self.prompt_library.normalize();
                        self.prompt_studio.select_profile(
                            self.prompt_library.active_id.clone(),
                            &self.prompt_library,
                        );
                        self.save_settings();
                    }
                }
                PromptStudioAction::ActivateProfile(profile) => {
                    self.commit_prompt_profile(profile.clone());
                    self.activate_prompt_template(profile.id);
                }
                PromptStudioAction::SaveProfile(profile) => {
                    self.commit_prompt_profile(profile);
                }
                PromptStudioAction::ExportProfile(profile) => {
                    let clean_name = sanitize_graph_file_name(&profile.name);
                    let default_name = format!("{clean_name}.json");
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Prompt Graph (*.json)", &["json"])
                        .set_file_name(&default_name)
                        .save_file()
                    {
                        if let Ok(json) = profile.export_project_json() {
                            let _ = std::fs::write(&path, json);
                        }
                    }
                }
                PromptStudioAction::ImportProfile => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Prompt Graph (*.json)", &["json"])
                        .pick_file()
                    {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let new_id = format!("custom-import-{}", uuid::Uuid::new_v4());
                            if let Ok(mut imported) =
                                xrtranslate_prompt::PromptTemplateProfile::import_project_json(
                                    &content, new_id,
                                )
                            {
                                if imported.name == "Imported Graph"
                                    || imported.name.trim().is_empty()
                                {
                                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                        let clean_stem = stem.trim();
                                        if !clean_stem.is_empty() {
                                            imported.name = clean_stem.to_string();
                                        }
                                    }
                                }
                                let id = imported.id.clone();
                                self.commit_prompt_profile(imported);
                                self.prompt_studio.select_profile(id, &self.prompt_library);
                                self.save_settings();
                            }
                        }
                    }
                }
            }
        }
    }

    fn commit_prompt_profile(&mut self, profile: xrtranslate_prompt::PromptTemplateProfile) {
        if self
            .prompt_library
            .profiles
            .iter()
            .any(|existing| existing.id == profile.id && existing.read_only)
        {
            return;
        }
        if let Some(existing) = self
            .prompt_library
            .profiles
            .iter_mut()
            .find(|existing| existing.id == profile.id)
        {
            *existing = profile;
        } else {
            self.prompt_library.profiles.push(profile);
        }
        self.prompt_library.normalize();
        self.save_settings();
    }

    pub(crate) fn open_plugin(&mut self, id: PluginId) {
        if self.plugin_enabled(id) {
            self.navigation.page = Page::Plugin(id);
            self.save_settings();
        }
    }

    pub(crate) fn plugin_disable_block_reason(&self, id: PluginId) -> Option<String> {
        match id {
            PluginId::MEETING => self
                .meeting_plugin
                .disable_block_reason()
                .map(str::to_owned),
            PluginId::VIDEO_PLAYER => {
                if self
                    .session_owner
                    .is_plugin(PluginId::VIDEO_PLAYER.as_str())
                    || self.player_plugin.has_active_task()
                {
                    Some("Stop the active video playback before disabling this plugin".into())
                } else {
                    None
                }
            }
            PluginId::OSC => None,
            _ => None,
        }
    }

    pub(crate) fn set_plugin_enabled(&mut self, id: PluginId, enabled: bool) {
        if self.plugin_enabled(id) == enabled {
            return;
        }
        if !enabled && let Some(reason) = self.plugin_disable_block_reason(id) {
            self.last_error = Some(reason);
            return;
        }

        let lifecycle = match id {
            PluginId::OSC if enabled => self.osc_plugin.activate(),
            PluginId::OSC => self.osc_plugin.deactivate(),
            PluginId::MEETING => Ok(()),
            _ => Ok(()),
        };
        if let Err(error) = lifecycle {
            self.last_error = Some(error);
            return;
        }

        let registry = PluginRegistry::builtin();
        registry.set_enabled(&mut self.plugin_preferences, id, enabled);
        registry.normalize_active_page(&self.plugin_preferences, &mut self.navigation.page);
        self.save_settings();
    }

    pub(crate) fn render_plugin_settings(&mut self, id: PluginId, ui: &mut egui::Ui) {
        if id != PluginId::OSC {
            return;
        }
        let actions = self.osc_plugin.render_settings(ui, self.ui_language);
        self.apply_osc_actions(actions);
    }

    fn apply_osc_actions(&mut self, actions: Vec<OscUiAction>) {
        for action in actions {
            match action {
                OscUiAction::ClearHostHistory => self.clear_history(),
                OscUiAction::SetMuteGateEnabled(enabled) => {
                    self.set_mute_self_pauses_translation(enabled)
                }
                OscUiAction::SetSpeakerNumberVisible(enabled) => {
                    self.set_osc_speaker_number_visible(enabled)
                }
                OscUiAction::SaveSettings => self.save_settings(),
                OscUiAction::SettingsApplied(result) => match result {
                    Ok(()) => self.last_error = None,
                    Err(error) => self.last_error = Some(error),
                },
                OscUiAction::TranslateInput {
                    text,
                    source_lang,
                    target_lang,
                } => self.translate_text(&text, Some(source_lang), Some(target_lang)),
            }
        }
    }

    fn render_osc_plugin_page(&mut self, ui: &mut egui::Ui) {
        let mute_gate_enabled = self.mute_self_pauses_translation.load(Ordering::Acquire);
        let actions = self.osc_plugin.render_page(
            ui,
            OscPageContext {
                language: self.ui_language,
                last_error: self.last_error.as_deref(),
                mute_gate_enabled,
            },
        );
        self.apply_osc_actions(actions);
    }

    fn meeting_ui_snapshot(&self) -> MeetingUiSnapshot {
        MeetingUiSnapshot {
            default_audio_source: capture_source_to_meeting(self.capture_source),
            default_source_language: self.source_lang.clone(),
            default_target_language: self.target_lang.clone(),
            host_session_busy: self.is_translating
                && self.meeting_plugin.controller.active_meeting_id().is_none(),
            language: self.ui_language,
        }
    }

    fn render_meeting_plugin_page(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.meeting_ui_snapshot();
        let action = self.meeting_plugin.render_page(&snapshot, ui);
        self.apply_meeting_action(action, ui.ctx().clone());
    }

    fn render_player_plugin_page(&mut self, ui: &mut egui::Ui) {
        let snapshot = plugins::player::VideoPlayerUiSnapshot {
            language: self.ui_language,
        };
        let action = self.player_plugin.render_page(&snapshot, ui);
        self.apply_video_player_action(action, ui.ctx().clone());
    }

    fn apply_video_player_action(
        &mut self,
        action: plugins::player::VideoPlayerAction,
        ctx: egui::Context,
    ) {
        match action {
            plugins::player::VideoPlayerAction::None => {}
            plugins::player::VideoPlayerAction::StopTranslation => {
                self.stop();
            }
            plugins::player::VideoPlayerAction::StartTranslation(request) => {
                if self.is_translating {
                    self.stop();
                }
                match request {
                    plugins::player::PlayerTranslationRequest::ImportMediaFile {
                        path,
                        source_language,
                        target_language,
                        recognition,
                        audio_channels,
                    } => {
                        let recognition_channels = audio_channels
                            .iter()
                            .filter(|c| c.recognition)
                            .map(|c| c.index)
                            .collect::<Vec<usize>>();
                        self.start_audio_file_translation(
                            path,
                            source_language,
                            target_language,
                            recognition,
                            recognition_channels,
                            media_import::AudioImportPacing::AsFastAsPossible,
                            Some(ctx),
                        );
                    }
                    plugins::player::PlayerTranslationRequest::LiveStream {
                        source_language,
                        target_language,
                        recognition,
                        audio_channels: _,
                    } => {
                        self.source_lang = source_language;
                        self.target_lang = target_language;
                        self.loopback_recognition = recognition;
                        self.start(Some(ctx));
                    }
                }
            }
        }
    }

    pub(crate) fn start_audio_file_translation(
        &mut self,
        path: std::path::PathBuf,
        source_language: String,
        target_language: String,
        recognition: RecognitionSettings,
        recognition_channels: Vec<usize>,
        pacing: media_import::AudioImportPacing,
        ctx: Option<eframe::egui::Context>,
    ) {
        self.source_lang = source_language;
        self.target_lang = target_language;
        match self.backend_manager.prepare(&self.server_url) {
            Ok(backend::BackendStart::Ready) => {
                self.start_audio_file_session(path, recognition, recognition_channels, pacing, ctx);
            }
            Ok(backend::BackendStart::Starting(stage)) => {
                self.pending_audio_import = Some((path, recognition, recognition_channels, pacing));
                self.backend_start_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(180));
                self.set_connection_status(stage.message());
            }
            Err(error) => {
                self.set_startup_error("Startup failed", error);
            }
        }
    }

    fn start_audio_file_session(
        &mut self,
        path: std::path::PathBuf,
        recognition: RecognitionSettings,
        recognition_channels: Vec<usize>,
        pacing: media_import::AudioImportPacing,
        ctx: Option<eframe::egui::Context>,
    ) {
        let Some(plugin_session) = self.player_plugin.translation_session_binding() else {
            self.set_startup_error(
                "Media import failed",
                "Media Player did not provide an active session binding".into(),
            );
            return;
        };
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<Vec<f32>>(64);
        let config = self.session_config(
            Some(&plugin_session),
            &recognition,
            CaptureSource::SystemAudio,
            recognition.background_noise.clamp(0.02, 0.95),
            ctx,
        );
        let session = start_session(audio_rx, self.event_tx.clone(), config);
        match media_import::import_audio_file(
            path,
            audio_tx,
            media_import::AudioImportOptions {
                chunk_frames: 1_600,
                pacing,
                recognition_channels,
            },
        ) {
            Ok(import) => {
                self.sessions = vec![session];
                self.host_audio_import = Some(import);
                self.audio_txs.clear();
                self.session_owner = TranslationSessionOwner::Plugin(plugin_session.owner);
                self.is_translating = true;
                if let Ok(mut state) = self.shared_session_state.lock() {
                    state.is_translating = true;
                    state.connection_status = "Transcribing and translating media audio...".into();
                    state.translations.clear();
                    state.recognition_history.clear();
                }
                self.set_connection_status("Transcribing and translating media audio...");
            }
            Err(error) => {
                session.finish();
                self.player_plugin.pause_task();
                self.set_startup_error("Media import failed", error.to_string());
                log::error!("Media audio import failed: {error}");
            }
        }
    }

    fn render_plugin_page(&mut self, id: PluginId, ui: &mut egui::Ui) {
        match id {
            PluginId::MEETING => self.render_meeting_plugin_page(ui),
            PluginId::VIDEO_PLAYER => self.render_player_plugin_page(ui),
            PluginId::OSC => self.render_osc_plugin_page(ui),
            _ => self.navigation.page = Page::Translation,
        }
    }

    fn apply_meeting_action(&mut self, action: MeetingAction, ctx: egui::Context) {
        match action {
            MeetingAction::None => {}
            MeetingAction::CreateAndStart(request) => {
                if self.is_translating && !self.session_owner.is_plugin(PluginId::MEETING.as_str())
                {
                    self.stop();
                }
                self.source_lang = request.source_language.clone();
                self.target_lang = request.target_language.clone();
                if let MeetingInputRequest::Live { source, .. } = &request.input {
                    self.capture_source = meeting_source_to_capture(*source);
                }
                let import_path = match &request.input {
                    MeetingInputRequest::ImportedAudio { path } => Some(path.clone()),
                    MeetingInputRequest::Live { .. } => None,
                };
                if let Some(id) = self.meeting_plugin.controller.create(&request)
                    && self.meeting_plugin.controller.begin_capture(&id)
                {
                    if let Some(path) = import_path {
                        self.start_audio_import(path, Some(ctx));
                    } else {
                        self.start(Some(ctx));
                    }
                }
            }
            MeetingAction::Continue(id) => {
                if self.is_translating && !self.session_owner.is_plugin(PluginId::MEETING.as_str())
                {
                    self.stop();
                }
                let resumed_in_place = self
                    .meeting_plugin
                    .controller
                    .active_meeting_id()
                    .as_deref()
                    == Some(id.as_str())
                    && self.resume_active_meeting();
                if !resumed_in_place && self.meeting_plugin.controller.begin_capture(&id) {
                    if let Ok(meeting) = self.meeting_plugin.controller.meeting(&id) {
                        self.source_lang = meeting.source_language;
                        self.target_lang = meeting.target_language;
                        self.capture_source = meeting
                            .input_source
                            .as_deref()
                            .map(meeting_source_name_to_capture)
                            .unwrap_or(CaptureSource::Microphone);
                    }
                    self.start(Some(ctx));
                }
            }
            MeetingAction::Pause => self.pause_active_meeting(),
            MeetingAction::End => {
                self.meeting_plugin.event_sink.request_finish();
                self.stop();
            }
            MeetingAction::Export(meeting_id) => {
                self.meeting_plugin.controller.open_meeting(&meeting_id);
                self.export_open_meeting_markdown();
            }
            MeetingAction::Reprocess(request) => {
                if self.is_translating && !self.session_owner.is_plugin(PluginId::MEETING.as_str())
                {
                    self.stop();
                }
                if self
                    .meeting_plugin
                    .controller
                    .begin_capture(&request.meeting_id)
                {
                    self.meeting_plugin
                        .controller
                        .create_capture_topic(&request.meeting_id, &request.topic_title);
                    self.start_audio_import(request.audio_path, Some(ctx));
                }
            }
        }
    }

    pub fn save_settings(&self) {
        let settings = ClientSettings {
            capture_source: self.capture_source,
            selected_device_id: self.selected_device_id.clone(),
            selected_loopback_device_id: self.selected_loopback_device_id.clone(),
            selected_tts_output_device_id: self.selected_tts_output_device_id.clone(),
            background_noise: self.microphone_recognition.background_noise,
            pause_tolerance: self.microphone_recognition.pause_tolerance,
            continuous_recognition: self.microphone_recognition.continuous_recognition,
            microphone_recognition: self.microphone_recognition.clone(),
            loopback_recognition: self.loopback_recognition.clone(),
            source_lang: self.source_lang.clone(),
            target_lang: self.target_lang.clone(),
            denoise_enabled: self.denoise_enabled,
            tts_enabled: self.tts_enabled,
            mute_self_pauses_translation: self.mute_self_pauses_translation.load(Ordering::Relaxed),
            ui_language: self.ui_language,
            first_run: self.first_run,
            server_url: self.server_url.clone(),
            download_proxy_url: self.download_proxy_url.clone(),
            update_channel: self.update_channel,
            osc_settings: self.osc_plugin.draft().clone(),
            plugin_preferences: self.plugin_preferences.clone(),
            active_page: self.navigation.page,
            sidebar_collapsed: self.navigation.collapsed,
            floating_subtitles_enabled: self.floating_subtitles_enabled,
            floating_subtitles_max_count: self.floating_subtitles_max_count,
            floating_subtitles_font_size: self.floating_subtitles_font_size,
            prompt_library: self.prompt_library.clone(),
        };
        if let Err(e) = settings.save(&self.project_root()) {
            log::error!("Failed to save client settings: {e}");
        }
    }

    pub fn finish_onboarding(&mut self) {
        if self.service_config.has_unsaved_changes()
            && self.service_config.save_onboarding_configuration().is_err()
        {
            self.onboarding_page = 1;
            return;
        }
        if onboarding::has_unmet_prerequisites(
            &self.project_root(),
            &self.service_config,
            &self.backend_manager,
            &self.model_task_manager,
            &self.runtime_installer,
        ) {
            return;
        }
        self.first_run = false;
        self.save_settings();
    }

    pub fn set_ui_language(&mut self, language: UiLanguage) {
        self.ui_language = language;
        self.save_settings();
    }

    pub fn app_update_state(&self) -> &app_update::AppUpdateState {
        self.app_update_manager.state()
    }

    pub fn check_for_updates(&mut self) {
        if let Err(error) = self.app_update_manager.check() {
            self.last_error = Some(error);
        }
    }

    pub fn set_download_proxy_url(&mut self, proxy_url: String) {
        self.download_proxy_url = proxy_url.trim().to_owned();
        self.model_task_manager
            .set_proxy_url(&self.download_proxy_url);
        self.runtime_installer
            .set_proxy_url(&self.download_proxy_url);
        self.app_update_manager
            .set_proxy_url(&self.download_proxy_url);
        self.save_settings();
    }

    pub fn set_update_channel(&mut self, channel: client_settings::UpdateChannel) {
        self.update_channel = channel;
        self.app_update_manager.set_channel(channel);
        self.save_settings();
        self.check_for_updates();
    }

    fn show_available_update(&mut self) {
        let app_update::AppUpdateState::Available(info) = self.app_update_manager.state() else {
            return;
        };
        if self.notified_update_version.as_deref() == Some(&info.version) {
            return;
        }
        self.notified_update_version = Some(info.version.clone());
        self.modal_dialog =
            ui::modal::ModalDialog::update_available(&info.version, self.ui_language);
    }

    fn show_ready_update(&mut self) {
        let app_update::AppUpdateState::Ready(info) = self.app_update_manager.state() else {
            return;
        };
        if self.notified_ready_update_version.as_deref() == Some(&info.version) {
            return;
        }
        self.notified_ready_update_version = Some(info.version.clone());
        self.modal_dialog = ui::modal::ModalDialog::update_ready(&info.version, self.ui_language);
    }

    pub fn download_update(&mut self) {
        if let Err(error) = self.app_update_manager.download(self.project_root()) {
            self.last_error = Some(error);
        }
    }

    pub fn install_update_and_restart(&mut self) {
        let install = match self.app_update_manager.begin_install() {
            Ok(install) => install,
            Err(error) => {
                self.last_error = Some(error);
                return;
            }
        };
        self.stop();
        self.backend_start_deadline = None;
        self.backend_manager.shutdown();
        if let Ok(mut overlay) = self.overlay_manager.lock() {
            overlay.stop();
        }
        match app_update::spawn_updater(install) {
            Ok(()) => std::process::exit(0),
            Err(error) => self.last_error = Some(error),
        }
    }

    fn set_connection_status(&mut self, status: impl Into<String>) {
        let status = status.into();
        self.connection_status.clone_from(&status);
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.connection_status = status;
        }
    }

    fn set_startup_error(&mut self, status: &str, error: String) {
        self.set_connection_status(status);
        self.last_error = Some(error.clone());
        self.meeting_plugin.fail_active_startup(&error);
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.last_error = Some(error);
            state.is_translating = false;
        }
    }

    pub fn start(&mut self, ctx: Option<eframe::egui::Context>) {
        if self.backend_start_deadline.is_some() {
            return;
        }
        match self.backend_manager.prepare(&self.server_url) {
            Ok(backend::BackendStart::Ready) => self.start_session(ctx),
            Ok(backend::BackendStart::Starting(stage)) => {
                self.backend_start_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(180));
                self.set_connection_status(stage.message());
                self.last_error = None;
                if let Ok(mut state) = self.shared_session_state.lock() {
                    state.last_error = None;
                }
            }
            Err(error) => self.set_startup_error("Startup failed", error),
        }
    }

    pub(crate) fn start_audio_import(
        &mut self,
        path: std::path::PathBuf,
        ctx: Option<eframe::egui::Context>,
    ) {
        if self.backend_start_deadline.is_some() || self.meeting_plugin.has_audio_import() {
            return;
        }
        self.meeting_plugin.controller.mark_imported_audio();
        match self.backend_manager.prepare(&self.server_url) {
            Ok(backend::BackendStart::Ready) => self.start_audio_import_session(path, ctx),
            Ok(backend::BackendStart::Starting(stage)) => {
                self.meeting_plugin.set_pending_audio_import(path);
                self.backend_start_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(180));
                self.set_connection_status(stage.message());
            }
            Err(error) => {
                self.meeting_plugin.set_error(error.clone());
                self.set_startup_error("Startup failed", error);
            }
        }
    }

    fn start_audio_import_session(
        &mut self,
        path: std::path::PathBuf,
        ctx: Option<eframe::egui::Context>,
    ) {
        let Some(plugin_session) = self.meeting_plugin.translation_session_binding() else {
            self.meeting_plugin
                .set_error("Meeting did not provide an active session binding");
            return;
        };
        self.meeting_plugin.event_sink.begin_sessions(1);
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<Vec<f32>>(32);
        let recognition = self.loopback_recognition.clone();
        let config = self.session_config(
            Some(&plugin_session),
            &recognition,
            CaptureSource::SystemAudio,
            vad_threshold_for_background_noise(recognition.background_noise),
            ctx,
        );
        let session = start_session(audio_rx, self.event_tx.clone(), config);
        match media_import::import_audio_file(
            path,
            audio_tx,
            media_import::AudioImportOptions::default(),
        ) {
            Ok(import) => {
                self.sessions = vec![session];
                self.meeting_plugin.set_audio_import(import);
                self.audio_txs.clear();
                self.session_owner = TranslationSessionOwner::Plugin(plugin_session.owner);
                self.is_translating = true;
                if let Ok(mut state) = self.shared_session_state.lock() {
                    state.is_translating = true;
                    state.connection_status = "Processing imported audio".into();
                }
                self.set_connection_status("Processing imported audio");
            }
            Err(error) => {
                session.finish();
                self.meeting_plugin.event_sink.cancel_sessions();
                let error = error.to_string();
                self.meeting_plugin.set_error(error.clone());
                if let Some(store_error) =
                    self.meeting_plugin.controller.fail_active_meeting(&error)
                {
                    self.meeting_plugin.set_error(store_error.to_string());
                }
            }
        }
    }

    pub(crate) fn apply_service_configuration(&mut self, ctx: Option<eframe::egui::Context>) {
        if self.session_owner.is_plugin(PluginId::MEETING.as_str())
            || self.meeting_plugin.controller.active_meeting_id().is_some()
        {
            self.meeting_plugin
                .set_error("Finish the active meeting before changing service configuration");
            self.navigation.page = Page::Plugin(PluginId::MEETING);
            return;
        }
        if self
            .session_owner
            .is_plugin(PluginId::VIDEO_PLAYER.as_str())
            || self.player_plugin.has_active_task()
        {
            self.player_plugin
                .set_error("Stop the active video task before changing service configuration");
            self.navigation.page = Page::Plugin(PluginId::VIDEO_PLAYER);
            return;
        }
        self.prompt_studio
            .sync_provider(self.service_config.translation_prompt_target());
        if !self.service_config.tts_is_configured() {
            self.tts_enabled = false;
            self.audio_system.clear_tts_playback();
        }
        let resume_translation = self.is_translating;
        if resume_translation {
            self.stop();
        }
        self.backend_start_deadline = None;
        self.backend_manager.shutdown();
        self.model_task_manager.invalidate_discovery();
        let requirements = self.service_config.runtime_requirements();
        if !self.runtime_installer.is_busy()
            && !self.runtime_installer.plan_matches(requirements)
            && let Err(error) = self
                .runtime_installer
                .prepare_for(self.project_root(), requirements)
        {
            self.last_error = Some(error);
        }
        if requirements.llama_cpp && !self.backend_manager.llama_server_path_is_valid() {
            if !self.first_run {
                self.first_run = true;
                self.onboarding_page = 3;
            }
            self.set_connection_status("Ready");
            return;
        }
        if resume_translation {
            self.start(ctx);
        } else {
            self.set_connection_status("Ready");
        }
    }

    fn start_session(&mut self, ctx: Option<eframe::egui::Context>) {
        // A new WebSocket session must not inherit a stale visual state. The
        // backend will immediately re-announce any encoded voice retained by
        // its shared TTS adapter after audio-source configuration.
        self.microphone_clone_state = None;
        self.loopback_clone_state = None;
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.microphone_clone_state = None;
            state.loopback_clone_state = None;
        }
        let routes = self.capture_source.routes();
        let plugin_session = self.active_plugin_session();
        let publish_to_host_outputs = plugin_session
            .as_ref()
            .is_none_or(PluginSessionBinding::publish_to_host_outputs);
        let session_channels = routes
            .iter()
            .map(|_| bounded::<Vec<f32>>(LIVE_AUDIO_QUEUE_CAPACITY))
            .collect::<Vec<_>>();
        let recording_sink = self.start_meeting_recording();
        let mut meeting_audio_routers = Vec::new();
        let audio_txs = session_channels
            .iter()
            .zip(routes.iter())
            .map(|((session_tx, _), source)| {
                if let Some(sink) = recording_sink.clone() {
                    let (capture_tx, worker) =
                        spawn_meeting_audio_router(session_tx.clone(), sink, *source);
                    meeting_audio_routers.push(worker);
                    capture_tx
                } else {
                    session_tx.clone()
                }
            })
            .collect::<Vec<_>>();
        let session_configs = initialize_live_audio(
            self,
            |app| {
                routes
                    .iter()
                    .map(|source| {
                        let recognition = app.recognition_settings(*source).clone();
                        app.session_config(
                            plugin_session.as_ref(),
                            &recognition,
                            *source,
                            vad_threshold_for_background_noise(recognition.background_noise),
                            ctx.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            },
            |app| app.start_selected_capture(routes, &audio_txs),
        );
        match session_configs {
            Ok(session_configs) => {
                if !publish_to_host_outputs {
                    self.meeting_plugin.event_sink.begin_sessions(routes.len());
                }
                if let Some(binding) = &plugin_session {
                    self.session_owner = TranslationSessionOwner::Plugin(binding.owner.clone());
                } else {
                    self.session_owner = TranslationSessionOwner::Host {
                        capture_source: self.capture_source,
                    };
                }
                self.sessions = session_channels
                    .iter()
                    .zip(session_configs)
                    .map(|((_, audio_rx), config)| {
                        let session =
                            start_session(audio_rx.clone(), self.event_tx.clone(), config);
                        if crate::feature_access::is_available(
                            crate::feature_access::Feature::TtsPlayback,
                        ) {
                            session.set_tts_enabled(self.tts_enabled);
                        }
                        session
                    })
                    .collect();
                self.audio_txs = audio_txs;
                self.meeting_audio_routers = meeting_audio_routers;
                self.is_translating = true;
                self.connection_status = "Connecting...".into();
                self.last_error = None;
                self.partial_text.clear();
                self.recognition_history.clear();
                self.translations.clear();

                if let Ok(mut state) = self.shared_session_state.lock() {
                    state.connection_status = "Connecting...".into();
                    state.partial_text.clear();
                    state.pending_final_asr.clear();
                    state.pending_recognition_windows.clear();
                    state.recognition_history.clear();
                    state.translations.clear();
                    state.last_error = None;
                    state.is_translating = true;
                }
            }
            Err(error) => {
                drop(audio_txs);
                for worker in meeting_audio_routers {
                    let _ = worker.join();
                }
                if let Some(recording) = self.meeting_plugin.meeting_recording.take()
                    && let Err(recording_error) = recording.stop_without_finalizing()
                {
                    log::error!("Could not checkpoint failed meeting recording: {recording_error}");
                }
                self.set_startup_error("Audio input failed", error);
                self.reset_audio_levels();
            }
        }
    }

    fn start_meeting_recording(&mut self) -> Option<plugins::meeting::recording::RecordingSink> {
        let active = self
            .meeting_plugin
            .controller
            .active_capture
            .lock()
            .ok()
            .and_then(|active| active.clone())?;
        if active.imported_audio {
            return None;
        }
        let meeting = self
            .meeting_plugin
            .controller
            .store
            .get_meeting(&active.meeting_id)
            .ok()?;
        let root = meeting.recording_path?;
        let directory = std::path::PathBuf::from(root).join(&active.recognition_run_id);
        match plugins::meeting::recording::MeetingRecording::start(
            plugins::meeting::recording::RecordingConfig::new(directory),
        ) {
            Ok(recording) => {
                let sink = recording.sink();
                self.meeting_plugin.meeting_recording = Some(recording);
                Some(sink)
            }
            Err(error) => {
                self.meeting_plugin
                    .set_error(format!("Could not start meeting recording: {error}"));
                None
            }
        }
    }

    fn poll_backend_startup(&mut self, ctx: Option<eframe::egui::Context>) {
        let Some(deadline) = self.backend_start_deadline else {
            return;
        };
        match self.backend_manager.status(&self.server_url) {
            backend::BackendStatus::Ready => {
                self.backend_start_deadline = None;
                if let Some((path, recognition, recognition_channels, pacing)) =
                    self.pending_audio_import.take()
                {
                    self.start_audio_file_session(
                        path,
                        recognition,
                        recognition_channels,
                        pacing,
                        ctx,
                    );
                } else if let Some(path) = self.meeting_plugin.take_pending_audio_import() {
                    self.start_audio_import_session(path, ctx);
                } else {
                    self.start_session(ctx);
                }
            }
            backend::BackendStatus::Starting(stage) if std::time::Instant::now() < deadline => {
                self.set_connection_status(stage.message());
            }
            backend::BackendStatus::Starting(_) => {
                self.backend_start_deadline = None;
                self.backend_manager.shutdown();
                self.set_startup_error(
                    "Startup timed out",
                    "Local services did not become ready within 180 seconds".into(),
                );
            }
            backend::BackendStatus::Failed(error) => {
                self.backend_start_deadline = None;
                self.set_startup_error("Startup failed", error.clone());
                self.modal_dialog = ui::modal::ModalDialog::error(
                    "Backend Startup Failure",
                    "The native backend process failed to initialize or exited prematurely.",
                    Some(&error),
                );
            }
        }
    }

    fn refresh_selected_input_config(&mut self) {
        let result = match self.capture_source {
            CaptureSource::Microphone => self.audio_system.input_config(&self.selected_device_id),
            CaptureSource::SystemAudio => self
                .audio_system
                .loopback_config(&self.selected_loopback_device_id),
            CaptureSource::Both => self.audio_system.input_config(&self.selected_device_id),
        };
        match result {
            Ok(config) => {
                self.selected_input_config = Some(config);
                self.last_error = None;
            }
            Err(error) => {
                self.selected_input_config = None;
                self.last_error = Some(error);
            }
        }
    }

    fn request_audio_device_refresh(&mut self, ctx: eframe::egui::Context) {
        if self.device_refresh_rx.is_some() {
            return;
        }

        let (tx, rx) = bounded(1);
        self.device_refresh_rx = Some(rx);
        let spawn_result = std::thread::Builder::new()
            .name("audio-device-refresh".into())
            .spawn(move || {
                let audio_system = AudioSystem::new();
                let snapshot = AudioDeviceSnapshot {
                    devices: audio_system.available_devices(),
                    loopback_devices: audio_system.available_loopback_devices(),
                };
                let _ = tx.send(snapshot);
                ctx.request_repaint();
            });
        if let Err(error) = spawn_result {
            self.device_refresh_rx = None;
            self.last_error = Some(format!("Could not refresh audio devices: {error}"));
        }
    }

    fn poll_audio_device_refresh(&mut self) {
        let Some(rx) = &self.device_refresh_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(snapshot) => {
                self.device_refresh_rx = None;
                self.devices = snapshot.devices;
                self.loopback_devices = snapshot.loopback_devices;
                self.refresh_selected_input_config();
                if !self.is_translating {
                    self.reset_audio_levels();
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.device_refresh_rx = None;
                self.last_error = Some("Audio device refresh stopped unexpectedly".into());
            }
        }
    }

    fn poll_audio_import(&mut self) {
        let events = self
            .meeting_plugin
            .audio_import
            .as_ref()
            .map(|import| import.events().try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut completed = false;
        let mut terminal_error = None;
        for event in events {
            match event {
                media_import::AudioImportEvent::Started(info) => {
                    self.set_connection_status(format!(
                        "Processing {} Hz, {} channel audio",
                        info.source_sample_rate, info.source_channels
                    ));
                }
                media_import::AudioImportEvent::Progress(progress) => {
                    let percentage = progress.fraction.map(|value| value * 100.0);
                    self.set_connection_status(percentage.map_or_else(
                        || format!("Processing audio · {}s", progress.position.as_secs()),
                        |value| format!("Processing audio · {value:.0}%"),
                    ));
                }
                media_import::AudioImportEvent::Completed { .. } => completed = true,
                media_import::AudioImportEvent::Stopped { .. } => {
                    terminal_error = Some("Audio import stopped".to_owned())
                }
                media_import::AudioImportEvent::Error(error) => terminal_error = Some(error),
            }
        }
        if completed {
            // Dropping the completed importer disconnects its bounded audio
            // sender. The network producer then drains every queued frame and
            // emits `input_ended`; the meeting is ended only after the backend
            // acknowledges its ordered inference drain.
            self.meeting_plugin.clear_audio_import();
            self.set_connection_status("Finishing imported audio");
        }
        if let Some(error) = terminal_error {
            for session in &self.sessions {
                session.finish();
            }
            if self.meeting_plugin.controller.active_meeting_id().is_some() {
                if let Some(store_error) =
                    self.meeting_plugin.controller.fail_active_meeting(&error)
                {
                    self.meeting_plugin.set_error(store_error.to_string());
                }
            }
            self.meeting_plugin.clear_audio_import();
        }

        let host_events = self
            .host_audio_import
            .as_ref()
            .map(|import| import.events().try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut host_completed = false;
        for event in host_events {
            match event {
                media_import::AudioImportEvent::Started(info) => {
                    self.set_connection_status(format!(
                        "Processing media audio ({} Hz)",
                        info.source_sample_rate
                    ));
                }
                media_import::AudioImportEvent::Progress(progress) => {
                    let percentage = progress.fraction.map(|value| value * 100.0);
                    match progress.stage {
                        media_import::AudioImportStage::Extracting => {
                            self.player_plugin.update_import_progress(
                                plugins::player::ImportProgressStage::Extracting,
                                progress.fraction,
                                progress.position,
                                progress.duration,
                            );
                            self.set_connection_status(percentage.map_or_else(
                                || {
                                    format!(
                                        "Extracting media audio · {}s",
                                        progress.position.as_secs()
                                    )
                                },
                                |value| format!("Extracting media audio · {value:.0}%"),
                            ));
                        }
                        media_import::AudioImportStage::Recognizing => {
                            self.player_plugin.update_import_progress(
                                plugins::player::ImportProgressStage::Recognizing,
                                progress.fraction,
                                progress.position,
                                progress.duration,
                            );
                            self.set_connection_status(percentage.map_or_else(
                                || {
                                    format!(
                                        "Transcribing media audio · {}s",
                                        progress.position.as_secs()
                                    )
                                },
                                |value| format!("Transcribing media audio · {value:.0}%"),
                            ));
                        }
                    }
                }
                media_import::AudioImportEvent::Completed { .. } => {
                    self.player_plugin.complete_import();
                    host_completed = true;
                }
                media_import::AudioImportEvent::Stopped { .. } => {
                    self.player_plugin.stop_import();
                    self.host_audio_import = None;
                }
                media_import::AudioImportEvent::Error(error) => {
                    log::error!("Host audio import error: {error}");
                    self.player_plugin.stop_import();
                    self.player_plugin.set_error(error.clone());
                    self.set_startup_error("Media audio error", error);
                    self.host_audio_import = None;
                }
            }
        }
        if host_completed {
            self.host_audio_import = None;
            self.set_connection_status("Media translation completed");
        }
    }

    fn start_selected_capture(
        &mut self,
        routes: &[CaptureSource],
        audio_txs: &[Sender<Vec<f32>>],
    ) -> Result<(), String> {
        self.input_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.loopback_level
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.microphone_vad_active.store(false, Ordering::Relaxed);
        self.loopback_vad_active.store(false, Ordering::Relaxed);
        self.audio_system.stop();
        for (source, audio_tx) in routes.iter().zip(audio_txs) {
            let result = match source {
                CaptureSource::Microphone => self.audio_system.start_capture(
                    &self.selected_device_id,
                    audio_tx.clone(),
                    Arc::clone(&self.input_level),
                ),
                CaptureSource::SystemAudio => self.audio_system.start_loopback_capture(
                    &self.selected_loopback_device_id,
                    audio_tx.clone(),
                    Arc::clone(&self.loopback_level),
                ),
                CaptureSource::Both => unreachable!("Both expands into individual capture routes"),
            };
            if let Err(error) = result {
                self.audio_system.stop();
                return Err(error);
            }
        }
        Ok(())
    }

    fn reset_audio_levels(&self) {
        self.input_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.loopback_level
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.microphone_vad_active.store(false, Ordering::Relaxed);
        self.loopback_vad_active.store(false, Ordering::Relaxed);
    }

    fn switch_capture_device(&mut self, source: CaptureSource, previous_device_id: String) {
        self.refresh_selected_input_config();
        self.save_settings();
        if !self.is_translating {
            self.reset_audio_levels();
            return;
        }

        if self.audio_txs.is_empty() {
            self.last_error = Some("Active audio channel is unavailable".into());
            return;
        }
        let routes = self.capture_source.routes();
        let audio_txs = self.audio_txs.clone();
        match self.start_selected_capture(routes, &audio_txs) {
            Ok(()) => {
                self.connection_status = "Connected - microphone switched".into();
                self.last_error = None;
            }
            Err(error) => {
                match source {
                    CaptureSource::Microphone => self.selected_device_id = previous_device_id,
                    CaptureSource::SystemAudio => {
                        self.selected_loopback_device_id = previous_device_id
                    }
                    CaptureSource::Both => unreachable!("Device selectors use concrete routes"),
                }
                self.refresh_selected_input_config();
                self.save_settings();
                let rollback_error = self.start_selected_capture(routes, &audio_txs).err();
                self.last_error = Some(match rollback_error {
                    Some(rollback_error) => format!(
                        "Could not switch audio device: {error}; could not restore previous device: {rollback_error}"
                    ),
                    None => {
                        format!("Could not switch audio device: {error}; previous device restored")
                    }
                });
            }
        }
    }

    fn switch_capture_source(&mut self, previous_source: CaptureSource) {
        self.refresh_selected_input_config();
        self.save_settings();
        if !self.is_translating {
            self.reset_audio_levels();
            return;
        }
        if self.capture_source.routes().len() != previous_source.routes().len() {
            // Recreate sessions when the route count changes.
            for session in &self.sessions {
                session.stop();
            }
            self.sessions.clear();
            self.audio_txs.clear();
            self.audio_system.stop();
            self.is_translating = false;
            self.start_session(None);
            return;
        }
        if self.audio_txs.is_empty() {
            self.last_error = Some("Active audio channel is unavailable".into());
            return;
        }
        let routes = self.capture_source.routes();
        let audio_txs = self.audio_txs.clone();
        if let Err(error) = self.start_selected_capture(routes, &audio_txs) {
            self.capture_source = previous_source;
            self.refresh_selected_input_config();
            self.save_settings();
            self.last_error = Some(format!("Could not switch audio source: {error}"));
        } else {
            self.connection_status = "Connected - audio source switched".into();
            self.last_error = None;
            for (session, source) in self.sessions.iter().zip(routes) {
                let recognition = self.recognition_settings(*source);
                session.reset_audio_pipeline(
                    self.source_lang.clone(),
                    self.target_lang.clone(),
                    *source,
                    vad_threshold_for_background_noise(recognition.background_noise),
                    pause_tolerance_to_ms(recognition.pause_tolerance),
                    recognition.continuous_recognition,
                );
            }
        }
    }

    fn apply_language_route(&mut self) {
        if self.source_lang == "auto" && !self.target_lang.contains(',') {
            self.target_lang = "zh,en".into();
        } else if self.source_lang != "auto" && self.target_lang.contains(',') {
            self.target_lang = "en".into();
        }
        self.save_settings();
        for session in &self.sessions {
            session.update_language_route(self.source_lang.clone(), self.target_lang.clone());
        }
    }

    fn set_tts_enabled(&mut self, enabled: bool) {
        if enabled && !self.service_config.tts_is_configured() {
            self.tts_enabled = false;
            self.last_error =
                Some("Configure a TTS provider in Settings before enabling TTS.".into());
            return;
        }
        self.tts_enabled = enabled
            && crate::feature_access::is_available(crate::feature_access::Feature::TtsPlayback);
        self.save_settings();
        if !self.tts_enabled {
            self.audio_system.clear_tts_playback();
        }
        for session in &self.sessions {
            session.set_tts_enabled(self.tts_enabled);
        }
    }

    fn begin_voice_clone(&mut self, source: CaptureSource) {
        if let Some(index) = self
            .capture_source
            .routes()
            .iter()
            .position(|route| *route == source)
            && let Some(session) = self.sessions.get(index)
        {
            session.begin_voice_clone();
        } else {
            self.last_error =
                Some("Start translation on this audio source before cloning its voice.".into());
        }
    }

    fn voice_clone_state(
        &self,
        source: CaptureSource,
    ) -> Option<&xrtranslate_protocol::VoiceCloneState> {
        match source {
            CaptureSource::Microphone => self.microphone_clone_state.as_ref(),
            CaptureSource::SystemAudio => self.loopback_clone_state.as_ref(),
            CaptureSource::Both => None,
        }
    }

    /// Controls only whether OSC presentation includes the infrastructure-provided ID.
    fn set_osc_speaker_number_visible(&mut self, enabled: bool) {
        let enabled = enabled
            && crate::feature_access::is_available(crate::feature_access::Feature::SpeakerNumbers);
        self.osc_plugin.draft_mut().show_speaker_number = enabled;
        match self.osc_plugin.apply_draft() {
            Ok(()) => self.last_error = None,
            Err(error) => self.last_error = Some(error),
        }
        self.save_settings();
    }

    fn set_mute_self_pauses_translation(&mut self, enabled: bool) {
        let enabled = enabled
            && crate::feature_access::is_available(crate::feature_access::Feature::MuteSync);
        self.mute_self_pauses_translation
            .store(enabled, Ordering::Release);
        self.save_settings();
    }

    fn set_floating_subtitles_enabled(&mut self, enabled: bool) {
        self.floating_subtitles_enabled = enabled
            && crate::feature_access::is_available(
                crate::feature_access::Feature::FloatingSubtitles,
            );
        self.save_settings();
    }

    fn recognition_settings(&self, source: CaptureSource) -> &RecognitionSettings {
        match source {
            CaptureSource::Microphone => &self.microphone_recognition,
            CaptureSource::SystemAudio => &self.loopback_recognition,
            CaptureSource::Both => unreachable!("Both has one profile per route"),
        }
    }

    fn recognition_settings_mut(&mut self, source: CaptureSource) -> &mut RecognitionSettings {
        match source {
            CaptureSource::Microphone => &mut self.microphone_recognition,
            CaptureSource::SystemAudio => &mut self.loopback_recognition,
            CaptureSource::Both => unreachable!("Both has one profile per route"),
        }
    }

    fn set_audio_adaptation(&mut self, source: CaptureSource) {
        let recognition = self.recognition_settings_mut(source);
        recognition.background_noise = recognition.background_noise.clamp(0.2, 0.8);
        recognition.pause_tolerance = recognition.pause_tolerance.clamp(0.0, 1.0);
        self.save_settings();
        let recognition = self.recognition_settings(source).clone();
        let session_index = self
            .capture_source
            .routes()
            .iter()
            .position(|route| *route == source);
        if let Some(session) = session_index.and_then(|index| self.sessions.get(index)) {
            session.update_audio_segmentation(
                vad_threshold_for_background_noise(recognition.background_noise),
                pause_tolerance_to_ms(recognition.pause_tolerance),
                recognition.continuous_recognition,
                self.source_lang.clone(),
                self.target_lang.clone(),
            );
        }
    }

    pub(crate) fn clear_history(&mut self) {
        self.translations.clear();
        self.recognition_history.clear();
        self.partial_text.clear();
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.translations.clear();
            state.recognition_history.clear();
            state.partial_text.clear();
            state.pending_final_asr.clear();
            state.pending_recognition_windows.clear();
        }
        self.osc_plugin.clear_chatbox();
    }

    pub(crate) fn translate_text(
        &mut self,
        text: &str,
        source_lang: Option<String>,
        target_lang: Option<String>,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.is_translating && !self.sessions.is_empty() {
            self.sessions[0].translate_text(trimmed, source_lang, target_lang);
        } else {
            self.last_error = Some(
                i18n::tr(
                    self.ui_language,
                    "Translation session is not active. Please start translation first.",
                )
                .to_string(),
            );
        }
    }

    pub(crate) fn pause_active_meeting(&mut self) {
        for session in &self.sessions {
            session.pause();
        }
        self.audio_system.stop();
        self.reset_audio_levels();
        if let Some(recording) = &self.meeting_plugin.meeting_recording
            && let Err(error) = recording.checkpoint()
        {
            self.meeting_plugin
                .set_error(format!("Could not checkpoint meeting audio: {error}"));
        }
        let _ = self.meeting_plugin.controller.pause_capture();
    }

    pub(crate) fn resume_active_meeting(&mut self) -> bool {
        if self.sessions.is_empty() || self.audio_txs.is_empty() {
            return false;
        }
        let routes = self.capture_source.routes();
        let audio_txs = self.audio_txs.clone();
        match self.start_selected_capture(routes, &audio_txs) {
            Ok(()) => {
                for session in &self.sessions {
                    session.resume();
                }
                if self.meeting_plugin.controller.active_meeting_id().is_some() {
                    match self.meeting_plugin.controller.resume_active_meeting() {
                        Ok(_) => {}
                        Err(error) => self.meeting_plugin.set_error(error.to_string()),
                    }
                }
                true
            }
            Err(error) => {
                self.meeting_plugin
                    .set_error(format!("Could not resume meeting audio: {error}"));
                false
            }
        }
    }

    pub(crate) fn export_open_meeting_markdown(&mut self) {
        let Some(bundle) = self.meeting_plugin.controller.bundle.as_ref() else {
            return;
        };
        let default_name = format!("{}.md", sanitize_export_name(&bundle.meeting.name));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let markdown = plugins::meeting::store::render_markdown(bundle);
        if let Err(error) = std::fs::write(path, markdown) {
            self.meeting_plugin
                .set_error(format!("Could not export meeting: {error}"));
        }
    }

    fn stop(&mut self) {
        self.audio_system.stop();
        self.audio_txs.clear();
        for worker in self.meeting_audio_routers.drain(..) {
            let _ = worker.join();
        }
        self.meeting_plugin.clear_audio_import();
        self.meeting_plugin.clear_pending_audio_import();
        self.host_audio_import = None;
        self.pending_audio_import = None;
        self.backend_start_deadline = None;
        // User-initiated stop cancels queued inference. Natural finite-input
        // EOF still uses the ordered drain path in the network session.
        for session in &self.sessions {
            session.cancel();
        }
        self.sessions.clear();
        if let Some(recording) = self.meeting_plugin.meeting_recording.take()
            && let Err(error) = recording.finalize()
        {
            self.meeting_plugin
                .set_error(format!("Could not finalize meeting recording: {error}"));
        }
        self.input_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.loopback_level
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.osc_plugin.clear_chatbox();
        self.session_owner = TranslationSessionOwner::None;
        self.is_translating = false;
        self.connection_status = "Stopped".into();

        if let Ok(mut state) = self.shared_session_state.lock() {
            state.connection_status = "Stopped".into();
            state.is_translating = false;
        }
        self.reset_audio_levels();
    }

    fn poll_session_events(&mut self) {
        if let Some(error) = self.osc_plugin.manager().take_error() {
            self.last_error = Some(error);
        }

        // Sync atomic settings to background pump thread
        self.overlay_enabled_atomic
            .store(self.floating_subtitles_enabled, Ordering::Relaxed);
        self.overlay_max_count_atomic
            .store(self.floating_subtitles_max_count, Ordering::Relaxed);
        self.overlay_font_size_atomic
            .store(self.floating_subtitles_font_size as u32, Ordering::Relaxed);

        if self.floating_subtitles_enabled {
            if let Ok(mut mgr) = self.overlay_manager.lock() {
                mgr.start();
                for event in mgr.poll_events() {
                    match event {
                        overlay_ipc::OverlayEvent::CloseRequested => {
                            self.floating_subtitles_enabled = false;
                            self.overlay_enabled_atomic.store(false, Ordering::Relaxed);
                            mgr.stop();
                        }
                        overlay_ipc::OverlayEvent::MaxCountChanged(new_max) => {
                            let clamped = new_max.clamp(1, 10);
                            self.floating_subtitles_max_count = clamped;
                            self.overlay_max_count_atomic
                                .store(clamped, Ordering::Relaxed);
                        }
                    }
                }
            }
        } else {
            if let Ok(mut mgr) = self.overlay_manager.lock() {
                mgr.stop();
            }
        }

        // Copy latest shared state into self for local rendering when main UI is visible
        let mut open_provider_configuration = false;
        if let Ok(mut state) = self.shared_session_state.lock() {
            self.connection_status = state.connection_status.clone();
            self.partial_text = state.partial_text.clone();
            self.recognition_history = state.recognition_history.clone();
            self.translations = state.translations.clone();
            self.microphone_clone_state = state.microphone_clone_state.clone();
            self.loopback_clone_state = state.loopback_clone_state.clone();
            self.tts_runtime_backend = state.tts_runtime_backend.clone();
            self.tts_runtime_cuda_version = state.tts_runtime_cuda_version.clone();
            let prompt_trace = match self.prompt_studio.active_provider() {
                PromptProviderTarget::AsrInstruction | PromptProviderTarget::AsrContextBias => {
                    state.latest_asr_prompt_trace.clone()
                }
                PromptProviderTarget::Hunyuan | PromptProviderTarget::OpenAiCompatible => {
                    state.latest_translation_prompt_trace.clone()
                }
            };
            self.prompt_studio.set_runtime_trace(prompt_trace);
            let was_translating = self.is_translating;
            self.is_translating = state.is_translating;
            if was_translating
                && !self.is_translating
                && self
                    .session_owner
                    .is_plugin(PluginId::VIDEO_PLAYER.as_str())
            {
                self.player_plugin.pause_task();
                self.session_owner = TranslationSessionOwner::None;
                for session in &self.sessions {
                    session.finish();
                }
                self.sessions.clear();
                self.host_audio_import = None;
            }
            if let Some((source_lang, target_lang)) = state.pending_route_change.take() {
                self.source_lang = source_lang;
                self.target_lang = target_lang;
                self.save_settings();
            }
            if let Some(err) = &state.last_error {
                self.last_error = Some(err.clone());
            }
            open_provider_configuration =
                std::mem::take(&mut state.provider_configuration_required);
        }
        if open_provider_configuration {
            self.stop();
            self.first_run = true;
            self.onboarding_page = 1;
        }
    }
}

fn sanitize_export_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character => character,
        })
        .collect::<String>();
    let trimmed = sanitized.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "meeting".into()
    } else {
        trimmed.into()
    }
}

fn sanitize_graph_file_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character => character,
        })
        .collect::<String>();
    let trimmed = sanitized.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "prompt-graph".into()
    } else {
        trimmed.into()
    }
}

fn pause_tolerance_to_ms(value: f32) -> u32 {
    (240.0 + value.clamp(0.0, 1.0) * 960.0).round() as u32
}

fn vad_threshold_for_background_noise(value: f32) -> f32 {
    value.clamp(0.2, 0.8)
}

fn spawn_meeting_audio_router(
    session_tx: Sender<Vec<f32>>,
    recording: plugins::meeting::recording::RecordingSink,
    source: CaptureSource,
) -> (Sender<Vec<f32>>, std::thread::JoinHandle<()>) {
    let (capture_tx, capture_rx) = bounded::<Vec<f32>>(LIVE_AUDIO_QUEUE_CAPACITY);
    let track = match source {
        CaptureSource::Microphone => plugins::meeting::recording::RecordingTrack::Microphone,
        CaptureSource::SystemAudio => plugins::meeting::recording::RecordingTrack::SystemAudio,
        CaptureSource::Both => unreachable!("Both is expanded into concrete capture routes"),
    };
    let worker = std::thread::Builder::new()
        .name(format!("meeting-audio-router-{track:?}"))
        .spawn(move || {
            while let Ok(samples) = capture_rx.recv() {
                if let Err(error) = recording.try_append(track, samples.clone()) {
                    log::error!("Meeting recording could not keep up: {error}");
                }
                if session_tx.send(samples).is_err() {
                    break;
                }
            }
        })
        .expect("failed to start meeting audio router");
    (capture_tx, worker)
}

impl XRTranslateApp {
    fn sync_player_subtitles(&mut self) {
        if !self
            .session_owner
            .is_plugin(PluginId::VIDEO_PLAYER.as_str())
        {
            return;
        }
        let active_task_id = self.player_plugin.active_task_id();
        let Some(active_id) = active_task_id else {
            return;
        };
        let is_realtime = self
            .player_plugin
            .controller
            .store
            .get(&active_id)
            .map(|t| t.subtitle_mode == plugins::player::VideoSubtitleMode::RealtimeTranslation)
            .unwrap_or(false);
        if !is_realtime {
            return;
        }

        if let Ok(state) = self.shared_session_state.lock() {
            for entry in &state.translations {
                if entry.audio_source != CaptureSource::SystemAudio {
                    continue;
                }

                let (cue, metadata) = plugins::player::subtitles::cue_from_translation(
                    plugins::player::subtitles::TranslationCueInput {
                        turn_id: entry.turn_id.clone(),
                        segment_index: entry.segment_index,
                        stream_id: entry.stream_id,
                        start_ms: entry.source_start_ms.round() as i64,
                        end_ms: entry.source_end_ms.round() as i64,
                        speaker_id: entry.speaker_id.clone(),
                        source: entry.source.clone(),
                        translated: entry.translated.clone(),
                        timing: entry.timing,
                        boundary: entry.boundary,
                        revisable: entry.revisable,
                        finalized: !entry.live,
                    },
                );
                self.player_plugin.on_translation_cue(cue, metadata);
            }
        }
    }
}

impl eframe::App for XRTranslateApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.model_task_manager.poll();
        if let Some(path) = self.runtime_installer.poll() {
            self.backend_manager.use_installed_llama_server(&path);
        }
        let runtime_requirements = self.service_config.runtime_requirements();
        if !self.runtime_installer.is_busy()
            && !self.runtime_installer.plan_matches(runtime_requirements)
            && !matches!(
                self.runtime_installer.state(),
                runtime_install::RuntimeInstallState::Failed(_)
            )
            && let Err(error) = self
                .runtime_installer
                .prepare_for(self.project_root(), runtime_requirements)
        {
            self.last_error = Some(error);
        }
        self.app_update_manager.poll();
        self.player_plugin
            .controller
            .mpv_installer
            .set_proxy_url(Some(self.download_proxy_url.clone()));
        self.poll_audio_device_refresh();
        self.poll_audio_import();
        self.player_plugin
            .on_visibility_changed(self.navigation.page == Page::Plugin(PluginId::VIDEO_PLAYER));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if self.first_run {
            ui::render_onboarding_fullscreen(self, ui);
            self.render_modal_layer(ui.ctx());
            return;
        }

        self.show_available_update();
        self.show_ready_update();

        self.poll_backend_startup(Some(ui.ctx().clone()));
        self.poll_session_events();
        self.sync_player_subtitles();
        if self.plugin_enabled(PluginId::MEETING) {
            self.meeting_plugin.controller.poll_live_view();
        }

        let is_player_fullscreen = self.navigation.page == Page::Plugin(PluginId::VIDEO_PLAYER)
            && self.player_plugin.controller.fullscreen_mode;
        let viewport_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));

        if !is_player_fullscreen {
            let expand_target = if self.navigation.collapsed { 0.0 } else { 1.0 };
            let expand_factor = ui::animation::AnimationSystem::animate_value(
                ui.ctx(),
                egui::Id::new("sidebar_expand_anim"),
                expand_target,
                0.20,
            );
            let eased_expand = ui::animation::AnimationSystem::ease_out_cubic(expand_factor);
            let sidebar_width = egui::lerp(54.0..=200.0, eased_expand);
            let margin_x = egui::lerp(8.0..=12.0, eased_expand);

            let prev_collapsed = self.navigation.collapsed;
            let prev_page = self.navigation.page;

            // 1. Native Sidebar Panel (Animated width, full height)
            egui::Panel::left("sidebar_panel")
                .resizable(false)
                .exact_size(sidebar_width)
                .frame(
                    egui::Frame::new()
                        .fill(ui::theme::sidebar(viewport_focused))
                        .stroke(egui::Stroke::new(1.0, ui::theme::border()))
                        .inner_margin(egui::Margin::symmetric(margin_x.round() as i8, 14)),
                )
                .show(ui, |ui| {
                    ui::render_sidebar(
                        ui,
                        &mut self.navigation,
                        &self.plugin_preferences,
                        &mut self.modal_dialog,
                        &mut self.first_run,
                        &mut self.onboarding_page,
                        self.ui_language,
                        eased_expand,
                    );
                });

            if self.navigation.collapsed != prev_collapsed || self.navigation.page != prev_page {
                self.save_settings();
                self.player_plugin.on_visibility_changed(
                    self.navigation.page == Page::Plugin(PluginId::VIDEO_PLAYER),
                );
            }
        }

        // 2. Native Central Content Panel (Takes 100% of remaining width and height)
        let central_frame = if is_player_fullscreen {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(10, 15, 26))
                .inner_margin(egui::Margin::ZERO)
        } else {
            egui::Frame::new()
                .fill(ui::theme::content_backdrop(viewport_focused))
                .shadow(egui::Shadow {
                    offset: [0, 0],
                    blur: 14,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(24),
                })
                .inner_margin(egui::Margin::symmetric(24, 20))
        };

        egui::CentralPanel::default()
            .frame(central_frame)
            .show(ui, |ui| {
                let plugin_owned_scroll = match self.navigation.page {
                    Page::Plugin(id) => {
                        PluginRegistry::builtin()
                            .descriptor(id)
                            .is_some_and(|descriptor| {
                                descriptor.scroll_policy == PluginScrollPolicy::Plugin
                            })
                    }
                    _ => false,
                };
                if plugin_owned_scroll {
                    let Page::Plugin(id) = self.navigation.page else {
                        unreachable!();
                    };
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::Plugin(id),
                        |ui| self.render_plugin_page(id, ui),
                    );
                    return;
                }
                if self.navigation.page == Page::PromptStudio {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::PromptStudio,
                        |ui| {
                            let snapshot = self.prompt_studio.snapshot(&self.prompt_library);
                            let actions = ui::pages::prompt_studio::render(
                                &snapshot,
                                &mut self.prompt_studio,
                                ui,
                                self.ui_language,
                            );
                            self.apply_prompt_studio_actions(actions);
                        },
                    );
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("main_scroll_area")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.navigation.page {
                        Page::Translation => {
                            ui::animation::AnimationSystem::render_animated_page(
                                ui,
                                Page::Translation,
                                |ui| ui::pages::translation::render(self, ui),
                            );
                        }
                        Page::Plugin(id) => {
                            ui::animation::AnimationSystem::render_animated_page(
                                ui,
                                Page::Plugin(id),
                                |ui| self.render_plugin_page(id, ui),
                            );
                        }
                        Page::Settings => {
                            ui::animation::AnimationSystem::render_animated_page(
                                ui,
                                Page::Settings,
                                |ui| ui::pages::settings::render(self, ui),
                            );
                        }
                        Page::PromptStudio => unreachable!(),
                    });
            });

        self.render_modal_layer(ui.ctx());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.window_backdrop.clear_color()
    }
}

#[cfg(windows)]
fn configure_dll_search_paths() {
    use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
    use windows::core::HSTRING;
    for bin_dir in crate::plugins::player::runtime_bin_directories() {
        if bin_dir.is_dir() {
            if let Ok(abs) = bin_dir.canonicalize() {
                let _ = unsafe { SetDllDirectoryW(&HSTRING::from(abs.as_os_str())) };
            } else {
                let _ = unsafe { SetDllDirectoryW(&HSTRING::from(bin_dir.as_os_str())) };
            }
            return;
        }
    }
}

#[cfg(windows)]
fn cleanup_runtime_cache() {
    let cache_dir = std::path::Path::new("runtime/cache");
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("mpv_decode_") && name.ends_with(".wav") {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    #[cfg(windows)]
    configure_dll_search_paths();

    #[cfg(windows)]
    cleanup_runtime_cache();

    if std::env::args().any(|a| a == "--overlay") {
        #[cfg(windows)]
        overlay_native::run_native_overlay();
        return Ok(());
    }

    let window_backdrop = window_backdrop::WindowBackdrop::from_environment();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([880.0, 600.0])
            // Keep the native non-client frame stable from CreateWindowExW
            // onward. Toggling decorations after the first transparent frame
            // can leave a stale DWM frame behind on Windows.
            .with_decorations(true)
            .with_transparent(window_backdrop.uses_transparent_surface())
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../resources/branding/xrtranslate-logo.png"
                ))
                .expect("embedded application icon must be valid PNG"),
            ),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    #[cfg(windows)]
    let options = configure_transparent_wgpu(options, window_backdrop);
    eframe::run_native(
        "XRTranslate",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            ui::fonts::configure_multilingual_fonts(&cc.egui_ctx);
            ui::theme::apply_theme(&cc.egui_ctx);
            if let Some(state) = cc.wgpu_render_state.as_ref() {
                log::info!("wgpu adapter: {:?}", state.adapter.get_info());
            } else {
                log::warn!("wgpu render state is unavailable during app creation");
            }
            if let Err(error) = window_backdrop::apply(cc, window_backdrop) {
                log::warn!("Unable to configure {window_backdrop:?} window backdrop: {error}");
            }
            let mut app = XRTranslateApp::default();
            app.window_backdrop = window_backdrop;
            Ok(Box::new(app))
        }),
    )
}

#[cfg(windows)]
fn configure_transparent_wgpu(
    mut options: eframe::NativeOptions,
    backdrop: window_backdrop::WindowBackdrop,
) -> eframe::NativeOptions {
    if backdrop.uses_transparent_surface()
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
    {
        // A HWND swapchain is composited through the opaque GDI redirection
        // bitmap. DirectComposition owns the visual instead, which lets the
        // premultiplied alpha surface reach the desktop directly.
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
        setup
            .instance_descriptor
            .backend_options
            .dx12
            .presentation_system = eframe::wgpu::Dx12SwapchainKind::DxgiFromVisual;
        log::info!(
            "transparent WGPU configuration: backends={:?}, dx12_presentation=DxgiFromVisual",
            setup.instance_descriptor.backends
        );
    }
    options
}

#[cfg(test)]
mod tests {
    use super::{initialize_live_audio, vad_threshold_for_background_noise};

    #[test]
    fn noisier_environment_uses_a_stricter_vad_threshold() {
        let quiet = vad_threshold_for_background_noise(0.2);
        let medium = vad_threshold_for_background_noise(0.5);
        let noisy = vad_threshold_for_background_noise(0.8);

        assert!(quiet < medium);
        assert!(medium < noisy);
    }

    #[test]
    fn live_audio_initializes_output_dependencies_before_capture() {
        let mut events = Vec::new();

        let dependencies = initialize_live_audio(
            &mut events,
            |events| {
                events.push("output-ready");
                "session-config"
            },
            |events| {
                events.push("capture-active");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(dependencies, "session-config");
        assert_eq!(events, ["output-ready", "capture-active"]);
    }

    #[test]
    fn live_audio_propagates_capture_activation_failure() {
        let mut events = Vec::new();

        let error = initialize_live_audio(
            &mut events,
            |events| events.push("output-ready"),
            |events| {
                events.push("capture-failed");
                Err("device unavailable".into())
            },
        )
        .unwrap_err();

        assert_eq!(error, "device unavailable");
        assert_eq!(events, ["output-ready", "capture-failed"]);
    }
}
