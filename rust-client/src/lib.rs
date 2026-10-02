// Use the Windows GUI subsystem in release builds so double-clicking the
// executable never creates a transient console. Keep the console subsystem in
// debug builds for direct terminal logs and developer visibility.

use crossbeam_channel::{Receiver, Sender, bounded};
use eframe::egui;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_text_actions;
#[cfg(target_os = "android")]
mod android_translation_history;
mod app_update;
mod audio;
#[cfg(target_os = "linux")]
mod audio_linux;
mod audio_processing;
mod audio_studio;
mod backend;
mod child_process;
mod client_settings;
pub(crate) mod contributors;
mod feature_access;
mod file_dialog;
mod history;
mod i18n;
pub(crate) mod media_import;
mod model_install;
mod network;
#[cfg(any(windows, target_os = "linux"))]
mod ocr_capture;
#[cfg(any(windows, target_os = "linux"))]
mod ocr_host;
#[cfg(any(windows, target_os = "linux"))]
mod ocr_runtime;
mod onboarding;
mod overlay_ipc;
mod overlay_manager;
#[cfg(any(windows, target_os = "linux"))]
mod overlay_native;
mod plugins;
mod presentation;
mod runtime_install;
#[cfg(any(windows, target_os = "linux"))]
mod screen_capture;
mod service_config;
pub mod session_coordinator;
mod streaming;
mod text_translation;
mod translation_service;
mod ui;
pub(crate) mod usage_guidelines;
pub mod version;
mod voicemeeter;
mod window_backdrop;

use audio::{
    AudioApplication, AudioRouteConfig, AudioRouteLoopbackConfig, AudioRouteLoopbackTarget,
    AudioRouteMediaConfig, AudioRouteSourceConfig, AudioSystem, InputConfigInfo, InputDevice,
};
use audio_studio::{
    ApplicationSelection, AudioDeviceRole, AudioGraph, AudioNodeKind, AudioStudioController,
    AudioStudioHostAction, AudioStudioHostEvent, AudioStudioUiAction, HostAudioApplication,
    HostAudioCapabilities, HostAudioDevice, HostAudioSnapshot, SystemAudioCapture,
    VoiceMeeterBus as StudioVoiceMeeterBus, VoiceMeeterEdition as StudioVoiceMeeterEdition,
    VoiceMeeterInputSnapshot, VoiceMeeterSnapshot, VoiceMeeterStripIndex,
};
use client_settings::{CaptureSource, ClientSettings, RecognitionSettings};
use history::{
    PendingAuthoritativeRecognition, PendingAuthoritativeTranslation, PendingFinalAsr,
    PendingRecognitionWindow, RecognitionHistoryEntry, TranslationHistoryEntry,
    collect_authoritative_recognition_snapshot, collect_authoritative_translation_snapshot,
    collect_recognition_window, merge_authoritative_recognition_snapshot,
    merge_authoritative_translation_snapshot, merge_stream_recognition, merge_stream_translation,
    upsert_completed_translation,
};
use i18n::UiLanguage;
use network::{ExternalAudioGate, SessionConfig, SessionEvent, start_session};
use plugins::meeting::{
    MeetingAction, MeetingAudioSource, MeetingInputRequest, MeetingPlugin, MeetingUiSnapshot,
};
use plugins::osc::{OscPageContext, OscPlugin, OscUiAction};
use plugins::vr_overlay::{VrOverlayPageContext, VrOverlayPlugin, VrOverlayUiAction};
use plugins::{PluginId, PluginPreferences, PluginRegistry, PluginScrollPolicy};
pub(crate) use presentation::speaker::compact_speaker_label;
use session_coordinator::{
    CaptionUpdate, HostOutputEvent, HostOutputSubscriber, PluginSessionBinding,
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSessionOwner,
    TranslationSessionPlugin,
};
use ui::{NavigationState, Page};
use xrtranslate_prompt::{PromptExecutionTrace, PromptProviderTarget, PromptTemplateLibrary};
use xrtranslate_protocol::PromptGraphSet;

use session_coordinator::{TranslationInput, TranslationTask};
pub use xrtranslate_engine::language::LANGUAGE_OPTIONS;
use xrtranslate_engine::language::{LanguageCapabilities, LanguageSelection, LanguageSet};

/// Capture callbacks never block. A bounded handoff prevents an overloaded
/// network/model path from turning old audio into ever-growing live latency.
const LIVE_AUDIO_QUEUE_CAPACITY: usize = 64;

pub(crate) fn language_label(ui_language: UiLanguage, code: &str) -> &'static str {
    if code == "auto" {
        return i18n::tr(ui_language, "Auto (bidirectional)");
    }
    xrtranslate_engine::language::SupportedLanguage::parse(code)
        .map(|language| i18n::tr(ui_language, language.name()))
        .unwrap_or_else(|| i18n::tr(ui_language, "Unknown language"))
}

/// Returns true if the two language codes are mutually exclusive and should not
/// both be selectable at the same time (e.g. zh and zh-TW are the same base
/// language and would produce a no-op or circular translation route).
pub fn languages_conflict(a: &str, b: &str) -> bool {
    use xrtranslate_engine::language::SupportedLanguage;
    match (SupportedLanguage::parse(a), SupportedLanguage::parse(b)) {
        (Some(a), Some(b)) => a.base_code() == b.base_code(),
        _ => a.eq_ignore_ascii_case(b),
    }
}

fn capture_source_to_meeting(source: CaptureSource) -> MeetingAudioSource {
    match source {
        CaptureSource::Microphone => MeetingAudioSource::Microphone,
        CaptureSource::SystemAudio => MeetingAudioSource::SystemAudio,
        CaptureSource::Both => MeetingAudioSource::Both,
    }
}

fn meeting_source_name_to_capture(source: &str) -> CaptureSource {
    match source {
        "system_audio" => CaptureSource::SystemAudio,
        "both" => CaptureSource::Both,
        _ => CaptureSource::Microphone,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct AudioStudioAsrPlan {
    capture_source: CaptureSource,
    microphone_device_id: Option<String>,
    system_audio_input: Option<SystemAudioInputSelection>,
    microphone_effects: Vec<audio_processing::SourceEffect>,
    system_audio_effects: Vec<audio_processing::SourceEffect>,
}

impl AudioStudioAsrPlan {
    fn matches_current_settings(
        &self,
        capture_source: CaptureSource,
        microphone_device_id: &str,
        system_audio_input: &SystemAudioInputSelection,
    ) -> bool {
        self.capture_source == capture_source
            && self
                .microphone_device_id
                .as_ref()
                .is_none_or(|device_id| device_id == microphone_device_id)
            && self
                .system_audio_input
                .as_ref()
                .is_none_or(|input| input == system_audio_input)
    }
}

/// The one authoritative system-audio input used by the Translation pipeline.
/// Endpoint IDs and stable application identities belong here; a process ID is
/// resolved only when capture starts, because it changes whenever an app restarts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SystemAudioInputSelection {
    Endpoint { device_id: String },
    Application { application: ApplicationSelection },
}

#[derive(Clone, Debug)]
struct CompiledAudioStudioGraph {
    routes: Vec<AudioRouteConfig>,
    asr: Option<AudioStudioAsrPlan>,
}

/// Resolve a source's processing in wire order, independently of node storage
/// order. Gains after a mixer distribute over its inputs; a gate must precede
/// mixing so unrelated sources cannot open one another's gate.
fn audio_source_effects(
    graph: &audio_studio::AudioGraph,
    source: &audio_studio::NodeId,
    sink: &audio_studio::NodeId,
) -> Result<Vec<audio_processing::SourceEffect>, String> {
    use audio_processing::SourceEffect;
    use audio_studio::{AudioNodeKind, AudioProcessor, NodeId};
    fn paths(
        graph: &audio_studio::AudioGraph,
        current: &NodeId,
        sink: &NodeId,
        path: &mut Vec<NodeId>,
        found: &mut Vec<Vec<NodeId>>,
    ) {
        if path.contains(current) || found.len() > 1 {
            return;
        }
        let Some(node) = graph.node(current).filter(|node| !node.bypassed) else {
            return;
        };
        path.push(node.id.clone());
        if current == sink {
            found.push(path.clone());
        } else {
            for link in graph
                .links
                .iter()
                .filter(|link| link.enabled && link.from.node_id == *current)
            {
                paths(graph, &link.to.node_id, sink, path, found);
            }
        }
        path.pop();
    }
    let mut found = Vec::new();
    paths(graph, source, sink, &mut Vec::new(), &mut found);
    if found.len() > 1 {
        return Err(
            "A source reaches the same output more than once; keep one path per source and output"
                .into(),
        );
    }
    let mut effects = Vec::new();
    for id in found.first().into_iter().flatten() {
        match &graph.node(id).unwrap().kind {
            AudioNodeKind::Processing {
                processor: AudioProcessor::Gain { gain_db },
            } => {
                if !gain_db.is_finite() {
                    return Err("Gain must be finite".into());
                }
                effects.push(SourceEffect::Gain(
                    10.0_f32.powf(gain_db / 20.0).clamp(0.0, 8.0),
                ));
            }
            AudioNodeKind::Processing {
                processor: AudioProcessor::NoiseGate { threshold_db },
            } => {
                if !threshold_db.is_finite() || !(-80.0..=0.0).contains(threshold_db) {
                    return Err("Noise gate threshold must be between -80 and 0 dBFS".into());
                }
                let other_sources = graph
                    .nodes
                    .iter()
                    .filter(|node| node.kind.is_source() && node.id != *source)
                    .any(|node| {
                        let mut upstream_paths = Vec::new();
                        paths(graph, &node.id, id, &mut Vec::new(), &mut upstream_paths);
                        !upstream_paths.is_empty()
                    });
                if other_sources {
                    return Err(
                        "Place each noise gate before the mixer, directly after its source".into(),
                    );
                }
                effects.push(SourceEffect::NoiseGate(*threshold_db));
            }
            _ => {}
        }
    }
    Ok(effects)
}

/// Compiles plugin-owned graph semantics into the host's existing neutral
/// capabilities. Render branches become independent real-time audio routes;
/// one ASR branch becomes the ordinary translation capture lifecycle.
fn compile_audio_studio_route(
    graph: &audio_studio::AudioGraph,
) -> Result<CompiledAudioStudioGraph, String> {
    use audio_studio::{AudioNodeKind, AudioProcessor, NodeId};

    let validation = graph.validate();
    if !validation.is_valid() {
        let summary = validation
            .issues
            .iter()
            .take(3)
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("Audio graph is invalid: {summary}"));
    }

    let upstream_of = |sink: &NodeId| {
        let mut reverse = std::collections::HashMap::<NodeId, Vec<NodeId>>::new();
        for link in graph.links.iter().filter(|link| link.enabled) {
            reverse
                .entry(link.to.node_id.clone())
                .or_default()
                .push(link.from.node_id.clone());
        }
        let mut upstream = std::collections::HashSet::new();
        let mut pending = vec![sink.clone()];
        while let Some(node_id) = pending.pop() {
            if !upstream.insert(node_id.clone()) {
                continue;
            }
            if graph.node(&node_id).is_some_and(|node| node.bypassed) {
                upstream.remove(&node_id);
                continue;
            }
            if let Some(inputs) = reverse.get(&node_id) {
                pending.extend(inputs.iter().cloned());
            }
        }
        upstream
    };

    let render_sinks = graph
        .nodes
        .iter()
        .filter(|node| {
            !node.bypassed
                && matches!(
                    node.kind,
                    AudioNodeKind::MonitorOutput { .. }
                        | AudioNodeKind::GameMicrophoneOutput {
                            device_id: Some(_),
                            ..
                        }
                )
                && graph.has_enabled_source_path(&node.id)
        })
        .collect::<Vec<_>>();
    let routes =
        render_sinks
            .into_iter()
            .map(|render_sink| -> Result<AudioRouteConfig, String> {
                let upstream = upstream_of(&render_sink.id);
                let mut route = AudioRouteConfig {
                    follow_tts: matches!(render_sink.kind, AudioNodeKind::GameMicrophoneOutput { follow_tts: true, .. }),
                    output_device_id: render_sink
                        .kind
                        .selected_device()
                        .map(|device| device.0.clone())
                        .unwrap_or_default(),
                    tts_gain: None,
                    ..AudioRouteConfig::default()
                };
                for node in graph
                    .nodes
                    .iter()
                    .filter(|node| upstream.contains(&node.id) && !node.bypassed)
                {
                    match &node.kind {
                        AudioNodeKind::Microphone { device_id } => {
                            if route.microphone.is_some() {
                                return Err(
                                    "The current executor supports one microphone source".into()
                                );
                            }
                            route.microphone = Some(AudioRouteSourceConfig {
                                device_id: device_id
                                    .as_ref()
                                    .map(|device| device.0.clone())
                                    .unwrap_or_default(),
                                gain: 1.0,
                                effects: audio_source_effects(graph, &node.id, &render_sink.id)?,
                            });
                        }
                        AudioNodeKind::SystemAudio { capture } => {
                            if route.system_loopback.is_some() {
                                return Err(
                                    "The current executor supports one system-audio source".into(),
                                );
                            }
                            let target = match capture {
                                SystemAudioCapture::Endpoint { device_id, .. } => {
                                    AudioRouteLoopbackTarget::Endpoint {
                                        device_id: device_id
                                            .as_ref()
                                            .map(|device| device.0.clone())
                                            .unwrap_or_default(),
                                    }
                                }
                                SystemAudioCapture::Application {
                                    application,
                                    resolved_process_id,
                                } => AudioRouteLoopbackTarget::Application {
                                    process_id: resolved_process_id.ok_or_else(|| {
                                        "The selected application's audio session is unavailable"
                                            .to_owned()
                                    })?,
                                    application_name: application
                                        .as_ref()
                                        .map(|application| application.display_name.clone())
                                        .unwrap_or_else(|| "selected application".into()),
                                },
                            };
                            route.system_loopback = Some(AudioRouteLoopbackConfig {
                                target,
                                gain: 1.0,
                                effects: audio_source_effects(graph, &node.id, &render_sink.id)?,
                            });
                        }
                        AudioNodeKind::TextToSpeech => {
                            route.tts_gain = Some(1.0);
                            route.tts_effects =
                                audio_source_effects(graph, &node.id, &render_sink.id)?;
                        }
                        AudioNodeKind::Media {
                            source,
                            loop_playback,
                        } => {
                            route.media.push(AudioRouteMediaConfig {
                                path: source
                                    .clone()
                                    .filter(|source| !source.trim().is_empty())
                                    .ok_or("Select a media file")?,
                                loop_playback: *loop_playback,
                                effects: audio_source_effects(graph, &node.id, &render_sink.id)?,
                            });
                        }
                        AudioNodeKind::Processing {
                            processor:
                                AudioProcessor::Gain { .. } | AudioProcessor::NoiseGate { .. },
                        } => {}
                        AudioNodeKind::Processing {
                            processor:
                                AudioProcessor::Compressor { .. } | AudioProcessor::Ducker { .. },
                        } => {
                            return Err(
                                "This processor is not available in the real-time executor".into(),
                            );
                        }
                        AudioNodeKind::Processing {
                            processor: AudioProcessor::Limiter { ceiling_db },
                        } => {
                            route.output_ceiling = route
                                .output_ceiling
                                .min(10.0_f32.powf(ceiling_db / 20.0).clamp(0.01, 1.0));
                        }
                        AudioNodeKind::Mixer
                        | AudioNodeKind::AsrTap
                        | AudioNodeKind::MonitorOutput { .. }
                        | AudioNodeKind::GameMicrophoneOutput { .. } => {}
                    }
                }
                if route.microphone.is_none()
                    && route.system_loopback.is_none()
                    && route.tts_gain.is_none()
                    && route.media.is_empty()
                {
                    return Err("The selected output has no executable audio source".into());
                }
                if route.follow_tts && (route.microphone.is_none() || route.tts_gain.is_none() || route.system_loopback.is_some() || !route.media.is_empty()) {
                    return Err("Automatic translator microphone requires only microphone and TTS sources. Use the Translator microphone preset or edit the graph.".into());
                }
                Ok(route)
            })
            .collect::<Result<Vec<_>, _>>()?;

    let asr_sinks = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap))
        .filter(|node| graph.has_enabled_source_path(&node.id))
        .collect::<Vec<_>>();
    if asr_sinks.len() > 1 {
        return Err("The current translation host supports one ASR sink per graph".into());
    }
    let asr = asr_sinks
        .first()
        .map(|sink| -> Result<AudioStudioAsrPlan, String> {
            let upstream = upstream_of(&sink.id);
            let mut microphone_device_id = None;
            let mut system_audio_input = None;
            let mut microphone_effects = Vec::new();
            let mut system_audio_effects = Vec::new();
            for node in graph
                .nodes
                .iter()
                .filter(|node| upstream.contains(&node.id) && !node.bypassed)
            {
                match &node.kind {
                    AudioNodeKind::Microphone { device_id } => {
                        if microphone_device_id.is_some() {
                            return Err("The ASR branch supports one microphone source".into());
                        }
                        microphone_effects = audio_source_effects(graph, &node.id, &sink.id)?;
                        microphone_device_id = Some(
                            device_id
                                .as_ref()
                                .map(|device| device.0.clone())
                                .unwrap_or_default(),
                        );
                    }
                    AudioNodeKind::SystemAudio { capture } => {
                        if system_audio_input.is_some() {
                            return Err("The ASR branch supports one system-audio source".into());
                        }
                        system_audio_effects = audio_source_effects(graph, &node.id, &sink.id)?;
                        system_audio_input = Some(match capture {
                            SystemAudioCapture::Endpoint { device_id, .. } => {
                                SystemAudioInputSelection::Endpoint {
                                    device_id: device_id
                                        .as_ref()
                                        .map(|device| device.0.clone())
                                        .unwrap_or_default(),
                                }
                            }
                            SystemAudioCapture::Application { application, .. } => {
                                SystemAudioInputSelection::Application {
                                    application: application.clone().ok_or_else(|| {
                                        "Select an application for the ASR input".to_owned()
                                    })?,
                                }
                            }
                        });
                    }
                    AudioNodeKind::TextToSpeech => {
                        return Err("TTS cannot be connected to ASR; route it to a monitor or game-microphone output".into());
                    }
                    AudioNodeKind::Media { .. } => {
                        return Err("Direct media-file ASR is not available in Audio Studio yet".into());
                    }
                    AudioNodeKind::Processing {
                        processor: AudioProcessor::Gain { .. } | AudioProcessor::NoiseGate { .. },
                    } => {}
                    AudioNodeKind::Processing { .. } => {
                        return Err("DSP nodes on the ASR branch are not executable yet".into());
                    }
                    AudioNodeKind::Mixer
                    | AudioNodeKind::AsrTap
                    | AudioNodeKind::MonitorOutput { .. }
                    | AudioNodeKind::GameMicrophoneOutput { .. } => {}
                }
            }
            let capture_source = match (
                microphone_device_id.is_some(),
                system_audio_input.is_some(),
            ) {
                (true, true) => CaptureSource::Both,
                (true, false) => CaptureSource::Microphone,
                (false, true) => CaptureSource::SystemAudio,
                (false, false) => return Err("The ASR sink has no executable source".into()),
            };
            Ok(AudioStudioAsrPlan {
                capture_source,
                microphone_device_id,
                system_audio_input,
                microphone_effects,
                system_audio_effects,
            })
        })
        .transpose()?;

    if routes.is_empty() && asr.is_none() {
        return Err("This graph has no executable ASR, monitor, or game-microphone output".into());
    }
    Ok(CompiledAudioStudioGraph { routes, asr })
}

/// Compile only the branch that feeds recognition. A partially configured
/// render branch must not prevent Audio Studio from synchronizing ASR input.
fn compile_audio_studio_asr(
    graph: &audio_studio::AudioGraph,
) -> Result<Option<AudioStudioAsrPlan>, String> {
    use audio_studio::{AudioNodeKind, NodeId};

    let asr_sinks = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap))
        .filter(|node| graph.has_enabled_source_path(&node.id))
        .collect::<Vec<_>>();
    if asr_sinks.is_empty() {
        return Ok(None);
    }
    if asr_sinks.len() > 1 {
        return Err("The current translation host supports one ASR sink per graph".into());
    }

    let mut reverse = std::collections::HashMap::<NodeId, Vec<NodeId>>::new();
    for link in graph.links.iter().filter(|link| link.enabled) {
        reverse
            .entry(link.to.node_id.clone())
            .or_default()
            .push(link.from.node_id.clone());
    }
    let mut retained = std::collections::HashSet::new();
    let mut pending = vec![asr_sinks[0].id.clone()];
    while let Some(node_id) = pending.pop() {
        if !retained.insert(node_id.clone()) {
            continue;
        }
        if let Some(inputs) = reverse.get(&node_id) {
            pending.extend(inputs.iter().cloned());
        }
    }

    let mut asr_graph = graph.clone();
    asr_graph.nodes.retain(|node| retained.contains(&node.id));
    asr_graph.links.retain(|link| {
        link.enabled && retained.contains(&link.from.node_id) && retained.contains(&link.to.node_id)
    });
    compile_audio_studio_route(&asr_graph).map(|execution| execution.asr)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingResourceDeletion {
    Model(xrtranslate_assets::ModelAssetId),
    Runtime,
}

struct XRTranslateApp {
    pending_file_dialogs: Vec<(
        file_dialog::FileRequest,
        Box<dyn FnOnce(&mut XRTranslateApp, std::path::PathBuf)>,
    )>,
    audio_system: AudioSystem,
    devices: Vec<InputDevice>,
    device_refresh_rx: Option<Receiver<AudioDeviceSnapshot>>,
    application_refresh_rx: Option<Receiver<Result<Vec<AudioApplication>, String>>>,
    last_device_refresh_request: Option<std::time::Instant>,
    last_application_refresh_request: Option<std::time::Instant>,
    last_audio_discovery_page: Option<Page>,
    selected_device_id: String,
    loopback_devices: Vec<InputDevice>,
    audio_applications: Vec<AudioApplication>,
    selected_loopback_device_id: String,
    system_audio_input: SystemAudioInputSelection,
    tts_output_devices: Vec<InputDevice>,
    capture_source: CaptureSource,
    microphone_recognition: RecognitionSettings,
    loopback_recognition: RecognitionSettings,
    selected_input_config: Option<InputConfigInfo>,
    translation_enabled: bool,
    microphone_enabled: bool,
    system_audio_enabled: bool,
    audio_tasks: Vec<translation_service::AudioTask>,
    input_level: Arc<AtomicU32>,
    loopback_level: Arc<AtomicU32>,
    microphone_vad_active: Arc<AtomicBool>,
    loopback_vad_active: Arc<AtomicBool>,
    event_tx: Sender<translation_service::TaskEvent>,
    session_event_subscribers: Arc<Vec<Box<dyn SessionEventSubscriber>>>,
    host_output_subscribers: Arc<Vec<Box<dyn HostOutputSubscriber>>>,
    connection_status: String,
    partial_text: String,
    recognition_history: Vec<RecognitionHistoryEntry>,
    translations: Vec<TranslationHistoryEntry>,
    fullscreen_history: Option<ui::pages::translation::FullscreenHistory>,
    last_error: Option<String>,
    companion_inbox: ui::companion::Inbox,
    text_translation: text_translation::TextTranslation,
    text_composer: ui::components::text_composer::TextComposer,
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
    vr_overlay_plugin: VrOverlayPlugin,
    audio_studio: AudioStudioController,
    voicemeeter_remote: Option<voicemeeter::VoiceMeeterRemote>,
    voicemeeter_route: Option<voicemeeter::VoiceMeeterStripRouteGuard>,
    audio_studio_started_voicemeeter: bool,
    meeting_plugin: MeetingPlugin,
    player_plugin: plugins::player::VideoPlayerPlugin,
    #[cfg(any(windows, target_os = "linux"))]
    ocr: ocr_host::OcrHost,
    host_audio_import: Option<media_import::AudioImportHandle>,
    pending_translations: Vec<TranslationTask>,
    active_languages: Option<LanguageSelection>,
    plugin_preferences: PluginPreferences,
    service_config: service_config::ServiceConfigEditor,
    pub preferred_gpu: Option<String>,
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
    corpus_studio: ui::pages::corpus_studio::CorpusStudioController,
    tts_center: ui::pages::tts_center::TtsCenterController,
    pub modal_dialog: ui::modal::ModalDialog,
    pending_resource_deletion: Option<PendingResourceDeletion>,
    pub first_run: bool,
    pub model_defaults_initialized: bool,
    pub usage_guidelines_accepted: bool,
    pub onboarding_page: usize,
    pub ui_language: UiLanguage,
    pub ui_theme: ui::theme::UiTheme,
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
    output_devices: Vec<InputDevice>,
}

#[derive(Default)]
struct SharedSessionState {
    connection_status: String,
    partial_text: String,
    pending_final_asr: Vec<PendingFinalAsr>,
    pending_recognition_windows: Vec<PendingRecognitionWindow>,
    recognition_history: Vec<RecognitionHistoryEntry>,
    translations: Vec<TranslationHistoryEntry>,
    translation_previews: Vec<TranslationHistoryEntry>,
    last_error: Option<String>,
    translation_enabled: bool,
    pending_route_change: Option<(String, String)>,
    latest_asr_prompt_trace: Option<PromptExecutionTrace>,
    latest_translation_prompt_trace: Option<PromptExecutionTrace>,
    provider_configuration_required: bool,
    retired_streams: Vec<u64>,
    microphone_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    loopback_clone_state: Option<xrtranslate_protocol::VoiceCloneState>,
    tts_runtime_backend: Option<String>,
    tts_runtime_cuda_version: Option<String>,
}

impl SharedSessionState {
    fn retire_stream(&mut self, stream: u64) {
        self.retired_streams.push(stream);
        self.translation_previews
            .retain(|entry| entry.stream_id != Some(stream));
        for entry in &mut self.translations {
            if entry.stream_id == Some(stream) {
                entry.live = false;
            }
        }
        for entry in &mut self.recognition_history {
            if entry.stream_id == Some(stream) {
                entry.live = false;
            }
        }
        self.pending_final_asr
            .retain(|entry| entry.stream_id != stream);
        self.pending_recognition_windows
            .retain(|entry| entry.stream_id != stream);
    }

    fn overlay_state(
        &self,
        max_items: usize,
        font_size: u32,
        microphone_active: bool,
        system_active: bool,
    ) -> overlay_ipc::OverlayState {
        let visible_entries = self
            .translations
            .iter()
            .chain(&self.translation_previews)
            .skip(
                (self.translations.len() + self.translation_previews.len())
                    .saturating_sub(max_items),
            )
            .map(|translation| overlay_ipc::OverlayEntry {
                source: translation.source.clone(),
                translated: translation.translated.clone(),
                live: translation.live,
                vad_active: match translation.audio_source {
                    CaptureSource::Microphone => microphone_active,
                    CaptureSource::SystemAudio => system_active,
                    CaptureSource::Both => false,
                },
            })
            .collect();
        overlay_ipc::OverlayState {
            font_size,
            max_items,
            visible_entries,
            partial_text: (!self.partial_text.is_empty()).then(|| self.partial_text.clone()),
            vad_active: microphone_active || system_active,
        }
    }
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
    activate_capture: impl FnOnce(&mut Host, &mut Dependencies) -> Result<(), String>,
) -> Result<Dependencies, String> {
    let mut dependencies = initialize_dependencies(host);
    activate_capture(host, &mut dependencies)?;
    Ok(dependencies)
}

impl Default for XRTranslateApp {
    fn default() -> Self {
        let audio_system = AudioSystem::new();
        let devices = audio_system.available_devices();
        let loopback_devices = audio_system.available_loopback_devices();
        let audio_applications = audio_system.available_audio_applications();
        let tts_output_devices = audio_system.available_output_devices();
        let (event_tx, event_rx) = bounded(256);
        let backend_manager = backend::BackendManager::load();
        let service_config = service_config::ServiceConfigEditor::load();
        let mut settings = ClientSettings::load(&backend_manager.project_root());
        settings.sanitize_devices(&devices, &loopback_devices);
        let initial_system_audio_input = SystemAudioInputSelection::Endpoint {
            device_id: settings.selected_loopback_device_id.clone(),
        };
        let osc_plugin = OscPlugin::new(
            settings.osc_settings.clone(),
            settings.plugin_preferences.is_enabled(PluginId::OSC),
        );
        let vr_overlay_plugin = VrOverlayPlugin::new(
            settings.vr_overlay_settings.clone(),
            settings.plugin_preferences.is_enabled(PluginId::VR_OVERLAY),
        );
        let meeting_plugin = MeetingPlugin::open(&backend_manager.project_root());
        let audio_studio = AudioStudioController::open(&backend_manager.project_root());
        let voicemeeter_remote = match voicemeeter::VoiceMeeterRemote::discover() {
            Ok(remote) => remote,
            Err(error) => {
                log::warn!("VoiceMeeter integration is unavailable: {error}");
                None
            }
        };
        let player_plugin = plugins::player::VideoPlayerPlugin::new();
        #[cfg(any(windows, target_os = "linux"))]
        let ocr = ocr_host::OcrHost::default();
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
            microphone_clone_state: settings.microphone_clone_state.clone(),
            loopback_clone_state: settings.loopback_clone_state.clone(),
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
        let session_event_subscribers: Arc<Vec<Box<dyn SessionEventSubscriber>>> = Arc::new(vec![
            Box::new(meeting_plugin.event_sink.clone()),
            Box::new(player_plugin.event_sink.clone()),
            #[cfg(any(windows, target_os = "linux"))]
            Box::new(ocr.plugin.event_sink.clone()),
        ]);
        let host_output_subscribers: Arc<Vec<Box<dyn HostOutputSubscriber>>> = Arc::new(vec![
            Box::new(osc_plugin.publisher()),
            Box::new(vr_overlay_plugin.handle()),
            #[cfg(target_os = "android")]
            Box::new(android_translation_history::ResultSubscriber::default()),
        ]);
        let result_subscribers = Arc::clone(&session_event_subscribers);
        let output_subscribers = Arc::clone(&host_output_subscribers);

        std::thread::Builder::new()
            .name("session-event-pump".into())
            .spawn(move || {
                let mut pending_authoritative_sources =
                    Vec::<PendingAuthoritativeRecognition>::new();
                let mut pending_authoritative_translations =
                    Vec::<PendingAuthoritativeTranslation>::new();
                while let Ok(translation_service::TaskEvent { scope, event }) = rx.recv() {
                    // Cancellation and dispatch share this lock. Events already
                    // queued before Stop cannot revive a task or its outputs.
                    let Ok(mut state) = shared_state_clone.lock() else {
                        continue;
                    };
                    for retired in state.retired_streams.drain(..) {
                        pending_authoritative_sources
                            .retain(|snapshot| snapshot.stream_id != retired);
                        pending_authoritative_translations
                            .retain(|snapshot| snapshot.stream_id != retired);
                    }
                    if !state.translation_enabled || !scope.accepts_events() {
                        continue;
                    }
                    let scoped_stream = scope.stream_id.load(Ordering::Acquire);
                    scope.publish(&event, &result_subscribers);
                    match event {
                        SessionEvent::Connected => {
                            state.connection_status = "Connected - listening".into();
                            scope.ready.store(true, Ordering::Release);
                            state.tts_runtime_backend = None;
                            state.tts_runtime_cuda_version = None;
                        }
                        SessionEvent::Disconnected(_) => {
                            state
                                .translation_previews
                                .retain(|entry| entry.stream_id != Some(scoped_stream));
                            for entry in &mut state.translations {
                                if entry.stream_id == Some(scoped_stream) {
                                    entry.live = false;
                                }
                            }
                            for entry in &mut state.recognition_history {
                                if entry.stream_id == Some(scoped_stream) {
                                    entry.live = false;
                                }
                            }
                            state
                                .pending_recognition_windows
                                .retain(|entry| entry.stream_id != scoped_stream);
                            pending_authoritative_sources
                                .retain(|entry| entry.stream_id != scoped_stream);
                            pending_authoritative_translations
                                .retain(|entry| entry.stream_id != scoped_stream);
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
                            audio_source,
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
                                    &output_subscribers,
                                    HostOutputEvent::Caption {
                                        stream_id,
                                        audio_source,
                                        is_typing: false,
                                        source: &text,
                                        translated: "",
                                        speaker: "",
                                        update: CaptionUpdate::Replace,
                                    },
                                );
                            }
                            if kind == "final" && !text.is_empty() {
                                state.pending_final_asr.push(PendingFinalAsr {
                                    stream_id,
                                    text: text.clone(),
                                    turn_id: turn_id.clone(),
                                });
                                if state.pending_final_asr.len() > 100 {
                                    state.pending_final_asr.remove(0);
                                }
                                let is_duplicate =
                                    state.recognition_history.last().is_some_and(|entry| {
                                        entry.stream_id == Some(stream_id)
                                            && entry.text == text
                                            && entry.turn_id == turn_id
                                            && entry.speaker_id.is_empty()
                                    });
                                if !is_duplicate {
                                    state.recognition_history.push(RecognitionHistoryEntry {
                                        stream_id: Some(stream_id),
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
                                        authoritative_snapshot: false,
                                        revision_id: 0,
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
                            authoritative_snapshot,
                            revision,
                        } => {
                            if text.is_empty() {
                                continue;
                            }
                            if !publish_to_host_outputs {
                                continue;
                            }
                            state.latest_asr_prompt_trace = prompt_trace;
                            state.translation_previews.retain(|entry| {
                                entry.stream_id != Some(stream_id)
                                    || entry.turn_id != turn_id
                                    || entry.revision_id > revision
                                    || (entry.segment_index <= segment_count
                                        && (entry.segment_index != segment_index
                                            || entry.source == text))
                            });
                            if segment_index == 1 {
                                let pending_index = state
                                    .pending_final_asr
                                    .iter()
                                    .position(|pending| {
                                        pending.stream_id == stream_id
                                            && ((!turn_id.is_empty() && pending.turn_id == turn_id)
                                                || (turn_id.is_empty()
                                                    && pending.turn_id.is_empty()))
                                    })
                                    .or_else(|| {
                                        state.pending_final_asr.iter().position(|pending| {
                                            pending.stream_id == stream_id
                                                && pending.turn_id.is_empty()
                                        })
                                    });
                                if let Some(pending_index) = pending_index {
                                    let pending = state.pending_final_asr.remove(pending_index);
                                    let temporary_index =
                                        state.recognition_history.iter().rposition(|entry| {
                                            entry.stream_id == Some(stream_id)
                                                && entry.speaker_id.is_empty()
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
                                stream_id: Some(stream_id),
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
                                authoritative_snapshot,
                                revision_id: revision,
                                revision: None,
                            };
                            if authoritative_snapshot {
                                let complete = collect_authoritative_recognition_snapshot(
                                    &mut pending_authoritative_sources,
                                    stream_id,
                                    revision,
                                    segment_index,
                                    segment_count,
                                    entry,
                                );
                                if let Some(entries) = complete {
                                    let snapshot_turn = entries[0].turn_id.clone();
                                    let sources: Vec<_> =
                                        entries.iter().map(|entry| entry.text.clone()).collect();
                                    if merge_authoritative_recognition_snapshot(
                                        &mut state.recognition_history,
                                        stream_id,
                                        entries,
                                    ) {
                                        let still_matches = |entry: &TranslationHistoryEntry| {
                                            entry.stream_id != Some(stream_id)
                                                || entry.turn_id != snapshot_turn
                                                || entry.revision_id > revision
                                                || entry.segment_index.checked_sub(1).is_some_and(
                                                    |index| {
                                                        sources.get(index as usize)
                                                            == Some(&entry.source)
                                                    },
                                                )
                                        };
                                        state.translations.retain(still_matches);
                                        state.translation_previews.retain(still_matches);
                                    }
                                }
                            } else {
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
                                }
                            }
                            if state.recognition_history.len() > 100 {
                                state.recognition_history.remove(0);
                            }
                        }
                        SessionEvent::TranslationPreview {
                            stream_id,
                            audio_source,
                            publish_to_host_outputs,
                            preview,
                        } => {
                            if !publish_to_host_outputs {
                                continue;
                            }
                            let matches = |entry: &TranslationHistoryEntry| {
                                entry.stream_id == Some(stream_id)
                                    && entry.turn_id == preview.turn_id
                                    && entry.segment_index == preview.segment_index
                            };
                            if state
                                .translations
                                .iter()
                                .chain(&state.translation_previews)
                                .any(|entry| matches(entry) && entry.revision_id > preview.revision)
                            {
                                continue;
                            }
                            if preview.translated_text.is_empty() {
                                state.translation_previews.retain(|entry| !matches(entry));
                            } else {
                                if state.translation_previews.len() >= 32
                                    && !state.translation_previews.iter().any(matches)
                                {
                                    state.translation_previews.remove(0);
                                }
                                upsert_completed_translation(
                                    &mut state.translation_previews,
                                    TranslationHistoryEntry::preview(
                                        stream_id,
                                        audio_source,
                                        preview,
                                    ),
                                );
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
                            segment_count,
                            speaker_id,
                            source_start_ms,
                            source_end_ms,
                            timing,
                            boundary,
                            term_matches,
                            prompt_trace,
                            revisable,
                            overlap_ratio,
                            authoritative_snapshot,
                            revision,
                        } => {
                            if !publish_to_host_outputs {
                                continue;
                            }
                            state.latest_translation_prompt_trace = prompt_trace;
                            state.translation_previews.retain(|entry| {
                                !(entry.stream_id == Some(stream_id)
                                    && entry.turn_id == turn_id
                                    && entry.segment_index == segment_index
                                    && entry.revision_id <= revision)
                            });
                            let fragment = TranslationHistoryEntry {
                                turn_id: turn_id.clone(),
                                segment_index,
                                stream_id: Some(stream_id),
                                audio_source,
                                live: continuous && (revisable || authoritative_snapshot),
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
                                authoritative_snapshot,
                                revision_id: revision,
                                source_revision: None,
                                translated_revision: None,
                            };
                            if authoritative_snapshot {
                                let complete = collect_authoritative_translation_snapshot(
                                    &mut pending_authoritative_translations,
                                    stream_id,
                                    revision,
                                    segment_index,
                                    segment_count,
                                    fragment,
                                );
                                if let Some(entries) = complete {
                                    let merged = merge_authoritative_translation_snapshot(
                                        &mut state.translations,
                                        stream_id,
                                        entries,
                                    );
                                    if merged.accepted {
                                        for entry in &merged.stabilized {
                                            publish_host_output(
                                                &output_subscribers,
                                                HostOutputEvent::Caption {
                                                    stream_id,
                                                    audio_source,
                                                    is_typing: scope.is_text,
                                                    source: &entry.source,
                                                    translated: &entry.translated,
                                                    speaker: &entry.speaker_id,
                                                    update: CaptionUpdate::RollOver,
                                                },
                                            );
                                        }
                                        if merged.changed
                                            && let Some(entry) = merged.live.as_ref()
                                        {
                                            publish_host_output(
                                                &output_subscribers,
                                                HostOutputEvent::Caption {
                                                    stream_id,
                                                    audio_source,
                                                    is_typing: scope.is_text,
                                                    source: &entry.source,
                                                    translated: &entry.translated,
                                                    speaker: &entry.speaker_id,
                                                    update: CaptionUpdate::Replace,
                                                },
                                            );
                                        }
                                    }
                                }
                            } else if continuous && revisable {
                                let merged = merge_stream_translation(
                                    &mut state.translations,
                                    stream_id,
                                    fragment,
                                );
                                if merged.rolled_over {
                                    if let Some(previous) =
                                        state.translations.iter().rev().find(|entry| {
                                            entry.stream_id == Some(stream_id) && !entry.live
                                        })
                                    {
                                        publish_host_output(
                                            &output_subscribers,
                                            HostOutputEvent::Caption {
                                                stream_id,
                                                audio_source,
                                                is_typing: scope.is_text,
                                                source: &previous.source,
                                                translated: &previous.translated,
                                                speaker: &previous.speaker_id,
                                                update: CaptionUpdate::RollOver,
                                            },
                                        );
                                    }
                                    publish_host_output(
                                        &output_subscribers,
                                        HostOutputEvent::Caption {
                                            stream_id,
                                            audio_source,
                                            is_typing: scope.is_text,
                                            source: &merged.entry.source,
                                            translated: &merged.entry.translated,
                                            speaker: &merged.entry.speaker_id,
                                            update: CaptionUpdate::Replace,
                                        },
                                    );
                                } else if merged.changed {
                                    publish_host_output(
                                        &output_subscribers,
                                        HostOutputEvent::Caption {
                                            stream_id,
                                            audio_source,
                                            is_typing: scope.is_text,
                                            source: &merged.entry.source,
                                            translated: &merged.entry.translated,
                                            speaker: &merged.entry.speaker_id,
                                            update: CaptionUpdate::Replace,
                                        },
                                    );
                                }
                            } else {
                                publish_host_output(
                                    &output_subscribers,
                                    HostOutputEvent::Caption {
                                        stream_id,
                                        audio_source,
                                        is_typing: scope.is_text,
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
                            state
                                .translation_previews
                                .retain(|entry| entry.stream_id != Some(stream_id));
                            for entry in &mut state.translations {
                                if entry.stream_id == Some(stream_id) {
                                    entry.live = false;
                                }
                            }
                            for entry in &mut state.recognition_history {
                                if entry.stream_id == Some(stream_id) {
                                    entry.live = false;
                                }
                            }
                            state
                                .pending_recognition_windows
                                .retain(|window| window.stream_id != stream_id);
                            pending_authoritative_sources
                                .retain(|snapshot| snapshot.stream_id != stream_id);
                            pending_authoritative_translations
                                .retain(|snapshot| snapshot.stream_id != stream_id);
                            publish_host_output(
                                &output_subscribers,
                                HostOutputEvent::StreamEnded(stream_id),
                            );
                        }
                        SessionEvent::RouteChanged {
                            source_lang,
                            target_lang,
                        } => {
                            if scope.owner.is_host() {
                                state.pending_route_change = Some((source_lang, target_lang));
                            }
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
                            CaptureSource::SystemAudio | CaptureSource::Both => {}
                        },
                        SessionEvent::BackendError {
                            message,
                            configuration_required,
                        } => {
                            state
                                .translation_previews
                                .retain(|entry| entry.stream_id != Some(scoped_stream));
                            state.last_error = Some(message);
                            state.provider_configuration_required |= configuration_required;
                        }
                        SessionEvent::Error(error) => {
                            state
                                .translation_previews
                                .retain(|entry| entry.stream_id != Some(scoped_stream));
                            state.last_error = Some(error);
                            state.connection_status = "Connection error".into();
                        }
                    }

                    // Send state to overlay process immediately (unblocked by main window minimization)
                    if overlay_enabled_clone.load(Ordering::Relaxed) {
                        let max_items = overlay_max_count_clone.load(Ordering::Relaxed);
                        let font_size = overlay_font_size_clone.load(Ordering::Relaxed);
                        let overlay_state = state.overlay_state(
                            max_items,
                            font_size,
                            microphone_vad_active_clone.load(Ordering::Relaxed),
                            loopback_vad_active_clone.load(Ordering::Relaxed),
                        );

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
            pending_file_dialogs: Vec::new(),
            audio_system,
            devices,
            device_refresh_rx: None,
            application_refresh_rx: None,
            last_device_refresh_request: None,
            last_application_refresh_request: None,
            last_audio_discovery_page: None,
            selected_device_id: settings.selected_device_id,
            loopback_devices,
            audio_applications,
            selected_loopback_device_id: settings.selected_loopback_device_id,
            system_audio_input: initial_system_audio_input,
            tts_output_devices,
            capture_source: settings.capture_source,
            microphone_recognition: settings.microphone_recognition,
            loopback_recognition: settings.loopback_recognition,
            selected_input_config,
            translation_enabled: false,
            microphone_enabled: false,
            system_audio_enabled: false,
            audio_tasks: Vec::new(),
            input_level,
            loopback_level,
            microphone_vad_active,
            loopback_vad_active,
            event_tx,
            session_event_subscribers,
            host_output_subscribers,
            connection_status: "Ready".into(),
            partial_text: String::new(),
            recognition_history: Vec::new(),
            translations: Vec::new(),
            fullscreen_history: None,
            last_error: None,
            companion_inbox: ui::companion::Inbox::default(),
            text_translation: text_translation::TextTranslation::default(),
            text_composer: ui::components::text_composer::TextComposer::default(),
            server_url: settings.server_url,
            download_proxy_url: settings.download_proxy_url,
            update_channel: settings.update_channel,
            source_lang: settings.source_lang,
            target_lang: settings.target_lang,
            denoise_enabled: settings.denoise_enabled,
            tts_enabled: settings.tts_enabled && service_config.tts_is_configured(),
            microphone_clone_state: settings.microphone_clone_state,
            loopback_clone_state: settings.loopback_clone_state,
            tts_runtime_backend: None,
            tts_runtime_cuda_version: None,
            osc_plugin,
            vr_overlay_plugin,
            audio_studio,
            voicemeeter_remote,
            voicemeeter_route: None,
            audio_studio_started_voicemeeter: false,
            meeting_plugin,
            player_plugin,
            #[cfg(any(windows, target_os = "linux"))]
            ocr,
            host_audio_import: None,
            pending_translations: Vec::new(),
            active_languages: None,
            plugin_preferences: settings.plugin_preferences,
            preferred_gpu: settings
                .preferred_gpu
                .clone()
                .or_else(|| service_config.preferred_gpu()),
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
            corpus_studio: ui::pages::corpus_studio::CorpusStudioController::default(),
            tts_center: Default::default(),
            modal_dialog: ui::modal::ModalDialog::default(),
            pending_resource_deletion: None,
            first_run,
            model_defaults_initialized: settings.model_defaults_initialized || !settings.first_run,
            usage_guidelines_accepted: settings.usage_guidelines_accepted,
            onboarding_page,
            ui_language: settings.ui_language,
            ui_theme: settings.ui_theme,
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
        let _ = app.sync_translation_input_to_audio_studio();
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
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.stop_ocr();
            if self.ocr.is_stopping() {
                self.ocr.resource_deletion = Some(resource);
                return;
            }
        }
        self.delete_resource(resource);
    }

    fn delete_resource(&mut self, resource: PendingResourceDeletion) {
        let project_root = self.project_root();
        self.cancel_text_tasks(None);
        self.backend_manager.invalidate_runtime();
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
        if !self.modal_dialog.open
            && let Some(error) = self.last_error.as_deref()
            && ui::components::error_dialog(
                ctx,
                egui::Id::new("app_error"),
                self.ui_language,
                error,
            )
        {
            self.last_error = None;
        }
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

    pub(crate) fn plugin_available(&self, id: PluginId) -> bool {
        id.is_supported() && (id != PluginId::OCR || self.service_config.ocr_is_configured())
    }

    pub(crate) fn plugin_enabled(&self, id: PluginId) -> bool {
        // Embedded OCR is configured with its model, without a second switch.
        self.plugin_available(id)
            && (id == PluginId::OCR
                || PluginRegistry::builtin().is_enabled(&self.plugin_preferences, id))
    }

    /// Selects the first plugin currently requesting the exclusive translation
    /// capability. Concrete plugins implement the same neutral contract; the
    /// session infrastructure consumes only the returned binding.
    fn session_config(
        &mut self,
        languages: LanguageSelection,
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
            Some(
                self.audio_system
                    .tts_handle(self.service_config.tts_sample_rate()),
            )
        } else {
            None
        };

        SessionConfig {
            server_url: self.server_url.clone(),
            languages,
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
            prompt_graphs: Some(PromptGraphSet {
                graph: self.prompt_library.active_graph(),
            }),
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
        let graphs = PromptGraphSet {
            graph: self.prompt_library.active_graph(),
        };
        for session in self.host_channels().map(|channel| &channel.session) {
            session.update_prompt_templates(graphs.clone());
        }
        self.text_translation.update_prompts(graphs);
        self.save_settings();
    }

    fn apply_prompt_studio_actions(
        &mut self,
        actions: Vec<ui::pages::prompt_studio::PromptStudioAction>,
    ) {
        use ui::pages::prompt_studio::PromptStudioAction;

        for action in actions {
            // Recheck at the mutation boundary, including shortcuts and queued UI actions.
            if !matches!(action, PromptStudioAction::SelectStyle(_))
                && !ui::pages::prompt_studio::editor_enabled(
                    self.update_channel == client_settings::UpdateChannel::Beta,
                    &self.project_root(),
                )
            {
                continue;
            }
            match action {
                PromptStudioAction::SelectStyle(selection) => {
                    match ui::pages::prompt_studio::selected_style_profile(
                        &self.prompt_library,
                        &selection,
                        self.service_config.translation_prompt_target(),
                    ) {
                        Ok(profile) => {
                            let id = profile.id.clone();
                            self.commit_prompt_profile(profile);
                            self.activate_prompt_template(id.clone());
                            self.prompt_studio
                                .accept_style_selection(id, &self.prompt_library);
                        }
                        Err(error) => self.last_error = Some(error),
                    }
                }
                PromptStudioAction::SwitchDomain(next_domain) => {
                    self.prompt_studio.switch_domain(next_domain);
                }
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
                    if let Ok(json) = profile.export_project_json() {
                        if let Err(error) = file_dialog::FileDialog::new()
                            .add_filter("Prompt Graph (*.json)", &["json"])
                            .set_file_name(&default_name)
                            .save(json)
                        {
                            self.last_error = Some(error);
                        }
                    }
                }
                PromptStudioAction::ImportProfile => {
                    self.choose_file(
                        file_dialog::FileDialog::new()
                            .add_filter("Prompt Graph (*.json)", &["json"]),
                        |app, path| {
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
                                        if let Some(stem) =
                                            path.file_stem().and_then(|s| s.to_str())
                                        {
                                            let clean_stem = stem.trim();
                                            if !clean_stem.is_empty() {
                                                imported.name = clean_stem.to_string();
                                            }
                                        }
                                    }
                                    let id = imported.id.clone();
                                    app.commit_prompt_profile(imported);
                                    app.prompt_studio.select_profile(id, &app.prompt_library);
                                    app.save_settings();
                                }
                            }
                        },
                    );
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

    pub(crate) fn open_audio_studio(&mut self) {
        self.navigation.page = Page::AudioStudio;
        self.save_settings();
    }

    fn audio_studio_host_snapshot(&self) -> HostAudioSnapshot {
        fn add_default(devices: &mut Vec<HostAudioDevice>, role: AudioDeviceRole, name: &str) {
            devices.push(HostAudioDevice {
                id: audio_studio::DeviceId::new(""),
                name: name.into(),
                role,
                is_default: true,
                voicemeeter_strip_index: None,
            });
        }

        fn shape(edition: StudioVoiceMeeterEdition) -> (u8, u8) {
            match edition {
                StudioVoiceMeeterEdition::Standard => (2, 1),
                StudioVoiceMeeterEdition::Banana => (3, 2),
                StudioVoiceMeeterEdition::Potato => (5, 3),
            }
        }

        fn strip_for(name: &str, edition: StudioVoiceMeeterEdition) -> Option<u8> {
            let name = name.to_ascii_lowercase();
            if !name.contains("voicemeeter") {
                return None;
            }
            let (physical, virtuals) = shape(edition);
            for number in 1..=5u8 {
                if name.contains(&format!("voicemeeter in {number}")) {
                    return (number <= physical).then_some(number - 1);
                }
            }
            if name.contains("aux input") {
                return (virtuals >= 2).then_some(physical + 1);
            }
            if name.contains("vaio3 input") {
                return (virtuals >= 3).then_some(physical + 2);
            }
            name.contains("voicemeeter input").then_some(physical)
        }

        let remote_status = self
            .voicemeeter_remote
            .as_ref()
            .and_then(|remote| remote.status().ok());
        let inferred = if self.devices.iter().any(|device| {
            let name = device.name.to_ascii_lowercase();
            name.contains("out b3") || name.contains("vaio3 output")
        }) {
            StudioVoiceMeeterEdition::Potato
        } else if self.devices.iter().any(|device| {
            let name = device.name.to_ascii_lowercase();
            name.contains("out b2") || name.contains("aux output")
        }) {
            StudioVoiceMeeterEdition::Banana
        } else {
            StudioVoiceMeeterEdition::Standard
        };
        let edition = remote_status
            .as_ref()
            .and_then(|status| status.edition)
            .and_then(|edition| match edition {
                voicemeeter::VoiceMeeterEdition::Standard => {
                    Some(StudioVoiceMeeterEdition::Standard)
                }
                voicemeeter::VoiceMeeterEdition::Banana => Some(StudioVoiceMeeterEdition::Banana),
                voicemeeter::VoiceMeeterEdition::Potato => Some(StudioVoiceMeeterEdition::Potato),
                #[cfg(any(windows, test))]
                voicemeeter::VoiceMeeterEdition::Unknown(_) => None,
            })
            .unwrap_or(inferred);

        let mut devices = Vec::new();
        let is_game_microphone_render = |name: &str| {
            let name = name.to_ascii_lowercase();
            [
                "voicemeeter",
                "vb-audio",
                "cable input",
                "virtual cable",
                "virtual audio cable",
            ]
            .iter()
            .any(|marker| name.contains(marker))
        };
        if !self.devices.is_empty() {
            add_default(
                &mut devices,
                AudioDeviceRole::MicrophoneCapture,
                "Default microphone",
            );
        }
        devices.extend(self.devices.iter().map(|device| HostAudioDevice {
            id: audio_studio::DeviceId::new(device.id.clone()),
            name: device.name.clone(),
            role: AudioDeviceRole::MicrophoneCapture,
            is_default: false,
            voicemeeter_strip_index: None,
        }));
        if !self.loopback_devices.is_empty() {
            add_default(
                &mut devices,
                AudioDeviceRole::SystemAudioCapture,
                "Default system playback (loopback)",
            );
        }
        devices.extend(self.loopback_devices.iter().map(|device| HostAudioDevice {
            id: audio_studio::DeviceId::new(device.id.clone()),
            name: device.name.clone(),
            role: AudioDeviceRole::SystemAudioCapture,
            is_default: false,
            voicemeeter_strip_index: None,
        }));
        if !self.tts_output_devices.is_empty() {
            add_default(
                &mut devices,
                AudioDeviceRole::MonitorRender,
                "Default speaker",
            );
        }
        for device in &self.tts_output_devices {
            devices.push(HostAudioDevice {
                id: audio_studio::DeviceId::new(device.id.clone()),
                name: device.name.clone(),
                role: AudioDeviceRole::MonitorRender,
                is_default: false,
                voicemeeter_strip_index: None,
            });
            if is_game_microphone_render(&device.name) {
                let strip = strip_for(&device.name, edition);
                devices.push(HostAudioDevice {
                    id: audio_studio::DeviceId::new(device.id.clone()),
                    name: device.name.clone(),
                    role: AudioDeviceRole::GameMicrophoneSink,
                    is_default: false,
                    voicemeeter_strip_index: strip.map(VoiceMeeterStripIndex),
                });
            }
        }
        let game_microphone_output = devices
            .iter()
            .any(|device| device.role == AudioDeviceRole::GameMicrophoneSink);
        let voicemeeter = self.voicemeeter_remote.as_ref().map(|_| {
            let (_, bus_count) = shape(edition);
            VoiceMeeterSnapshot {
                edition,
                running: remote_status.as_ref().is_some_and(|status| status.running),
                version: remote_status
                    .as_ref()
                    .and_then(|status| status.version)
                    .map(|version| version.to_string()),
                inputs: devices
                    .iter()
                    .filter_map(|device| {
                        device
                            .voicemeeter_strip_index
                            .map(|strip_index| VoiceMeeterInputSnapshot {
                                strip_index,
                                name: device.name.clone(),
                                device_id: Some(device.id.clone()),
                            })
                    })
                    .collect(),
                buses: [
                    StudioVoiceMeeterBus::B1,
                    StudioVoiceMeeterBus::B2,
                    StudioVoiceMeeterBus::B3,
                ]
                .into_iter()
                .take(bus_count as usize)
                .collect(),
            }
        });
        HostAudioSnapshot {
            // Startup performs an initial synchronous discovery. Periodic
            // background refreshes preserve that last-good snapshot and must
            // not temporarily invalidate every graph while a scan is running.
            discovery_complete: true,
            translation_workflow_running: self.translation_enabled
                || self.backend_start_deadline.is_some(),
            translation_workflow_locked_by: None,
            capabilities: HostAudioCapabilities {
                microphone_capture: !self.devices.is_empty(),
                system_audio_capture: !self.loopback_devices.is_empty(),
                application_audio_capture: cfg!(any(windows, target_os = "linux"))
                    && !self.loopback_devices.is_empty(),
                exclude_own_process_audio: false,
                tts_feedback_suppression: !self.loopback_devices.is_empty(),
                tts_source: self.service_config.tts_is_configured(),
                media_source: true,
                monitor_output: !self.tts_output_devices.is_empty(),
                game_microphone_output,
                game_microphone_without_external_driver: false,
                multiple_render_sinks: true,
            },
            devices,
            applications: self
                .audio_applications
                .iter()
                .map(|application| HostAudioApplication {
                    id: audio_studio::ApplicationId::new(application.id.clone()),
                    display_name: application.name.clone(),
                    process_id: application.process_id,
                    active: application.active,
                })
                .collect(),
            voicemeeter,
        }
    }

    fn apply_audio_studio_ui_actions(&mut self, actions: Vec<AudioStudioUiAction>) {
        for action in actions {
            let snapshot = self.audio_studio_host_snapshot();
            match self.audio_studio.handle_ui_action(action, &snapshot) {
                Ok(host_actions) => self.apply_audio_studio_host_actions(host_actions),
                Err(error) => self.last_error = Some(error.to_string()),
            }
        }
    }

    fn reconcile_audio_studio_live_routing(&mut self) {
        self.audio_system.set_tts_enabled(self.tts_enabled);
        let _ = self
            .audio_studio
            .sync_translation_workflow_running(self.translation_enabled);
        let snapshot = self.audio_studio_host_snapshot();
        match self.audio_studio.reconcile_live_routing(&snapshot) {
            Ok(actions) => self.apply_audio_studio_host_actions(actions),
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    fn configure_voicemeeter_for_graph(&mut self, graph: &AudioGraph) -> Result<(), String> {
        let snapshot = self.audio_studio_host_snapshot();
        let requested = graph.nodes.iter().find_map(|node| match &node.kind {
            AudioNodeKind::GameMicrophoneOutput {
                device_id: Some(device_id),
                voicemeeter_bus: Some(bus),
                ..
            } => Some((device_id.clone(), *bus)),
            _ => None,
        });
        let requires_voicemeeter = requested.is_some()
            || graph.nodes.iter().any(|node| {
                node.kind.selected_device().is_some_and(|selected| {
                    snapshot
                        .devices
                        .iter()
                        .any(|device| &device.id == selected && device.requires_voicemeeter())
                })
            });
        if let Some(route) = self.voicemeeter_route.take() {
            route.clear().map_err(|error| error.to_string())?;
        }
        if !requires_voicemeeter {
            self.stop_audio_studio_managed_voicemeeter()?;
            return Ok(());
        }
        let remote = self
            .voicemeeter_remote
            .as_ref()
            .ok_or_else(|| "VoiceMeeter is not installed".to_string())?;
        let status = remote.status().map_err(|error| error.to_string())?;
        if !status.running {
            let edition = match self
                .audio_studio_host_snapshot()
                .voicemeeter
                .map(|snapshot| snapshot.edition)
                .unwrap_or(StudioVoiceMeeterEdition::Standard)
            {
                StudioVoiceMeeterEdition::Standard => voicemeeter::VoiceMeeterEdition::Standard,
                StudioVoiceMeeterEdition::Banana => voicemeeter::VoiceMeeterEdition::Banana,
                StudioVoiceMeeterEdition::Potato => voicemeeter::VoiceMeeterEdition::Potato,
            };
            remote.start(edition).map_err(|error| error.to_string())?;
            self.audio_studio_started_voicemeeter = true;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if remote.status().is_ok_and(|status| status.running) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if !remote.status().is_ok_and(|status| status.running) {
                let _ = remote.shutdown();
                self.audio_studio_started_voicemeeter = false;
                return Err("VoiceMeeter did not become ready after automatic startup".into());
            }
        }
        let Some((device_id, bus)) = requested else {
            return Ok(());
        };
        let Some(strip) = snapshot
            .devices
            .iter()
            .find(|device| device.id == device_id)
            .and_then(|device| device.voicemeeter_strip_index)
        else {
            return Ok(());
        };
        let bus = match bus {
            StudioVoiceMeeterBus::B1 => voicemeeter::VoiceMeeterBus::B1,
            StudioVoiceMeeterBus::B2 => voicemeeter::VoiceMeeterBus::B2,
            StudioVoiceMeeterBus::B3 => voicemeeter::VoiceMeeterBus::B3,
        };
        self.voicemeeter_route = Some(
            remote
                .configure(strip.0, bus, true)
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    fn stop_audio_studio_managed_voicemeeter(&mut self) -> Result<(), String> {
        if !self.audio_studio_started_voicemeeter {
            return Ok(());
        }
        if let Some(remote) = &self.voicemeeter_remote {
            remote.shutdown().map_err(|error| error.to_string())?;
        }
        self.audio_studio_started_voicemeeter = false;
        Ok(())
    }

    fn configure_translation_input_from_audio_studio(
        &mut self,
        plan: &AudioStudioAsrPlan,
    ) -> Result<(), String> {
        self.audio_system.set_capture_effects(
            plan.microphone_effects.clone(),
            plan.system_audio_effects.clone(),
        );
        if plan.matches_current_settings(
            self.capture_source,
            &self.selected_device_id,
            &self.system_audio_input,
        ) {
            return Ok(());
        }
        self.capture_source = plan.capture_source;
        if let Some(device_id) = &plan.microphone_device_id {
            self.selected_device_id.clone_from(device_id);
        }
        if let Some(input) = &plan.system_audio_input {
            if let SystemAudioInputSelection::Endpoint { device_id } = input {
                self.selected_loopback_device_id.clone_from(device_id);
            }
            self.system_audio_input = input.clone();
        } else {
            self.system_audio_input = SystemAudioInputSelection::Endpoint {
                device_id: self.selected_loopback_device_id.clone(),
            };
        }
        self.refresh_selected_input_config();
        self.save_settings();
        self.restart_host_captures(None);
        Ok(())
    }

    fn sync_translation_input_to_audio_studio(&mut self) -> Result<(), String> {
        let input_mode = match self.capture_source {
            CaptureSource::Microphone => audio_studio::graph::AsrInputMode::Microphone,
            CaptureSource::SystemAudio => audio_studio::graph::AsrInputMode::SystemAudio,
            CaptureSource::Both => audio_studio::graph::AsrInputMode::Both,
        };
        let microphone_device_id = (!self.selected_device_id.trim().is_empty())
            .then(|| audio_studio::DeviceId::new(self.selected_device_id.clone()));
        let system_capture = match &self.system_audio_input {
            SystemAudioInputSelection::Endpoint { device_id } => SystemAudioCapture::Endpoint {
                device_id: (!device_id.trim().is_empty())
                    .then(|| audio_studio::DeviceId::new(device_id.clone())),
                capture_policy: audio_studio::SystemCapturePolicy::SuppressDuringOwnTts,
            },
            SystemAudioInputSelection::Application { application } => {
                SystemAudioCapture::Application {
                    application: Some(application.clone()),
                    resolved_process_id: None,
                }
            }
        };
        self.audio_studio
            .sync_translation_input(input_mode, microphone_device_id, system_capture)
            .map_err(|error| error.to_string())?;
        self.audio_studio
            .sync_translation_workflow_running(self.translation_enabled)
            .map_err(|error| error.to_string())
    }

    fn apply_audio_studio_host_actions(&mut self, actions: Vec<AudioStudioHostAction>) {
        for action in actions {
            match action {
                AudioStudioHostAction::DiscoverApplications => {
                    self.request_audio_application_refresh();
                }
                AudioStudioHostAction::ConfigureAsrInput { graph } => {
                    let result = compile_audio_studio_asr(&graph).and_then(|asr| {
                        let plan =
                            asr.ok_or_else(|| "The selected graph has no ASR input".to_owned())?;
                        self.configure_translation_input_from_audio_studio(&plan)
                    });
                    match result {
                        Ok(()) => self.last_error = None,
                        Err(error) => self.last_error = Some(error),
                    }
                }
                AudioStudioHostAction::ChooseMedia { graph_id, node_id } => {
                    self.choose_file(
                        file_dialog::FileDialog::new()
                            .add_filter("Audio", &["wav", "flac", "mp3", "ogg", "m4a"]),
                        move |app, path| {
                            app.audio_studio.handle_host_event(
                                AudioStudioHostEvent::MediaSelected {
                                    graph_id,
                                    node_id,
                                    source: path.to_string_lossy().into_owned(),
                                },
                            );
                        },
                    );
                }

                AudioStudioHostAction::ActivateGraph { request_id, graph } => {
                    let result = self
                        .configure_voicemeeter_for_graph(&graph)
                        .and_then(|_| compile_audio_studio_route(&graph))
                        .and_then(|execution| {
                            self.audio_system
                                .replace_audio_routes(execution.routes)
                                .map(|_| ())
                                .map_err(|error| error.to_string())?;

                            Ok(())
                        });
                    match result {
                        Ok(()) => {
                            self.last_error = None;
                            self.audio_studio
                                .handle_host_event(AudioStudioHostEvent::Activated { request_id });
                        }
                        Err(message) => {
                            if let Some(route) = self.voicemeeter_route.take() {
                                let _ = route.clear();
                            }
                            let _ = self.stop_audio_studio_managed_voicemeeter();
                            self.last_error = Some(message.clone());
                            self.audio_studio.handle_host_event(
                                AudioStudioHostEvent::ActivationFailed {
                                    request_id,
                                    message,
                                },
                            );
                        }
                    }
                }
                AudioStudioHostAction::DeactivateGraph { request_id } => {
                    let stop_result = self
                        .audio_system
                        .replace_audio_routes(Vec::new())
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    if let Some(route) = self.voicemeeter_route.take() {
                        let _ = route.clear();
                    }
                    let voicemeeter_result = self.stop_audio_studio_managed_voicemeeter();
                    match stop_result.and(voicemeeter_result) {
                        Ok(()) => self.last_error = None,
                        Err(error) => self.last_error = Some(error),
                    }
                    self.audio_studio
                        .handle_host_event(AudioStudioHostEvent::Deactivated { request_id });
                }
                AudioStudioHostAction::EnqueueTts { text, .. } => {
                    if !self.tts_enabled {
                        self.set_tts_enabled(true);
                    }
                    self.translate_text(&text, None, None);
                }
                AudioStudioHostAction::SetTranslationWorkflowEnabled(enabled) => {
                    if enabled {
                        self.start(None);
                    } else {
                        self.stop();
                    }
                }
            }
        }
    }

    pub(crate) fn plugin_disable_block_reason(&self, id: PluginId) -> Option<String> {
        match id {
            PluginId::MEETING => self
                .meeting_plugin
                .disable_block_reason()
                .map(str::to_owned),
            PluginId::VIDEO_PLAYER => {
                if self.plugin_task_active(PluginId::VIDEO_PLAYER.as_str())
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
            PluginId::VR_OVERLAY => {
                self.vr_overlay_plugin.set_host_enabled(enabled);
                Ok(())
            }
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
        if id == PluginId::VR_OVERLAY {
            if plugins::vr_overlay::ui::render_settings_contribution(
                self.vr_overlay_plugin.draft_mut(),
                ui,
                self.ui_language,
            ) {
                self.vr_overlay_plugin.sync_settings();
                self.save_settings();
            }
            return;
        }
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
                } => {
                    if self.submit_text_translation(
                        &text,
                        Some(source_lang),
                        Some(target_lang),
                        None,
                    ) {
                        self.osc_plugin.draft_input_mut().clear();
                    }
                }
                OscUiAction::DirectInput(text) => {
                    if self.osc_plugin.draft().enabled && self.plugin_enabled(PluginId::OSC) {
                        self.osc_plugin.send_manual_message(&text);
                        self.osc_plugin.draft_input_mut().clear();
                    }
                }
            }
        }
    }

    fn render_osc_plugin_page(&mut self, ui: &mut egui::Ui) {
        let mute_gate_enabled = self.mute_self_pauses_translation.load(Ordering::Acquire);
        let languages = self.language_capabilities().for_text();
        let actions = self.osc_plugin.render_page(
            ui,
            OscPageContext {
                language: self.ui_language,
                preparing_text: self.text_translation.preparing_host(),
                mute_gate_enabled,
                languages,
            },
        );
        self.apply_osc_actions(actions);
    }

    fn render_text_composer(&mut self, ui: &mut egui::Ui) {
        use ui::components::text_composer::{ComposerContext, TextAction};
        let capabilities = self.language_capabilities().for_text();
        let action = self.text_composer.render(
            ui,
            "host_text_composer",
            (&mut self.source_lang, &mut self.target_lang),
            ComposerContext {
                language: self.ui_language,
                capabilities,
                preparing: self.text_translation.preparing_host(),
                allow_direct: false,
                direct_enabled: false,
                show_languages: false,
            },
        );
        if let Some(TextAction::Translate {
            text,
            source_lang,
            target_lang,
        }) = action
            && self.submit_text_translation(&text, Some(source_lang), Some(target_lang), None)
        {
            self.text_composer.text.clear();
        }
    }

    fn render_audio_studio_page(&mut self, ui: &mut egui::Ui) {
        let host_audio = self.audio_studio_host_snapshot();
        let mut snapshot = self.audio_studio.snapshot(&host_audio);
        let route_levels = self.audio_system.active_audio_route_levels();
        let routed_input = |select: fn(&audio::AudioRouteLevels) -> Option<f32>| {
            route_levels.iter().filter_map(select).reduce(f32::max)
        };
        let (capture_mic_input, capture_system_input) = self.audio_system.capture_input_levels();
        let routed = route_levels.iter().copied().fold(
            audio::AudioRouteLevels::default(),
            |mut aggregate, levels| {
                aggregate.microphone = aggregate.microphone.max(levels.microphone);
                aggregate.system_loopback = aggregate.system_loopback.max(levels.system_loopback);
                aggregate.tts = aggregate.tts.max(levels.tts);
                aggregate.media = aggregate.media.max(levels.media);
                aggregate.output = aggregate.output.max(levels.output);
                aggregate
            },
        );
        snapshot.signal_levels = audio_studio::AudioStudioSignalLevels {
            microphone: routed.microphone.max(if self.translation_enabled {
                f32::from_bits(self.input_level.load(Ordering::Relaxed))
            } else {
                0.0
            }),
            system_audio: routed.system_loopback.max(if self.translation_enabled {
                f32::from_bits(self.loopback_level.load(Ordering::Relaxed))
            } else {
                0.0
            }),
            tts: routed.tts,
            media: routed.media,
            output: routed.output,
            microphone_input: routed_input(|level| level.microphone_input)
                .into_iter()
                .chain(
                    (self.translation_enabled
                        && self
                            .capture_source
                            .routes()
                            .contains(&CaptureSource::Microphone))
                    .then_some(capture_mic_input),
                )
                .reduce(f32::max),
            system_audio_input: routed_input(|level| level.system_loopback_input)
                .into_iter()
                .chain(
                    (self.translation_enabled
                        && self
                            .capture_source
                            .routes()
                            .contains(&CaptureSource::SystemAudio))
                    .then_some(capture_system_input),
                )
                .reduce(f32::max),
            tts_input: routed_input(|level| level.tts_input),
            media_input: routed_input(|level| level.media_input),
        };
        let actions = ui::pages::audio_studio::render(&snapshot, ui, self.ui_language);
        self.apply_audio_studio_ui_actions(actions);

        if self.service_config.tts_is_configured() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let mut enabled = self.tts_enabled;
                if ui
                    .add_enabled(
                        self.service_config.tts_is_configured(),
                        egui::Checkbox::new(&mut enabled, i18n::tr(self.ui_language, "TTS")),
                    )
                    .changed()
                {
                    self.set_tts_enabled(enabled);
                }
                if snapshot.selected_graph.nodes.iter().any(|node| {
                    !node.bypassed
                        && matches!(
                            node.kind,
                            AudioNodeKind::GameMicrophoneOutput {
                                follow_tts: true,
                                ..
                            }
                        )
                }) {
                    ui.weak(i18n::tr(
                        self.ui_language,
                        if self.tts_enabled {
                            "Automatic microphone: translated speech"
                        } else {
                            "Automatic microphone: original voice"
                        },
                    ));
                }
            });
        }
    }

    fn language_capabilities(&self) -> LanguageCapabilities {
        self.service_config
            .language_capabilities()
            .unwrap_or(LanguageCapabilities {
                recognition: Some(LanguageSet::EMPTY),
                translation: Some(LanguageSet::EMPTY),
            })
    }

    fn select_languages(&mut self, source: &str, target: &str) -> Option<LanguageSelection> {
        match self
            .service_config
            .language_capabilities()
            .and_then(|caps| caps.select(source, target))
        {
            Ok(selection) => Some(selection),
            Err(error) => {
                self.last_error = Some(error.clone());
                self.meeting_plugin.set_error(error);
                None
            }
        }
    }

    fn meeting_ui_snapshot(&self) -> MeetingUiSnapshot {
        MeetingUiSnapshot {
            default_audio_source: capture_source_to_meeting(self.capture_source),
            default_source_language: self.source_lang.clone(),
            default_target_language: self.target_lang.clone(),
            languages: self.language_capabilities(),
            host_session_busy: false,
            waiting_for_microphone: self.audio_tasks.iter().any(|task| {
                task.owner.is_plugin(PluginId::MEETING.as_str())
                    && !task.paused
                    && task.is_live()
                    && task.channels.iter().all(|channel| {
                        channel.source == CaptureSource::Microphone && !channel.capturing
                    })
            }),
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
            languages: self.language_capabilities(),
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
                self.stop_plugin_task(PluginId::VIDEO_PLAYER);
            }
            plugins::player::VideoPlayerAction::StartTranslation { request, restart } => {
                use plugins::player::PlayerTranslationRequest;
                let (source, target) = match &request {
                    PlayerTranslationRequest::ImportMediaFile {
                        source_language,
                        target_language,
                        ..
                    }
                    | PlayerTranslationRequest::LiveStream {
                        source_language,
                        target_language,
                        ..
                    } => (source_language, target_language),
                };
                let Some(languages) = self.select_languages(source, target) else {
                    return;
                };
                self.stop_plugin_task(PluginId::VIDEO_PLAYER);
                if restart {
                    self.player_plugin.controller.clear_and_restart_task();
                } else {
                    self.player_plugin.controller.start_task();
                }
                let previous_recognition = self.loopback_recognition.clone();
                let input = match request {
                    PlayerTranslationRequest::ImportMediaFile {
                        path,
                        recognition,
                        audio_channels,
                        ..
                    } => TranslationInput::File {
                        path,
                        recognition,
                        options: media_import::AudioImportOptions {
                            chunk_frames: 1_600,
                            pacing: media_import::AudioImportPacing::AsFastAsPossible,
                            recognition_channels: audio_channels
                                .iter()
                                .filter(|channel| channel.recognition)
                                .map(|channel| channel.index)
                                .collect(),
                            ..media_import::AudioImportOptions::default()
                        },
                    },
                    PlayerTranslationRequest::LiveStream { recognition, .. } => {
                        self.loopback_recognition = recognition;
                        TranslationInput::Live(CaptureSource::SystemAudio)
                    }
                };
                self.start_translation_task(
                    TranslationTask {
                        languages,
                        plugin: self.player_plugin.translation_session_binding(),
                        input,
                        profiles: Vec::new(),
                    },
                    Some(ctx),
                );
                self.loopback_recognition = previous_recognition;
            }
        }
    }

    fn start_audio_file_session(&mut self, task: TranslationTask, ctx: Option<egui::Context>) {
        use translation_service::{AudioTask, ChannelScope, TaskChannel, scoped_events};
        let owner = task.owner();
        let TranslationInput::File {
            path,
            recognition,
            options,
        } = task.input
        else {
            return;
        };
        let Some(plugin) = task.plugin else { return };
        let meeting = owner.is_plugin(PluginId::MEETING.as_str());
        if meeting {
            self.meeting_plugin.event_sink.begin_sessions(1);
        }
        let (audio_tx, audio_rx) = bounded(64);
        let config = self.session_config(
            task.languages,
            Some(&plugin),
            &recognition,
            CaptureSource::SystemAudio,
            vad_threshold_for_background_noise(recognition.background_noise),
            ctx,
        );
        let scope = ChannelScope::new(owner.clone());
        let session = start_session(
            audio_rx,
            scoped_events(scope.clone(), self.event_tx.clone()),
            config,
        );
        scope
            .stream_id
            .store(session.stream_id(), Ordering::Release);
        match media_import::import_audio_file(path, audio_tx, options) {
            Ok(import) => {
                if meeting {
                    self.meeting_plugin.set_audio_import(import);
                } else {
                    self.host_audio_import = Some(import);
                }
                self.audio_tasks.push(AudioTask {
                    owner,
                    languages: task.languages,
                    paused: false,
                    routers: Vec::new(),
                    channels: vec![TaskChannel {
                        source: CaptureSource::SystemAudio,
                        scope,
                        session,
                        audio_tx: None,
                        recognition,
                        microphone_device_id: String::new(),
                        system_audio_input: self.system_audio_input.clone(),
                        capturing: false,
                    }],
                });
            }
            Err(error) => {
                scope.active.store(false, Ordering::Release);
                session.cancel();
                if meeting {
                    self.meeting_plugin.event_sink.cancel_sessions();
                }
                self.fail_task_startup(&owner, &error.to_string());
                self.last_error = Some(error.to_string());
            }
        }
    }

    fn render_plugin_page(&mut self, id: PluginId, ui: &mut egui::Ui) {
        match id {
            PluginId::MEETING => self.render_meeting_plugin_page(ui),
            PluginId::VIDEO_PLAYER => self.render_player_plugin_page(ui),
            PluginId::VR_OVERLAY => self.render_vr_overlay_plugin_page(ui),
            PluginId::OSC => self.render_osc_plugin_page(ui),
            _ => self.navigation.page = Page::Translation,
        }
    }

    fn render_vr_overlay_plugin_page(&mut self, ui: &mut egui::Ui) {
        let status = self.vr_overlay_plugin.manager().status();
        let context = VrOverlayPageContext {
            language: self.ui_language,
            status: &status,
        };
        let actions =
            plugins::vr_overlay::ui::render(self.vr_overlay_plugin.draft_mut(), ui, context);
        for action in actions {
            match action {
                VrOverlayUiAction::SettingsChanged => {
                    self.vr_overlay_plugin.sync_settings();
                    self.save_settings();
                }
                VrOverlayUiAction::ClearSubtitles => {
                    self.vr_overlay_plugin.handle().clear();
                }
                VrOverlayUiAction::ConnectSteamVr => {
                    self.vr_overlay_plugin.connect();
                }
                VrOverlayUiAction::RecenterAvatar => {
                    self.vr_overlay_plugin.manager().recenter_avatar()
                }
                VrOverlayUiAction::DisconnectSteamVr => {
                    self.vr_overlay_plugin.disconnect();
                }
            }
        }
    }

    fn apply_meeting_action(&mut self, action: MeetingAction, ctx: egui::Context) {
        match action {
            MeetingAction::None => {}
            MeetingAction::CreateAndStart(request) => {
                let Some(languages) =
                    self.select_languages(&request.source_language, &request.target_language)
                else {
                    return;
                };
                if self.plugin_task_active(PluginId::MEETING.as_str()) {
                    return;
                }
                let import_path = match &request.input {
                    MeetingInputRequest::ImportedAudio { path } => Some(path.clone()),
                    MeetingInputRequest::Live { .. } => None,
                };
                if let Some(id) = self.meeting_plugin.controller.create(&request)
                    && self.meeting_plugin.controller.begin_capture(&id)
                {
                    self.start_meeting_input(languages, import_path, Some(ctx));
                }
            }
            MeetingAction::Continue(id) => {
                if self
                    .meeting_plugin
                    .controller
                    .active_meeting_id()
                    .as_deref()
                    == Some(id.as_str())
                    && self.resume_active_meeting()
                {
                    return;
                }
                self.start_stored_meeting(&id, None, None, Some(ctx));
            }
            MeetingAction::Pause => self.pause_active_meeting(),
            MeetingAction::End => {
                self.stop_plugin_task(PluginId::MEETING);
            }
            MeetingAction::Export(meeting_id) => {
                self.meeting_plugin.controller.open_meeting(&meeting_id);
                self.export_open_meeting_markdown();
            }
            MeetingAction::Reprocess(request) => {
                self.start_stored_meeting(
                    &request.meeting_id,
                    Some(request.audio_path),
                    Some(&request.topic_title),
                    Some(ctx),
                );
            }
        }
    }

    fn start_stored_meeting(
        &mut self,
        id: &str,
        path: Option<std::path::PathBuf>,
        topic: Option<&str>,
        ctx: Option<egui::Context>,
    ) {
        let meeting = match self.meeting_plugin.controller.meeting(id) {
            Ok(meeting) => meeting,
            Err(error) => {
                self.meeting_plugin.set_error(error.to_string());
                return;
            }
        };
        let Some(languages) =
            self.select_languages(&meeting.source_language, &meeting.target_language)
        else {
            return;
        };
        self.stop_plugin_task(PluginId::MEETING);
        if !self.meeting_plugin.controller.begin_capture(id) {
            return;
        }
        if let Some(title) = topic {
            self.meeting_plugin
                .controller
                .create_capture_topic(id, title);
        }
        self.start_meeting_input(languages, path, ctx);
    }

    pub fn save_settings(&self) {
        let settings = ClientSettings {
            capture_source: self.capture_source,
            selected_device_id: self.selected_device_id.clone(),
            selected_loopback_device_id: self.selected_loopback_device_id.clone(),
            background_noise: self.microphone_recognition.background_noise,
            pause_tolerance: self.microphone_recognition.pause_tolerance,
            continuous_recognition: self.microphone_recognition.continuous_recognition,
            microphone_recognition: self.microphone_recognition.clone(),
            loopback_recognition: self.loopback_recognition.clone(),
            source_lang: self.source_lang.clone(),
            target_lang: self.target_lang.clone(),
            denoise_enabled: self.denoise_enabled,
            tts_enabled: self.tts_enabled,
            microphone_clone_state: self.microphone_clone_state.clone(),
            loopback_clone_state: self.loopback_clone_state.clone(),
            mute_self_pauses_translation: self.mute_self_pauses_translation.load(Ordering::Relaxed),
            ui_language: self.ui_language,
            ui_theme: self.ui_theme,
            first_run: self.first_run,
            model_defaults_initialized: self.model_defaults_initialized,
            server_url: self.server_url.clone(),
            download_proxy_url: self.download_proxy_url.clone(),
            update_channel: self.update_channel,
            osc_settings: self.osc_plugin.draft().clone(),
            vr_overlay_settings: self.vr_overlay_plugin.draft().clone(),
            plugin_preferences: self.plugin_preferences.clone(),
            active_page: self.navigation.page,
            sidebar_collapsed: self.navigation.collapsed,
            usage_guidelines_accepted: self.usage_guidelines_accepted,
            floating_subtitles_enabled: self.floating_subtitles_enabled,
            floating_subtitles_max_count: self.floating_subtitles_max_count,
            floating_subtitles_font_size: self.floating_subtitles_font_size,
            preferred_gpu: self.preferred_gpu.clone(),
            prompt_library: self.prompt_library.clone(),
        };
        if let Err(e) = settings.save(&self.project_root()) {
            log::error!("Failed to save client settings: {e}");
        }
    }

    pub fn set_preferred_gpu(&mut self, gpu_name: &str) {
        #[cfg(any(windows, target_os = "linux"))]
        self.stop_ocr();
        let name = gpu_name.trim();
        let value = if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
        self.preferred_gpu = value.clone();
        self.service_config.set_preferred_gpu(value.as_deref());
        let _ = self.service_config.save_onboarding_configuration();
        self.save_settings();
        let project_root = self.project_root();
        let requirements = self.service_config.runtime_requirements();
        let _ = self
            .runtime_installer
            .prepare_for(project_root, requirements);
        #[cfg(any(windows, target_os = "linux"))]
        self.refresh_ocr();
    }

    pub fn finish_onboarding(&mut self) {
        if self.service_config.has_unsaved_changes() {
            use service_config::OnboardingSaveOutcome;

            match self.service_config.save_onboarding_configuration() {
                Ok(OnboardingSaveOutcome::Saved { resolved_error }) => {
                    if resolved_error.as_ref() == self.last_error.as_ref() {
                        self.last_error = None;
                    }
                }
                Ok(OnboardingSaveOutcome::IncompleteRemoteProvider) => {
                    self.onboarding_page = 1;
                    return;
                }
                Err(error) => {
                    self.last_error = Some(error);
                    self.onboarding_page = 1;
                    return;
                }
            }
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
        if self.service_config.tts_is_configured() && !self.usage_guidelines_accepted {
            return;
        }
        self.first_run = false;
        self.save_settings();
        #[cfg(any(windows, target_os = "linux"))]
        self.refresh_ocr();
    }

    pub fn set_ui_language(&mut self, language: UiLanguage) {
        self.ui_language = language;
        self.save_settings();
    }

    pub fn set_ui_theme(&mut self, theme: ui::theme::UiTheme) {
        self.ui_theme = theme;
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
        self.first_run = true;
        let _ = self.save_settings();
        self.stop();
        self.backend_start_deadline = None;
        self.cancel_text_tasks(None);
        self.backend_manager.invalidate_runtime();
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
        let pending = std::mem::take(&mut self.pending_translations);
        for task in pending {
            self.fail_task_startup(&task.owner(), &error);
        }
        self.backend_start_deadline = None;
        self.set_connection_status(status);
        self.last_error = Some(error);
        self.release_unused_inputs();
    }

    pub fn start(&mut self, ctx: Option<egui::Context>) {
        if self.translation_enabled {
            return;
        }
        self.enable_translation_service();
        let sources = self.capture_source.routes();
        if sources.contains(&CaptureSource::Microphone) {
            self.set_microphone_enabled(true, ctx.clone());
        }
        if sources.contains(&CaptureSource::SystemAudio) && AudioSystem::supports_system_audio() {
            self.set_system_audio_enabled(true, ctx);
        }
    }

    fn enable_translation_service(&mut self) {
        if self.translation_enabled {
            return;
        }
        self.translation_enabled = true;
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.translation_enabled = true;
        }
        self.set_connection_status("Ready");
        #[cfg(any(windows, target_os = "linux"))]
        if self.ocr.enabled {
            self.refresh_ocr();
        }
    }

    fn plugin_task_active(&self, id: &str) -> bool {
        self.text_translation.owner_active(id)
            || self.audio_tasks.iter().any(|task| task.owner.is_plugin(id))
            || self.pending_translations.iter().any(|task| {
                task.plugin
                    .as_ref()
                    .is_some_and(|binding| binding.owner.plugin_id() == id)
            })
    }

    fn host_channels(&self) -> impl Iterator<Item = &translation_service::TaskChannel> {
        self.audio_tasks
            .iter()
            .filter(|task| task.owner.is_host())
            .flat_map(|task| &task.channels)
    }

    fn host_input_active(&self, source: CaptureSource) -> bool {
        self.host_channels().any(|channel| {
            channel.source == source && channel.audio_tx.is_some() && channel.scope.accepts_events()
        }) || self.pending_translations.iter().any(|task| {
            task.plugin.is_none()
                && matches!(task.input, TranslationInput::Live(input) if input.routes().contains(&source))
        })
    }

    fn live_input_requested(&self, source: CaptureSource, include_paused: bool) -> bool {
        self.audio_tasks.iter().any(|task| {
            (include_paused || !task.paused)
                && task.channels.iter().any(|channel| {
                    channel.source == source
                        && channel.audio_tx.is_some()
                        && channel.scope.accepts_events()
                })
        }) || self.pending_translations.iter().any(|task| {
            matches!(task.input, TranslationInput::Live(input) if input.routes().contains(&source))
        })
    }

    fn input_control_visible(&self, source: CaptureSource, show_selected: bool) -> bool {
        if source == CaptureSource::SystemAudio && !AudioSystem::supports_system_audio() {
            return false;
        }
        #[cfg(any(windows, target_os = "linux"))]
        let ocr_enabled = self.ocr.enabled;
        #[cfg(not(any(windows, target_os = "linux")))]
        let ocr_enabled = false;
        let has_work = ocr_enabled
            || !self.audio_tasks.is_empty()
            || !self.pending_translations.is_empty()
            || self.text_translation.busy();
        self.live_input_requested(source, true)
            || ((show_selected || !has_work) && self.capture_source.routes().contains(&source))
    }

    fn input_enabled(&self, source: CaptureSource) -> bool {
        match source {
            CaptureSource::Microphone => self.microphone_enabled,
            CaptureSource::SystemAudio => self.system_audio_enabled,
            CaptureSource::Both => unreachable!("Input controls use individual sources"),
        }
    }

    fn input_enabled_mut(&mut self, source: CaptureSource) -> &mut bool {
        match source {
            CaptureSource::Microphone => &mut self.microphone_enabled,
            CaptureSource::SystemAudio => &mut self.system_audio_enabled,
            CaptureSource::Both => unreachable!("Input controls use individual sources"),
        }
    }

    fn input_capturing(&self, source: CaptureSource) -> bool {
        self.audio_tasks
            .iter()
            .flat_map(|task| &task.channels)
            .any(|channel| channel.source == source && channel.capturing)
    }

    fn start_host_input(&mut self, source: CaptureSource, ctx: Option<egui::Context>) {
        if self.host_input_active(source) {
            return;
        }
        let Some(languages) =
            self.select_languages(&self.source_lang.clone(), &self.target_lang.clone())
        else {
            return;
        };
        self.active_languages = Some(languages);
        self.start_translation_task(
            TranslationTask {
                languages,
                plugin: None,
                input: TranslationInput::Live(source),
                profiles: Vec::new(),
            },
            ctx,
        );
    }

    fn set_microphone_enabled(&mut self, enabled: bool, ctx: Option<egui::Context>) {
        #[cfg(target_os = "android")]
        {
            if !enabled {
                android::cancel_microphone_request();
            }
            if enabled && !android::microphone_allowed() {
                if let Err(error) = android::request_microphone(true) {
                    self.last_error = Some(error);
                }
                return;
            }
        }
        self.set_microphone_input(enabled, ctx, true, Self::start_task_capture);
    }

    fn set_microphone_input(
        &mut self,
        enabled: bool,
        ctx: Option<egui::Context>,
        start_host_if_idle: bool,
        capture: impl FnMut(&mut Self, &translation_service::TaskChannel) -> Result<(), String>,
    ) {
        self.set_live_input(
            CaptureSource::Microphone,
            enabled,
            ctx,
            start_host_if_idle,
            capture,
        );
    }

    fn set_live_input(
        &mut self,
        source: CaptureSource,
        enabled: bool,
        ctx: Option<egui::Context>,
        start_host_if_idle: bool,
        mut capture: impl FnMut(&mut Self, &translation_service::TaskChannel) -> Result<(), String>,
    ) {
        if self.input_enabled(source) == enabled {
            return;
        }
        *self.input_enabled_mut(source) = enabled;
        if enabled {
            self.enable_translation_service();
        }
        let demand = self.live_input_requested(source, true);
        let active_demand = self.live_input_requested(source, false);
        let mut tasks = std::mem::take(&mut self.audio_tasks);
        for task in &mut tasks {
            if !task.is_live() {
                continue;
            }
            for channel in &mut task.channels {
                if channel.source != source {
                    continue;
                }
                if !enabled {
                    self.audio_system.stop_capture_group(channel.scope.id());
                    channel.capturing = false;
                    channel.session.pause();
                } else if !task.paused && !channel.capturing {
                    match capture(self, channel) {
                        Ok(()) => {
                            channel.capturing = true;
                            channel.session.resume();
                        }
                        Err(error) => self.last_error = Some(error),
                    }
                }
            }
        }
        self.audio_tasks = tasks;
        if enabled && !demand && start_host_if_idle {
            self.start_host_input(source, ctx);
        }
        if enabled && active_demand && !self.input_capturing(source)
            && !self.pending_translations.iter().any(|task| {
                matches!(task.input, TranslationInput::Live(input) if input.routes().contains(&source))
            })
        {
            *self.input_enabled_mut(source) = false;
        }
        if !enabled {
            self.reset_input_level(source);
        }
    }

    fn set_system_audio_enabled(&mut self, enabled: bool, ctx: Option<egui::Context>) {
        self.set_live_input(
            CaptureSource::SystemAudio,
            enabled,
            ctx,
            true,
            Self::start_task_capture,
        );
    }

    fn cancel_text_tasks(&mut self, owner: Option<&TranslationSessionOwner>) {
        if let Ok(_state) = self.shared_session_state.lock() {
            for scope in self
                .text_translation
                .scopes()
                .filter(|scope| owner.is_none_or(|owner| &scope.owner == owner))
            {
                scope.cancel(&self.session_event_subscribers);
            }
        }
        if let Some(owner) = owner {
            self.text_translation.cancel_owner(owner);
        } else {
            self.text_translation.reset();
        }
    }

    fn cancel_pending_tasks(&mut self, owner: Option<&TranslationSessionOwner>) {
        self.pending_translations.retain(|task| {
            let task_owner = task.owner();
            if owner.is_some_and(|owner| *owner != task_owner) {
                return true;
            }
            translation_service::publish_result(
                &task_owner,
                &TranslationEvent::Finished {
                    stream_id: 0,
                    outcome: TranslationOutcome::Cancelled,
                },
                &self.session_event_subscribers,
            );
            false
        });
    }

    fn stop_task_owner(&mut self, owner: &TranslationSessionOwner) {
        self.cancel_text_tasks(Some(owner));
        self.cancel_pending_tasks(Some(owner));
        let mut kept = Vec::new();
        let mut stopped = Vec::new();
        for task in self.audio_tasks.drain(..) {
            if task.owner == *owner {
                stopped.push(task);
            } else {
                kept.push(task);
            }
        }
        self.audio_tasks = kept;
        if let Ok(mut state) = self.shared_session_state.lock() {
            for task in &stopped {
                for channel in &task.channels {
                    channel.scope.cancel(&self.session_event_subscribers);
                    state.retire_stream(channel.session.stream_id());
                }
            }
        }
        for task in stopped {
            for channel in &task.channels {
                self.finish_caption_stream(channel.session.stream_id());
                self.audio_system.stop_capture_group(channel.scope.id());
            }
            task.cancel();
        }
        self.release_unused_inputs();
    }

    fn stop_plugin_task(&mut self, id: PluginId) {
        let mut owners: Vec<_> = self
            .audio_tasks
            .iter()
            .map(|task| task.owner.clone())
            .chain(self.pending_translations.iter().map(TranslationTask::owner))
            .chain(
                self.text_translation
                    .scopes()
                    .map(|scope| scope.owner.clone()),
            )
            .filter(|owner| owner.is_plugin(id.as_str()))
            .collect();
        owners.dedup();
        for owner in owners {
            self.stop_task_owner(&owner);
        }
        match id {
            PluginId::MEETING => {
                self.meeting_plugin.clear_audio_import();
                self.finalize_meeting_recording();
                self.meeting_plugin.event_sink.cancel_sessions();
                self.meeting_plugin.event_sink.finish_active();
            }
            PluginId::VIDEO_PLAYER => {
                self.player_plugin.poll_translation_events();
                self.host_audio_import = None;
                self.player_plugin.stop_import();
                self.player_plugin.pause_task();
            }
            _ => {}
        }
    }

    fn finish_caption_stream(&self, stream: u64) {
        let event = HostOutputEvent::StreamEnded(stream);
        publish_host_output(&self.host_output_subscribers, event);
    }

    fn finalize_meeting_recording(&mut self) {
        if let Some(recording) = self.meeting_plugin.meeting_recording.take()
            && let Err(error) = recording.finalize()
        {
            self.meeting_plugin
                .set_error(format!("Could not finalize meeting recording: {error}"));
        }
    }

    fn release_unused_inputs(&mut self) {
        for source in CaptureSource::Both.routes() {
            if !self.live_input_requested(*source, false) {
                *self.input_enabled_mut(*source) = false;
                self.reset_input_level(*source);
            }
        }
    }

    fn poll_translation_tasks(&mut self) {
        let mut kept = Vec::new();
        let mut ended = Vec::new();
        for task in self.audio_tasks.drain(..) {
            if task.done()
                || task
                    .channels
                    .iter()
                    .any(|channel| channel.scope.failed.load(Ordering::Acquire))
            {
                ended.push(task);
            } else {
                kept.push(task);
            }
        }
        self.audio_tasks = kept;
        for task in ended {
            if task.owner.is_plugin(PluginId::VIDEO_PLAYER.as_str()) {
                self.host_audio_import = None;
                self.player_plugin.pause_task();
            }
            if task.owner.is_plugin(PluginId::MEETING.as_str()) {
                self.meeting_plugin.clear_audio_import();
                self.finalize_meeting_recording();
            }
            if let Ok(mut state) = self.shared_session_state.lock() {
                task.invalidate();
                for channel in &task.channels {
                    state.retire_stream(channel.session.stream_id());
                }
            }
            for channel in &task.channels {
                self.audio_system.stop_capture_group(channel.scope.id());
            }
            task.cancel();
        }
        self.release_unused_inputs();
        if !self.translation_enabled {
            self.set_connection_status("Stopped");
        } else if self.backend_start_deadline.is_none()
            && !(self.audio_tasks.is_empty()
                && (self.connection_status.contains("failed")
                    || self.connection_status.contains("timed out")))
        {
            let status = if self.input_capturing(CaptureSource::Microphone)
                || self.input_capturing(CaptureSource::SystemAudio)
            {
                if self
                    .audio_tasks
                    .iter()
                    .flat_map(|task| &task.channels)
                    .any(|channel| channel.capturing && channel.scope.ready.load(Ordering::Acquire))
                {
                    "Connected - listening"
                } else {
                    "Connecting…"
                }
            } else if self.audio_tasks.iter().any(|task| !task.is_live()) {
                "Processing imported audio"
            } else if self.text_translation.busy() {
                "Translating…"
            } else if self
                .audio_tasks
                .iter()
                .any(|task| !task.paused && task.is_live())
            {
                if self.live_input_requested(CaptureSource::Microphone, false) {
                    "Waiting for microphone"
                } else {
                    "Waiting for audio"
                }
            } else {
                "Ready"
            };
            self.set_connection_status(status);
        }
    }

    fn start_translation_task(
        &mut self,
        mut task: TranslationTask,
        ctx: Option<egui::Context>,
    ) -> bool {
        let owner = task.owner();
        if matches!(task.input, TranslationInput::Text(_)) {
            self.enable_translation_service();
            return match self.text_translation.submit(task) {
                Ok(()) => true,
                Err(error) => {
                    self.fail_task_startup(&owner, &error);
                    self.last_error = Some(error);
                    false
                }
            };
        }
        if self.audio_tasks.iter().any(|active| active.owner == owner)
            || self
                .pending_translations
                .iter()
                .any(|pending| pending.owner() == owner)
        {
            return false;
        }
        if let TranslationInput::Live(source) = task.input
            && source.routes().contains(&CaptureSource::SystemAudio)
            && !AudioSystem::supports_system_audio()
        {
            let error = "System audio capture is unavailable on this platform. Choose microphone input or import an audio file.";
            self.fail_task_startup(&owner, error);
            self.last_error = Some(error.into());
            return false;
        }
        self.enable_translation_service();
        if let TranslationInput::Live(source) = task.input {
            for source in source.routes() {
                self.set_live_input(*source, true, ctx.clone(), false, Self::start_task_capture);
            }
            task.profiles = source
                .routes()
                .iter()
                .map(|source| (*source, self.recognition_settings(*source).clone()))
                .collect();
        }
        if self.backend_start_deadline.is_some() {
            self.pending_translations.push(task);
            return true;
        }
        match self.backend_manager.prepare(&self.server_url) {
            Ok(backend::BackendStart::Ready) => self.start_prepared_task(task, ctx),
            Ok(backend::BackendStart::Starting(stage)) => {
                self.pending_translations.push(task);
                self.backend_start_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(180));
                self.set_connection_status(stage.message());
            }
            Err(error) => {
                self.fail_task_startup(&owner, &error);
                self.last_error = Some(error);
                self.release_unused_inputs();
                return false;
            }
        }
        true
    }

    fn fail_task_startup(&mut self, owner: &TranslationSessionOwner, error: &str) {
        translation_service::publish_result(
            owner,
            &TranslationEvent::Finished {
                stream_id: 0,
                outcome: TranslationOutcome::Failed(error.to_owned()),
            },
            &self.session_event_subscribers,
        );
        if owner.is_plugin(PluginId::MEETING.as_str()) {
            self.meeting_plugin.fail_active_startup(error);
        }
        if owner.is_plugin(PluginId::VIDEO_PLAYER.as_str()) {
            self.player_plugin.pause_task();
            self.player_plugin.set_error(error);
        }
    }

    fn start_prepared_task(&mut self, task: TranslationTask, ctx: Option<egui::Context>) {
        if !self.translation_enabled {
            return;
        }
        let (source, target) = task.languages.wire();
        if self.select_languages(&source, &target).is_none() {
            self.fail_task_startup(&task.owner(), &self.last_error.clone().unwrap_or_default());
            return;
        }
        match task.input {
            TranslationInput::Text(_) => {
                self.start_translation_task(task, ctx);
            }
            TranslationInput::Live(_) => self.start_session(task, ctx),
            TranslationInput::File { .. } => self.start_audio_file_session(task, ctx),
        }
    }

    fn start_meeting_input(
        &mut self,
        languages: LanguageSelection,
        path: Option<std::path::PathBuf>,
        ctx: Option<eframe::egui::Context>,
    ) {
        let input = if let Some(path) = path {
            self.meeting_plugin.controller.mark_imported_audio();
            TranslationInput::File {
                path,
                recognition: self.loopback_recognition.clone(),
                options: media_import::AudioImportOptions::default(),
            }
        } else {
            TranslationInput::Live(
                self.meeting_plugin
                    .controller
                    .bundle
                    .as_ref()
                    .and_then(|bundle| bundle.meeting.input_source.as_deref())
                    .map(meeting_source_name_to_capture)
                    .unwrap_or(CaptureSource::Microphone),
            )
        };
        self.start_translation_task(
            TranslationTask {
                languages,
                plugin: self.meeting_plugin.translation_session_binding(),
                input,
                profiles: Vec::new(),
            },
            ctx,
        );
    }

    pub(crate) fn apply_service_configuration(&mut self, ctx: Option<eframe::egui::Context>) {
        if self.plugin_task_active(PluginId::MEETING.as_str())
            || self.meeting_plugin.controller.active_meeting_id().is_some()
        {
            self.meeting_plugin
                .set_error("Finish the active meeting before changing service configuration");
            self.navigation.page = Page::Plugin(PluginId::MEETING);
            return;
        }
        if self.plugin_task_active(PluginId::VIDEO_PLAYER.as_str())
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
            self.audio_system.set_tts_enabled(false);
            self.audio_system.clear_tts_playback();
        }
        let resume_translation = self.translation_enabled;
        let resume_inputs = (self.microphone_enabled, self.system_audio_enabled);
        if resume_translation {
            self.stop();
        }
        self.backend_start_deadline = None;
        self.cancel_text_tasks(None);
        self.backend_manager.shutdown();
        self.model_task_manager.invalidate_discovery();
        let requirements = self.service_config.runtime_requirements();
        let model_assets = self.service_config.selected_model_asset_ids();
        if !self.runtime_installer.is_busy()
            && !self
                .runtime_installer
                .plan_matches(requirements, &model_assets)
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
            self.enable_translation_service();
            self.set_microphone_enabled(resume_inputs.0, ctx.clone());
            self.set_system_audio_enabled(resume_inputs.1, ctx);
        } else {
            self.set_connection_status("Ready");
        }
    }

    fn start_session(&mut self, task: TranslationTask, ctx: Option<egui::Context>) {
        use translation_service::{AudioTask, ChannelScope, TaskChannel, scoped_events};
        let TranslationInput::Live(source) = task.input else {
            return;
        };
        let owner = task.owner();
        let meeting = owner.is_plugin(PluginId::MEETING.as_str());
        let recording = if meeting {
            self.start_meeting_recording()
        } else {
            None
        };
        let mut active = AudioTask {
            owner: owner.clone(),
            languages: task.languages,
            channels: Vec::new(),
            paused: false,
            routers: Vec::new(),
        };
        for source in source.routes() {
            let recognition = task
                .profiles
                .iter()
                .find(|(profile, _)| profile == source)
                .map(|(_, settings)| settings.clone())
                .unwrap_or_else(|| self.recognition_settings(*source).clone());
            let config = self.session_config(
                task.languages,
                task.plugin.as_ref(),
                &recognition,
                *source,
                vad_threshold_for_background_noise(recognition.background_noise),
                ctx.clone(),
            );
            let (tx, rx) = bounded(LIVE_AUDIO_QUEUE_CAPACITY);
            let capture_tx = if let Some(sink) = recording.clone() {
                let (capture, worker) = spawn_meeting_audio_router(tx, sink, *source);
                active.routers.push(worker);
                capture
            } else {
                tx
            };
            let scope = ChannelScope::new(owner.clone());
            let session = start_session(
                rx,
                scoped_events(scope.clone(), self.event_tx.clone()),
                config,
            );
            scope
                .stream_id
                .store(session.stream_id(), Ordering::Release);
            if task.plugin.as_ref().is_none_or(|binding| binding.host_tts) {
                session.set_tts_enabled(self.tts_enabled);
            }
            active.channels.push(TaskChannel {
                source: *source,
                scope,
                session,
                audio_tx: Some(capture_tx),
                recognition,
                microphone_device_id: self.selected_device_id.clone(),
                system_audio_input: self.system_audio_input.clone(),
                capturing: false,
            });
        }
        if meeting {
            self.meeting_plugin
                .event_sink
                .begin_sessions(active.channels.len());
        }
        for channel in &mut active.channels {
            if !self.input_enabled(channel.source) {
                channel.session.pause();
                continue;
            }
            if let Err(error) = self.start_task_capture(channel) {
                self.fail_task_startup(&owner, &error);
                self.last_error = Some(error);
                for channel in &active.channels {
                    self.audio_system.stop_capture_group(channel.scope.id());
                }
                active.cancel();
                if meeting && let Some(recording) = self.meeting_plugin.meeting_recording.take() {
                    if let Err(error) = recording.stop_without_finalizing() {
                        self.meeting_plugin.set_error(error.to_string());
                    }
                }
                self.release_unused_inputs();
                return;
            }
            channel.capturing = true;
        }
        self.audio_tasks.push(active);
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
                for task in std::mem::take(&mut self.pending_translations) {
                    self.start_prepared_task(task, ctx.clone());
                }
            }
            backend::BackendStatus::Starting(stage) if std::time::Instant::now() < deadline => {
                self.set_connection_status(stage.message());
            }
            backend::BackendStatus::Starting(_) => {
                self.backend_start_deadline = None;
                self.cancel_text_tasks(None);
                self.backend_manager.shutdown();
                self.set_startup_error(
                    "Startup timed out",
                    "Local services did not become ready within 180 seconds".into(),
                );
            }
            backend::BackendStatus::Failed(error) => {
                self.backend_start_deadline = None;
                self.set_startup_error("Startup failed", error.clone());
            }
        }
    }

    fn refresh_selected_input_config(&mut self) {
        let result = match self.capture_source {
            CaptureSource::Microphone => self.audio_system.input_config(&self.selected_device_id),
            CaptureSource::SystemAudio => match &self.system_audio_input {
                SystemAudioInputSelection::Application { application } => {
                    if self
                        .audio_applications
                        .iter()
                        .any(|candidate| candidate.id == application.id.0)
                    {
                        Ok(InputConfigInfo {
                            sample_rate: audio::AUDIO_ROUTE_SAMPLE_RATE,
                            channels: 2,
                            sample_format: "F32 application loopback".into(),
                        })
                    } else {
                        Err(format!(
                            "{} is not running or has no Windows audio session",
                            application.display_name
                        ))
                    }
                }
                SystemAudioInputSelection::Endpoint { device_id } => {
                    self.audio_system.loopback_config(device_id)
                }
            },
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

    fn request_audio_device_refresh(&mut self) {
        if self.device_refresh_rx.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .last_device_refresh_request
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(500))
        {
            return;
        }
        self.last_device_refresh_request = Some(now);

        let (tx, rx) = bounded(1);
        self.device_refresh_rx = Some(rx);
        let spawn_result = std::thread::Builder::new()
            .name("audio-device-refresh".into())
            .spawn(move || {
                let audio_system = AudioSystem::new();
                let snapshot = AudioDeviceSnapshot {
                    devices: audio_system.available_devices(),
                    loopback_devices: audio_system.available_loopback_devices(),
                    output_devices: audio_system.available_output_devices(),
                };
                let _ = tx.send(snapshot);
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
                self.tts_output_devices = snapshot.output_devices;
                let refreshed_device_id = (!self.selected_device_id.is_empty()).then(|| {
                    audio::matching_available_input_id(&self.selected_device_id, &self.devices)
                        .unwrap_or_default()
                        .to_owned()
                });
                if let Some(refreshed_device_id) = refreshed_device_id
                    && refreshed_device_id != self.selected_device_id
                {
                    log::info!(
                        "Selected microphone '{}' changed to '{}'.",
                        self.selected_device_id,
                        if refreshed_device_id.is_empty() {
                            "default"
                        } else {
                            &refreshed_device_id
                        }
                    );
                    self.selected_device_id = refreshed_device_id;
                    if let Err(error) = self.sync_translation_input_to_audio_studio() {
                        log::warn!("Could not update Audio Studio after device refresh: {error}");
                    }
                    self.save_settings();
                    self.refresh_selected_input_config();
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.device_refresh_rx = None;
                self.last_error = Some("Audio device refresh stopped unexpectedly".into());
            }
        }
    }

    fn request_audio_application_refresh(&mut self) {
        if self.application_refresh_rx.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .last_application_refresh_request
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(500))
        {
            return;
        }
        self.last_application_refresh_request = Some(now);
        let (tx, rx) = bounded(1);
        self.application_refresh_rx = Some(rx);
        let spawn_result = std::thread::Builder::new()
            .name("audio-application-refresh".into())
            .spawn(move || {
                let audio_system = AudioSystem::new();
                let result = audio_system.try_available_audio_applications();
                let _ = tx.send(result);
            });
        if let Err(error) = spawn_result {
            self.application_refresh_rx = None;
            log::warn!("Could not refresh audio applications: {error}");
        }
    }

    fn poll_audio_application_refresh(&mut self) {
        let Some(rx) = &self.application_refresh_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(applications)) => {
                self.application_refresh_rx = None;
                if self.audio_applications != applications {
                    self.audio_applications = applications;
                }
            }
            Ok(Err(error)) => {
                self.application_refresh_rx = None;
                // A transient COM/session-enumeration failure must not erase
                // the last-good application list or unrelated host errors.
                log::warn!("Could not refresh audio applications: {error}");
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.application_refresh_rx = None;
                log::warn!("Audio application refresh stopped unexpectedly");
            }
        }
    }

    fn discover_audio_sources_on_page_entry(&mut self) {
        let page = self.navigation.page;
        if self.last_audio_discovery_page == Some(page) {
            return;
        }
        self.last_audio_discovery_page = Some(page);
        if matches!(page, Page::Translation | Page::AudioStudio) {
            self.request_audio_device_refresh();
            self.request_audio_application_refresh();
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
            self.meeting_plugin.event_sink.fail_active(&error);
            if let Some(owner) = self
                .audio_tasks
                .iter()
                .find(|task| task.owner.is_plugin(PluginId::MEETING.as_str()))
                .map(|task| task.owner.clone())
            {
                self.stop_task_owner(&owner);
            }
            self.meeting_plugin.clear_audio_import();
            self.finalize_meeting_recording();
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
                    self.last_error = Some(error);
                    self.stop_plugin_task(PluginId::VIDEO_PLAYER);
                    self.host_audio_import = None;
                }
            }
        }
        if host_completed {
            self.host_audio_import = None;
            self.set_connection_status("Media translation completed");
        }
    }

    fn start_task_capture(
        &mut self,
        channel: &translation_service::TaskChannel,
    ) -> Result<(), String> {
        if channel.audio_tx.is_none() {
            return Ok(());
        }
        self.audio_system.stop_capture_group(channel.scope.id());
        self.audio_system.set_capture_group(channel.scope.id());
        initialize_live_audio(
            self,
            |app| app.audio_system.set_tts_enabled(app.tts_enabled),
            |app, _| app.open_task_capture(channel),
        )
    }

    fn open_task_capture(
        &mut self,
        channel: &translation_service::TaskChannel,
    ) -> Result<(), String> {
        let Some(tx) = &channel.audio_tx else {
            return Ok(());
        };
        let result = match channel.source {
            CaptureSource::Microphone => self.audio_system.start_capture(
                &channel.microphone_device_id,
                tx.clone(),
                self.input_level.clone(),
            ),
            CaptureSource::SystemAudio => match &channel.system_audio_input {
                SystemAudioInputSelection::Endpoint { device_id } => self
                    .audio_system
                    .start_loopback_capture(device_id, tx.clone(), self.loopback_level.clone()),
                SystemAudioInputSelection::Application { application } => self
                    .audio_system
                    .try_available_audio_applications()
                    .and_then(|applications| {
                        applications
                            .iter()
                            .find(|candidate| candidate.id == application.id.0)
                            .ok_or_else(|| {
                                format!(
                                    "{} is not running or has no audio session",
                                    application.display_name
                                )
                            })
                            .and_then(|candidate| {
                                self.audio_system.start_application_loopback_capture(
                                    candidate.process_id,
                                    &application.display_name,
                                    tx.clone(),
                                    self.loopback_level.clone(),
                                )
                            })
                    }),
            },
            CaptureSource::Both => unreachable!("Capture uses individual sources"),
        };
        self.audio_system.set_capture_group(0);
        if result.is_err() {
            self.audio_system.stop_capture_group(channel.scope.id());
        }
        result
    }

    fn reset_audio_levels(&self) {
        for source in CaptureSource::Both.routes() {
            self.reset_input_level(*source);
        }
    }

    fn reset_input_level(&self, source: CaptureSource) {
        let (level, vad) = match source {
            CaptureSource::Microphone => (&self.input_level, &self.microphone_vad_active),
            CaptureSource::SystemAudio => (&self.loopback_level, &self.loopback_vad_active),
            CaptureSource::Both => unreachable!("Input controls use individual sources"),
        };
        level.store(0f32.to_bits(), Ordering::Relaxed);
        vad.store(false, Ordering::Relaxed);
    }

    fn switch_capture_device(&mut self, source: CaptureSource, previous_device_id: String) {
        let previous_system = self.system_audio_input.clone();
        if source == CaptureSource::SystemAudio {
            self.system_audio_input = SystemAudioInputSelection::Endpoint {
                device_id: self.selected_loopback_device_id.clone(),
            };
        }
        if let Err(error) = self.sync_translation_input_to_audio_studio() {
            match source {
                CaptureSource::Microphone => self.selected_device_id = previous_device_id,
                CaptureSource::SystemAudio => {
                    self.selected_loopback_device_id = previous_device_id;
                    self.system_audio_input = previous_system;
                }
                _ => {}
            }
            self.last_error = Some(error);
            return;
        }
        self.refresh_selected_input_config();
        self.save_settings();
        self.restart_host_captures(Some(source));
    }

    fn restart_host_captures(&mut self, source: Option<CaptureSource>) {
        let mut tasks = std::mem::take(&mut self.audio_tasks);
        for task in tasks
            .iter_mut()
            .filter(|task| task.owner.is_host() && task.is_live())
        {
            for channel in &mut task.channels {
                if source.is_some_and(|source| source != channel.source) {
                    continue;
                }
                let old_mic = channel.microphone_device_id.clone();
                let old_system = channel.system_audio_input.clone();
                channel.microphone_device_id = self.selected_device_id.clone();
                channel.system_audio_input = self.system_audio_input.clone();
                if !channel.capturing {
                    continue;
                }
                if let Err(error) = self.start_task_capture(channel) {
                    channel.microphone_device_id = old_mic;
                    channel.system_audio_input = old_system;
                    channel.capturing = self.start_task_capture(channel).is_ok();
                    self.last_error = Some(error);
                }
            }
        }
        self.audio_tasks = tasks;
    }

    fn switch_capture_source(&mut self, previous_source: CaptureSource) {
        if let Err(error) = self.sync_translation_input_to_audio_studio() {
            self.capture_source = previous_source;
            self.last_error = Some(error);
        }
        self.refresh_selected_input_config();
        self.save_settings();
    }

    fn apply_language_route(&mut self) {
        let Some(selection) =
            self.select_languages(&self.source_lang.clone(), &self.target_lang.clone())
        else {
            return;
        };
        self.active_languages = Some(selection);
        self.save_settings();
        for session in self.host_channels().map(|channel| &channel.session) {
            session.update_language_route(self.source_lang.clone(), self.target_lang.clone());
        }
    }

    fn set_tts_enabled(&mut self, enabled: bool) {
        if enabled && !self.service_config.tts_is_configured() {
            self.tts_enabled = false;
            self.audio_system.set_tts_enabled(false);
            self.last_error =
                Some("Configure a TTS provider in Settings before enabling TTS.".into());
            return;
        }
        self.tts_enabled = enabled
            && crate::feature_access::is_available(crate::feature_access::Feature::TtsPlayback);
        self.audio_system.set_tts_enabled(self.tts_enabled);
        self.save_settings();
        if !self.tts_enabled {
            self.audio_system.clear_tts_playback();
        }
        for session in self.host_channels().map(|channel| &channel.session) {
            session.set_tts_enabled(self.tts_enabled);
        }
    }

    fn begin_voice_clone(&mut self) {
        if let Some(channel) = self
            .host_channels()
            .find(|channel| channel.source == CaptureSource::Microphone && channel.capturing)
        {
            channel.session.begin_voice_clone();
        } else {
            self.companion_inbox
                .post("Start microphone translation to clone your voice.");
        }
    }

    fn voice_clone_state(&self) -> Option<&xrtranslate_protocol::VoiceCloneState> {
        self.microphone_clone_state.as_ref()
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
        #[cfg(any(windows, target_os = "linux"))]
        if !enabled {
            self.stop_ocr();
            self.ocr.reset_window();
        }
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
        let recognition = recognition.clone();
        self.save_settings();
        for task in self
            .audio_tasks
            .iter_mut()
            .filter(|task| task.owner.is_host())
        {
            let (source_language, target_language) = task.languages.wire();
            for channel in task
                .channels
                .iter_mut()
                .filter(|channel| channel.source == source)
            {
                channel.recognition = recognition.clone();
                channel.session.update_audio_segmentation(
                    vad_threshold_for_background_noise(recognition.background_noise),
                    pause_tolerance_to_ms(recognition.pause_tolerance),
                    recognition.continuous_recognition,
                    source_language.clone(),
                    target_language.clone(),
                );
            }
        }
    }

    pub(crate) fn clear_history(&mut self) {
        self.translations.clear();
        self.recognition_history.clear();
        self.partial_text.clear();
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.translations.clear();
            state.translation_previews.clear();
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
    ) -> bool {
        self.submit_text_translation(text, source_lang, target_lang, None)
    }

    fn submit_text_translation(
        &mut self,
        text: &str,
        source_lang: Option<String>,
        target_lang: Option<String>,
        plugin: Option<PluginSessionBinding>,
    ) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }
        let source = source_lang.unwrap_or_else(|| self.source_lang.clone());
        let target = target_lang.unwrap_or_else(|| self.target_lang.clone());
        match self
            .service_config
            .language_capabilities()
            .and_then(|capabilities| {
                text_translation::select_languages(trimmed, &source, &target, capabilities)
            }) {
            Ok(languages) => self.start_translation_task(
                TranslationTask::text(trimmed.to_owned(), languages, plugin),
                None,
            ),
            Err(error) => {
                self.last_error = Some(error);
                false
            }
        }
    }

    pub(crate) fn pause_active_meeting(&mut self) {
        for task in self
            .audio_tasks
            .iter_mut()
            .filter(|task| task.owner.is_plugin(PluginId::MEETING.as_str()))
        {
            if !task.is_live() {
                continue;
            }
            task.paused = true;
            for channel in &mut task.channels {
                channel.session.pause();
                self.audio_system.stop_capture_group(channel.scope.id());
                channel.capturing = false;
            }
        }
        if let Some(recording) = &self.meeting_plugin.meeting_recording
            && let Err(error) = recording.checkpoint()
        {
            self.meeting_plugin.set_error(error.to_string());
        }
        let _ = self.meeting_plugin.controller.pause_capture();
        self.release_unused_inputs();
    }

    pub(crate) fn resume_active_meeting(&mut self) -> bool {
        self.resume_meeting_capture(Self::start_task_capture)
    }

    fn resume_meeting_capture(
        &mut self,
        mut capture: impl FnMut(&mut Self, &translation_service::TaskChannel) -> Result<(), String>,
    ) -> bool {
        let Some(index) = self
            .audio_tasks
            .iter()
            .position(|task| task.owner.is_plugin(PluginId::MEETING.as_str()) && task.is_live())
        else {
            return false;
        };
        self.enable_translation_service();
        let mut task = self.audio_tasks.remove(index);
        for source in CaptureSource::Both.routes() {
            if task
                .channels
                .iter()
                .any(|channel| channel.source == *source)
            {
                self.set_live_input(*source, true, None, false, &mut capture);
            }
        }
        let mut ok = true;
        for channel in &mut task.channels {
            if channel.capturing {
                continue;
            }
            match capture(self, channel) {
                Ok(()) => {
                    channel.capturing = true;
                    channel.session.resume();
                }
                Err(error) => {
                    self.meeting_plugin.set_error(error);
                    ok = false;
                }
            }
        }
        task.paused = !ok;
        if !ok {
            for channel in &mut task.channels {
                self.audio_system.stop_capture_group(channel.scope.id());
                channel.session.pause();
                channel.capturing = false;
            }
        }
        self.audio_tasks.insert(index, task);
        self.microphone_enabled = self.input_capturing(CaptureSource::Microphone);
        self.system_audio_enabled = self.input_capturing(CaptureSource::SystemAudio);
        if ok {
            if let Err(error) = self.meeting_plugin.controller.resume_active_meeting() {
                self.meeting_plugin.set_error(error.to_string());
            }
        }
        ok
    }

    pub(crate) fn export_open_meeting_markdown(&mut self) {
        let Some(bundle) = self.meeting_plugin.controller.bundle.as_ref() else {
            return;
        };
        let default_name = format!("{}.md", sanitize_export_name(&bundle.meeting.name));
        let markdown = plugins::meeting::store::render_markdown(bundle);
        if let Err(error) = file_dialog::FileDialog::new()
            .add_filter("Markdown", &["md"])
            .set_file_name(&default_name)
            .save(markdown)
        {
            self.meeting_plugin
                .set_error(format!("Could not export meeting: {error}"));
        }
    }

    fn stop(&mut self) {
        #[cfg(target_os = "android")]
        android::cancel_microphone_request();
        #[cfg(any(windows, target_os = "linux"))]
        self.stop_ocr();
        self.translation_enabled = false;
        self.microphone_enabled = false;
        self.system_audio_enabled = false;
        self.backend_start_deadline = None;
        self.cancel_pending_tasks(None);
        self.cancel_text_tasks(None);
        if let Ok(mut state) = self.shared_session_state.lock() {
            state.translation_enabled = false;
            for task in &self.audio_tasks {
                for channel in &task.channels {
                    channel.scope.cancel(&self.session_event_subscribers);
                    state.retire_stream(channel.session.stream_id());
                }
            }
            for entry in &mut state.translations {
                entry.live = false;
            }
            for entry in &mut state.recognition_history {
                entry.live = false;
            }
            state.partial_text.clear();
            state.translation_previews.clear();
            state.pending_final_asr.clear();
            state.pending_recognition_windows.clear();
            state.pending_route_change = None;
        }
        self.meeting_plugin.clear_audio_import();
        self.host_audio_import = None;
        self.audio_system.stop();
        let tasks = std::mem::take(&mut self.audio_tasks);
        for task in tasks {
            for channel in &task.channels {
                self.finish_caption_stream(channel.session.stream_id());
            }
            task.cancel();
        }
        self.finalize_meeting_recording();
        self.meeting_plugin.event_sink.cancel_sessions();
        self.meeting_plugin.event_sink.finish_active();
        self.player_plugin.poll_translation_events();
        self.player_plugin.stop_import();
        self.player_plugin.pause_task();
        self.active_languages = None;
        self.partial_text.clear();
        self.reset_audio_levels();
        self.set_connection_status("Stopped");
    }

    fn poll_text_translation(&mut self, ctx: &egui::Context) {
        if !self.translation_enabled {
            return;
        }
        for error in self.text_translation.poll(
            &mut self.backend_manager,
            &self.server_url,
            PromptGraphSet {
                graph: self.prompt_library.active_graph(),
            },
            Some(ctx.clone()),
            &self.event_tx,
        ) {
            self.last_error = Some(error);
        }
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

        let overlay_controls = overlay_ipc::OverlayControls {
            translation_enabled: self.translation_enabled,
            microphone_enabled: self
                .input_control_visible(CaptureSource::Microphone, false)
                .then_some(self.microphone_enabled),
            system_audio_enabled: self
                .input_control_visible(CaptureSource::SystemAudio, false)
                .then_some(self.system_audio_enabled),
            #[cfg(any(windows, target_os = "linux"))]
            ocr_enabled: self
                .plugin_enabled(PluginId::OCR)
                .then_some(self.ocr.enabled),
            #[cfg(not(any(windows, target_os = "linux")))]
            ocr_enabled: None,
        };
        let overlay_state = self
            .floating_subtitles_enabled
            .then(|| {
                self.shared_session_state.lock().ok().map(|state| {
                    state.overlay_state(
                        self.floating_subtitles_max_count,
                        self.floating_subtitles_font_size as u32,
                        self.microphone_vad_active.load(Ordering::Relaxed),
                        self.loopback_vad_active.load(Ordering::Relaxed),
                    )
                })
            })
            .flatten();
        let overlay_events = if let Ok(mut manager) = self.overlay_manager.lock() {
            if self.floating_subtitles_enabled {
                manager.set_language(self.ui_language);
                manager.set_controls(overlay_controls);
                // Consume close/failure events before considering another launch.
                let events = manager.poll_events();
                if events.iter().any(|event| {
                    matches!(
                        event,
                        overlay_ipc::OverlayEvent::CloseRequested
                            | overlay_ipc::OverlayEvent::Failed(_)
                    )
                }) {
                    manager.stop();
                } else {
                    if let Some(state) = overlay_state {
                        manager.send_state(&state);
                    }
                    manager.start();
                }
                events
            } else {
                manager.stop();
                Vec::new()
            }
        } else {
            Vec::new()
        };
        for event in overlay_events {
            match event {
                overlay_ipc::OverlayEvent::CloseRequested
                | overlay_ipc::OverlayEvent::Failed(_) => {
                    self.overlay_enabled_atomic.store(false, Ordering::Relaxed);
                    self.set_floating_subtitles_enabled(false);
                    if let overlay_ipc::OverlayEvent::Failed(error) = event {
                        self.last_error = Some(error);
                    }
                    break;
                }
                overlay_ipc::OverlayEvent::TranslationEnabled(enabled) => {
                    if enabled {
                        self.start(None);
                    } else {
                        self.stop();
                    }
                }
                overlay_ipc::OverlayEvent::MicrophoneEnabled(enabled) => {
                    self.set_microphone_enabled(enabled, None);
                }
                overlay_ipc::OverlayEvent::SystemAudioEnabled(enabled) => {
                    self.set_system_audio_enabled(enabled, None);
                }
                event => {
                    #[cfg(any(windows, target_os = "linux"))]
                    self.handle_ocr_event(event);
                    #[cfg(not(any(windows, target_os = "linux")))]
                    let _ = event;
                }
            }
        }

        // Apply window invalidation before accepting recognition from its old area.
        #[cfg(any(windows, target_os = "linux"))]
        self.poll_ocr();

        // Copy latest shared state into self for local rendering when main UI is visible
        let mut open_provider_configuration = false;
        if let Ok(mut state) = self.shared_session_state.lock() {
            self.connection_status = state.connection_status.clone();
            self.partial_text = state.partial_text.clone();
            self.recognition_history = state.recognition_history.clone();
            self.translations = state.translations.clone();
            self.translations
                .extend(state.translation_previews.iter().cloned());
            let prev_microphone_clone_state = self.microphone_clone_state.clone();
            let prev_loopback_clone_state = self.loopback_clone_state.clone();
            self.microphone_clone_state = state.microphone_clone_state.clone();
            self.loopback_clone_state = state.loopback_clone_state.clone();
            if self.microphone_clone_state != prev_microphone_clone_state
                || self.loopback_clone_state != prev_loopback_clone_state
            {
                self.save_settings();
            }
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
            if let Some((source_lang, target_lang)) = state.pending_route_change.take() {
                self.active_languages = LanguageSelection::parse(&source_lang, &target_lang).ok();
                if self.audio_tasks.iter().any(|task| task.owner.is_host()) {
                    self.source_lang = source_lang;
                    self.target_lang = target_lang;
                }
                // Dynamic runtime route changes update live session state without
                // permanently overwriting the user's base configuration in config.json.
            }
            if let Some(err) = &state.last_error {
                self.last_error = Some(err.clone());
            }
            open_provider_configuration =
                std::mem::take(&mut state.provider_configuration_required);
        }
        self.poll_translation_tasks();
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
    fn current_page_name(&self) -> String {
        if self.first_run {
            format!("Onboarding:{}", self.onboarding_page)
        } else {
            match self.navigation.page {
                Page::Translation => "Translation".to_string(),
                Page::Settings => "Settings".to_string(),
                Page::AudioStudio => "AudioStudio".to_string(),
                Page::PromptStudio => "PromptStudio".to_string(),
                Page::CorpusStudio => "CorpusStudio".to_string(),
                Page::TtsCenter => "TtsCenter".to_string(),
                Page::Plugin(PluginId::OSC) => "Plugin:OSC".to_string(),
                Page::Plugin(PluginId::MEETING) => "Plugin:Meeting".to_string(),
                Page::Plugin(PluginId::VR_OVERLAY) => "Plugin:VROverlay".to_string(),
                Page::Plugin(PluginId::VIDEO_PLAYER) => "Plugin:VideoPlayer".to_string(),
                Page::Plugin(_) => "Plugin".to_string(),
            }
        }
    }
}

impl Drop for XRTranslateApp {
    fn drop(&mut self) {
        self.stop();
        if let Some(route) = self.voicemeeter_route.take() {
            let _ = route.clear();
        }
        let _ = self.stop_audio_studio_managed_voicemeeter();
    }
}

impl eframe::App for XRTranslateApp {
    #[cfg(target_os = "android")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        android::ime::apply_input(ctx, input);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        self.poll_android_text_actions(ctx);
        if !self.first_run && !ctx.input(|input| input.viewport().visible().unwrap_or(true)) {
            self.poll_backend_startup(Some(ctx.clone()));
            self.poll_text_translation(ctx);
            self.poll_session_events();
            self.player_plugin.poll_translation_events();
            ui::companion::tick_background(ctx, self);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::theme::install_context(ui.ctx(), self.ui_theme);
        ui::layout::begin_frame(ui.ctx());
        self.poll_file_dialogs();
        #[cfg(target_os = "android")]
        self.poll_android_text_actions(ui.ctx());
        #[cfg(any(windows, target_os = "linux"))]
        self.poll_ocr_cleanup();
        #[cfg(target_os = "android")]
        if let Some(result) = android::take_resources_result() {
            match result {
                Ok(()) => {
                    let _ = self.runtime_installer.prepare_for(
                        self.project_root(),
                        self.service_config.runtime_requirements(),
                    );
                }
                Err(error) => self.last_error = Some(error),
            }
        }
        #[cfg(target_os = "android")]
        if android::take_capture_stop_request() {
            self.set_microphone_enabled(false, Some(ui.ctx().clone()));
            self.last_error =
                Some("Microphone recording stopped. Start it again to continue.".into());
        }
        #[cfg(target_os = "android")]
        if let Some(granted) = android::take_microphone_result() {
            if granted {
                self.set_microphone_enabled(true, Some(ui.ctx().clone()));
            } else {
                self.last_error = Some("Allow microphone access before recording.".into());
            }
        }

        let page_name = self.current_page_name();
        ui::automation::begin_frame(&page_name);
        if let Some(page) = ui::automation::take_pending_page() {
            self.navigation.page = page;
            self.first_run = false;
        }
        if let Some(step) = ui::automation::take_pending_onboarding_step() {
            self.onboarding_page = step;
            self.first_run = true;
        }
        PluginRegistry::builtin()
            .normalize_active_page(&self.plugin_preferences, &mut self.navigation.page);
        if let Page::Plugin(id) = self.navigation.page
            && !self.plugin_enabled(id)
        {
            self.navigation.page = Page::Translation;
        }
        if self.navigation.page == Page::TtsCenter && !self.service_config.tts_is_configured() {
            self.navigation.page = Page::Translation;
        }
        if self.navigation.page != Page::PromptStudio || self.first_run {
            let actions = ui::pages::prompt_studio::leave_page(
                ui.ctx(),
                &mut self.prompt_studio,
                &self.prompt_library,
            );
            self.apply_prompt_studio_actions(actions);
        }

        self.model_task_manager.poll();
        if let Some(path) = self.runtime_installer.poll() {
            self.backend_manager.use_installed_llama_server(&path);
        }
        let runtime_requirements = self.service_config.runtime_requirements();
        let model_assets = self.service_config.selected_model_asset_ids();
        let failed_without_plan = matches!(
            self.runtime_installer.state(),
            runtime_install::RuntimeInstallState::Failed(_)
        ) && !self.runtime_installer.has_plan();
        if !self.runtime_installer.is_busy()
            && !self
                .runtime_installer
                .plan_matches(runtime_requirements, &model_assets)
            && !failed_without_plan
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
        self.poll_audio_application_refresh();
        self.discover_audio_sources_on_page_entry();
        self.reconcile_audio_studio_live_routing();
        self.poll_audio_import();
        self.player_plugin
            .on_visibility_changed(self.navigation.page == Page::Plugin(PluginId::VIDEO_PLAYER));
        let idle_repaint_interval = if self.translation_enabled {
            std::time::Duration::from_millis(33)
        } else {
            std::time::Duration::from_millis(100)
        };
        ui.ctx().request_repaint_after(idle_repaint_interval);
        self.tts_center.poll(
            &mut self.backend_manager,
            &self.server_url,
            ui.ctx(),
            !self.first_run && self.navigation.page == Page::TtsCenter,
        );
        if let Some((id, pcm)) = self.tts_center.catalog.preview.take()
            && !self.first_run
            && self.navigation.page == Page::TtsCenter
        {
            match self
                .audio_system
                .preview_voice(&pcm, xrtranslate_assets::voices::SAMPLE_RATE)
            {
                Ok(()) => self.tts_center.previewing = Some(id),
                Err(error) => self.tts_center.error = Some(error),
            }
        }
        if self.first_run || self.navigation.page != Page::TtsCenter {
            self.audio_system.stop_voice_preview();
            self.tts_center.previewing = None;
            if !self.tts_center.busy() {
                self.tts_center.loaded = false;
            }
        } else if !self.audio_system.voice_preview_playing() {
            self.tts_center.previewing = None;
        }
        if self.first_run {
            self.audio_system.set_audio_studio_metering(false);
            let companion_layout = ui::render_onboarding_fullscreen(self, ui);
            self.render_modal_layer(ui.ctx());
            ui::companion::show(
                ui.ctx(),
                self,
                ui::companion::Layout::Onboarding(companion_layout),
            );
            ui::layout::finish_frame(ui.ctx());
            ui::automation::finish_frame();
            return;
        }

        self.show_available_update();
        self.show_ready_update();

        self.poll_backend_startup(Some(ui.ctx().clone()));
        self.poll_text_translation(ui.ctx());
        self.poll_session_events();
        self.player_plugin.poll_translation_events();
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
                ui::theme::animation_timings(ui.ctx()).sidebar,
            );
            let eased_expand = ui::animation::AnimationSystem::ease_out_cubic(expand_factor);
            let compact_height = ui.available_height() < 500.0;
            let (min_w, max_w) = if compact_height {
                (44.0, 160.0)
            } else {
                (54.0, 200.0)
            };
            let sidebar_width = egui::lerp(min_w..=max_w, eased_expand);
            let (min_mx, max_mx) = if compact_height {
                (4.0, 8.0)
            } else {
                (8.0, 12.0)
            };
            let margin_x = egui::lerp(min_mx..=max_mx, eased_expand);
            let margin_y = if compact_height { 6 } else { 14 };

            let prev_collapsed = self.navigation.collapsed;
            let prev_page = self.navigation.page;

            // 1. Native Sidebar Panel (Animated width, full height)
            if ui.available_width() < 680.0 {
                egui::Panel::top("navigation_panel")
                    .frame(
                        egui::Frame::new()
                            .fill(ui::theme::sidebar(viewport_focused))
                            .inner_margin(8),
                    )
                    .show(ui, |ui| {
                        ui::render_top_navigation(
                            ui,
                            &mut self.navigation,
                            &self.plugin_preferences,
                            self.service_config.tts_is_configured(),
                            &mut self.modal_dialog,
                            &mut self.first_run,
                            &mut self.onboarding_page,
                            self.ui_language,
                        )
                    });
            } else {
                egui::Panel::left("sidebar_panel")
                    .resizable(false)
                    .exact_size(sidebar_width)
                    .frame(
                        egui::Frame::new()
                            .fill(ui::theme::sidebar(viewport_focused))
                            .stroke(egui::Stroke::new(1.0, ui::theme::border()))
                            .inner_margin(egui::Margin::symmetric(
                                margin_x.round() as i8,
                                margin_y,
                            )),
                    )
                    .show(ui, |ui| {
                        ui::render_sidebar(
                            ui,
                            &mut self.navigation,
                            &self.plugin_preferences,
                            self.service_config.tts_is_configured(),
                            &mut self.modal_dialog,
                            &mut self.first_run,
                            &mut self.onboarding_page,
                            self.ui_language,
                            eased_expand,
                        );
                    });
            }
            if self.navigation.collapsed != prev_collapsed || self.navigation.page != prev_page {
                self.save_settings();
                self.player_plugin.on_visibility_changed(
                    self.navigation.page == Page::Plugin(PluginId::VIDEO_PLAYER),
                );
            }
        }

        self.audio_system.set_audio_studio_metering(
            !self.first_run && self.navigation.page == Page::AudioStudio,
        );

        // 2. Native Central Content Panel (Takes 100% of remaining width and height)
        let central_frame = if is_player_fullscreen {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(10, 15, 26))
                .inner_margin(egui::Margin::ZERO)
        } else {
            let is_compact_screen = ui.available_width() < 550.0 || ui.available_height() < 500.0;
            egui::Frame::new()
                .fill(ui::theme::content_backdrop(viewport_focused))
                .shadow(egui::Shadow {
                    offset: [0, 0],
                    blur: 14,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(24),
                })
                .inner_margin(egui::Margin::symmetric(
                    if is_compact_screen { 12 } else { 24 },
                    if is_compact_screen { 10 } else { 20 },
                ))
        };

        let content = egui::CentralPanel::default()
            .frame(central_frame)
            .show(ui, |ui| {
                if self.navigation.page == Page::Translation {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::Translation,
                        |ui| ui::pages::translation::render(self, ui),
                    );
                    return;
                }
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
                if self.navigation.page == Page::AudioStudio {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::AudioStudio,
                        |ui| self.render_audio_studio_page(ui),
                    );
                    return;
                }
                if self.navigation.page == Page::PromptStudio {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::PromptStudio,
                        |ui| {
                            let project_root = self.project_root();
                            let actions = ui::pages::prompt_studio::render(
                                &self.prompt_library,
                                &mut self.prompt_studio,
                                ui,
                                self.ui_language,
                                self.service_config.translation_prompt_target(),
                                self.update_channel == client_settings::UpdateChannel::Beta,
                                &project_root,
                            );
                            self.apply_prompt_studio_actions(actions);
                        },
                    );
                    return;
                }
                if self.navigation.page == Page::TtsCenter {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::TtsCenter,
                        |ui| {
                            use ui::pages::tts_center::{self, Action};
                            let asr_languages = self
                                .language_capabilities()
                                .recognition
                                .unwrap_or(LanguageSet::ALL);
                            match tts_center::render(
                                &mut self.tts_center,
                                ui,
                                self.ui_language,
                                self.service_config.tts_is_configured(),
                                asr_languages,
                            ) {
                                Some(Action::Settings) => {
                                    self.navigation.page = Page::Settings;
                                    self.settings_section =
                                        ui::pages::settings::SettingsSection::ServiceProviders;
                                }
                                Some(Action::Preview(id)) => {
                                    if self.tts_center.previewing.as_deref() == Some(&id) {
                                        self.audio_system.stop_voice_preview();
                                        self.tts_center.previewing = None;
                                    } else {
                                        self.tts_center.catalog.preview(id, ui.ctx());
                                    }
                                }
                                None => {}
                            }
                        },
                    );
                    return;
                }
                if self.navigation.page == Page::CorpusStudio {
                    ui::animation::AnimationSystem::render_animated_page(
                        ui,
                        Page::CorpusStudio,
                        |ui| {
                            ui::pages::corpus_studio::render(
                                &mut self.corpus_studio,
                                &mut self.backend_manager,
                                ui,
                                self.ui_language,
                            )
                        },
                    );
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("main_scroll_area")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.navigation.page {
                        Page::Translation => unreachable!(),
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
                        Page::AudioStudio => unreachable!(),
                        Page::PromptStudio => unreachable!(),
                        Page::CorpusStudio => unreachable!(),
                        Page::TtsCenter => unreachable!(),
                    });
            });

        self.render_modal_layer(ui.ctx());
        ui::companion::show(
            ui.ctx(),
            self,
            ui::companion::Layout::Page {
                bounds: content.response.rect,
                layer: content.response.layer_id,
            },
        );
        ui::layout::finish_frame(ui.ctx());
        ui::automation::finish_frame();
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

pub fn run() -> eframe::Result<()> {
    #[cfg(not(target_os = "android"))]
    env_logger::init();

    #[cfg(debug_assertions)]
    {
        let mut vr_args = std::env::args().skip_while(|arg| arg != "--vr-avatar-render");
        if vr_args.next().is_some() {
            let path = vr_args.next().ok_or_else(|| {
                eframe::Error::AppCreation("--vr-avatar-render requires an output directory".into())
            })?;
            return plugins::vr_overlay::render_avatar_preview(std::path::Path::new(&path))
                .map_err(eframe::Error::AppCreation);
        }
        let mut render_args = std::env::args().skip_while(|arg| arg != "--avatar-render");
        if render_args.next().is_some() {
            let path = render_args.next().ok_or_else(|| {
                eframe::Error::AppCreation("--avatar-render requires an output directory".into())
            })?;
            return ui::components::avatar::render_views(std::path::Path::new(&path))
                .map_err(eframe::Error::AppCreation);
        }
    }

    #[cfg(windows)]
    configure_dll_search_paths();

    #[cfg(windows)]
    cleanup_runtime_cache();

    if std::env::args().any(|a| a == "--overlay") {
        #[cfg(any(windows, target_os = "linux"))]
        return overlay_native::run_native_overlay();
        #[cfg(not(any(windows, target_os = "linux")))]
        return Ok(());
    }

    let window_backdrop = window_backdrop::WindowBackdrop::from_environment();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size(ui::layout::BASE_MIN_INNER_SIZE)
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
    #[cfg(target_os = "android")]
    let options = android::native_options(options);
    #[cfg(windows)]
    let options = configure_transparent_wgpu(options, window_backdrop);
    eframe::run_native(
        "XRTranslate",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            #[cfg(target_os = "android")]
            android::install_clipboard(&cc.egui_ctx);
            #[cfg(target_os = "android")]
            android::ime::install(&cc.egui_ctx);
            ui::fonts::configure_multilingual_fonts(&cc.egui_ctx);
            ui::theme::apply_theme(&cc.egui_ctx);
            file_dialog::set_context(cc.egui_ctx.clone());
            #[cfg(target_os = "android")]
            cc.egui_ctx.all_styles_mut(|style| {
                style.spacing.interact_size.y = 40.0;
                style.spacing.button_padding = egui::vec2(8.0, 5.0);
            });
            if let Some(state) = cc.wgpu_render_state.as_ref() {
                log::info!("wgpu adapter: {:?}", state.adapter.get_info());
                ui::organic_border::install(
                    &state.device,
                    state.target_format,
                    &mut state.renderer.write(),
                );
                ui::components::avatar::install(
                    &state.device,
                    state.target_format,
                    &mut state.renderer.write(),
                );
            } else {
                log::warn!("wgpu render state is unavailable during app creation");
            }
            if let Err(error) = window_backdrop::apply(cc, window_backdrop) {
                log::warn!("Unable to configure {window_backdrop:?} window backdrop: {error}");
            }
            let args: Vec<String> = std::env::args().collect();
            let director_port = if let Some(idx) = args.iter().position(|a| a == "--director-port")
            {
                args.get(idx + 1).and_then(|p| p.parse::<u16>().ok())
            } else if let Some(idx) = args.iter().position(|a| a == "--director") {
                args.get(idx + 1)
                    .and_then(|p| p.parse::<u16>().ok())
                    .or(Some(ui::automation::DEFAULT_DIRECTOR_PORT))
            } else {
                None
            };
            ui::automation::init(cc.egui_ctx.clone(), director_port);

            let mut app = XRTranslateApp::default();
            #[cfg(target_os = "android")]
            android::prepare_resources(cc.egui_ctx.clone());
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
    use super::{
        AudioStudioAsrPlan, CaptureSource, SystemAudioInputSelection, XRTranslateApp,
        audio_studio::{AudioStudioPreset, graph_for_preset},
        compile_audio_studio_asr, compile_audio_studio_route, initialize_live_audio,
        vad_threshold_for_background_noise,
    };

    fn fixture_task(
        owner: crate::TranslationSessionOwner,
        sources: &[CaptureSource],
        finite: bool,
    ) -> (
        crate::translation_service::AudioTask,
        Vec<crate::network::TestCommands>,
    ) {
        use crate::translation_service::{AudioTask, ChannelScope, TaskChannel};
        let mut controls = Vec::new();
        let channels = sources
            .iter()
            .map(|source| {
                let (session, commands) = crate::network::test_session();
                controls.push(commands);
                let scope = ChannelScope::new(owner.clone());
                scope
                    .stream_id
                    .store(session.stream_id(), std::sync::atomic::Ordering::Release);
                TaskChannel {
                    source: *source,
                    scope,
                    session,
                    audio_tx: if finite {
                        None
                    } else {
                        Some(crossbeam_channel::bounded(4).0)
                    },
                    recognition: Default::default(),
                    microphone_device_id: "shared-mic".into(),
                    system_audio_input: SystemAudioInputSelection::Endpoint {
                        device_id: String::new(),
                    },
                    capturing: !finite,
                }
            })
            .collect();
        (
            AudioTask {
                owner,
                languages: xrtranslate_engine::language::LanguageSelection::parse("en", "zh")
                    .unwrap(),
                channels,
                paused: false,
                routers: Vec::new(),
            },
            controls,
        )
    }

    fn plugin_owner(
        id: crate::plugins::PluginId,
        operation: &str,
    ) -> crate::TranslationSessionOwner {
        crate::TranslationSessionOwner::Plugin(crate::session_coordinator::PluginSessionOwner::new(
            id.as_str(),
            operation,
            "Task",
            "Open",
            "Active",
        ))
    }

    #[test]
    fn microphone_switch_preserves_system_audio_and_finite_tasks_and_resumes_consumers() {
        use std::sync::atomic::Ordering;
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        app.microphone_enabled = true;
        let (meeting, _meeting_controls) = fixture_task(
            plugin_owner(crate::plugins::PluginId::MEETING, "meeting-1"),
            &[CaptureSource::Microphone, CaptureSource::SystemAudio],
            false,
        );
        let (home, _home_controls) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
            &[CaptureSource::Microphone],
            false,
        );
        let (file, _file_controls) = fixture_task(
            plugin_owner(crate::plugins::PluginId::VIDEO_PLAYER, "video-1"),
            &[CaptureSource::SystemAudio],
            true,
        );
        let scopes = meeting
            .channels
            .iter()
            .map(|channel| channel.scope.clone())
            .collect::<Vec<_>>();
        app.audio_tasks = vec![meeting, home, file];
        app.set_microphone_input(false, None, true, |_, _| {
            panic!("Off must not open capture")
        });
        assert!(app.translation_enabled);
        assert!(!app.microphone_enabled);
        assert_eq!(app.audio_tasks.len(), 3);
        assert!(!app.input_capturing(CaptureSource::Microphone));
        assert!(app.input_capturing(CaptureSource::SystemAudio));
        assert!(app.audio_tasks[0].channels[0].session.paused_for_test());
        assert!(!app.audio_tasks[0].channels[1].session.paused_for_test());
        assert!(
            scopes
                .iter()
                .all(|scope| scope.active.load(Ordering::Acquire))
        );
        let mut resumed = Vec::new();
        app.set_microphone_input(true, None, false, |_, channel| {
            resumed.push(channel.session.stream_id());
            Ok(())
        });
        assert_eq!(resumed.len(), 2);
        assert_eq!(app.audio_tasks.len(), 3);
        assert!(app.microphone_enabled);
        assert!(app.input_capturing(CaptureSource::Microphone));
    }

    #[test]
    fn task_stop_releases_only_its_channels_global_stop_preserves_draft_and_cancels_everything() {
        use std::sync::atomic::Ordering;
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        app.microphone_enabled = true;
        let meeting_owner = plugin_owner(crate::plugins::PluginId::MEETING, "meeting-1");
        let (meeting, _a) =
            fixture_task(meeting_owner.clone(), &[CaptureSource::Microphone], false);
        let (home, _b) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
            &[CaptureSource::Microphone],
            false,
        );
        let (file, _c) = fixture_task(
            plugin_owner(crate::plugins::PluginId::VIDEO_PLAYER, "video-1"),
            &[CaptureSource::SystemAudio],
            true,
        );
        let meeting_scope = meeting.channels[0].scope.clone();
        let home_scope = home.channels[0].scope.clone();
        let file_scope = file.channels[0].scope.clone();
        app.audio_tasks = vec![meeting, home, file];
        *app.osc_plugin.draft_input_mut() = "unsent words".into();
        app.stop_task_owner(&meeting_owner);
        assert!(!meeting_scope.active.load(Ordering::Acquire));
        assert!(home_scope.active.load(Ordering::Acquire));
        assert!(file_scope.active.load(Ordering::Acquire));
        assert_eq!(app.audio_tasks.len(), 2);
        assert!(app.translation_enabled);
        assert!(app.microphone_enabled);
        app.stop();
        app.stop();
        assert!(!app.translation_enabled);
        assert!(!app.microphone_enabled);
        assert!(app.audio_tasks.is_empty());
        assert!(app.pending_translations.is_empty());
        assert_eq!(app.osc_plugin.draft_input(), "unsent words");
        assert!(!home_scope.accepts_events());
        assert!(!file_scope.accepts_events());
        app.enable_translation_service();
        app.poll_translation_tasks();
        assert!(app.translation_enabled);
        assert_eq!(app.connection_status, "Ready");
        assert!(app.audio_tasks.is_empty());
        assert!(!app.microphone_enabled);
    }

    #[test]
    fn plugin_microphone_activation_has_no_extra_home_task_and_repeated_calls_are_idempotent() {
        let mut app = XRTranslateApp::default();
        let mut starts = 0;
        app.set_microphone_input(true, None, false, |_, _| {
            starts += 1;
            Ok(())
        });
        app.set_microphone_input(true, None, false, |_, _| {
            starts += 1;
            Ok(())
        });
        assert!(app.translation_enabled);
        assert!(app.microphone_enabled);
        assert!(app.audio_tasks.is_empty());
        assert_eq!(starts, 0);
    }

    #[test]
    fn manually_paused_meeting_is_not_resumed_by_the_global_microphone_control() {
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (mut meeting, _controls) = fixture_task(
            plugin_owner(crate::plugins::PluginId::MEETING, "meeting-1"),
            &[CaptureSource::Microphone],
            false,
        );
        meeting.paused = true;
        meeting.channels[0].capturing = false;
        app.audio_tasks.push(meeting);
        app.set_microphone_input(true, None, false, |_, _| {
            panic!("Manually paused input must stay paused")
        });
        assert!(app.audio_tasks[0].paused);
        assert!(!app.audio_tasks[0].channels[0].capturing);
    }

    #[test]
    fn resuming_a_meeting_also_resumes_other_waiting_microphone_consumers() {
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (mut meeting, _a) = fixture_task(
            plugin_owner(crate::plugins::PluginId::MEETING, "meeting-1"),
            &[CaptureSource::Microphone],
            false,
        );
        meeting.paused = true;
        meeting.channels[0].capturing = false;
        let (mut home, _b) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
            &[CaptureSource::Microphone],
            false,
        );
        home.channels[0].capturing = false;
        app.audio_tasks = vec![meeting, home];
        let mut resumed = Vec::new();
        assert!(app.resume_meeting_capture(|_, channel| {
            resumed.push(channel.session.stream_id());
            Ok(())
        }));
        assert_eq!(resumed.len(), 2);
        assert_eq!(app.audio_tasks.len(), 2);
        assert!(app.microphone_enabled);
        assert!(
            app.audio_tasks
                .iter()
                .all(|task| !task.paused && task.channels[0].capturing)
        );
    }

    #[test]
    fn failed_meeting_resume_does_not_leave_a_partially_running_capture() {
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (mut meeting, _a) = fixture_task(
            plugin_owner(crate::plugins::PluginId::MEETING, "meeting-1"),
            &[CaptureSource::SystemAudio, CaptureSource::Microphone],
            false,
        );
        meeting.paused = true;
        for channel in &mut meeting.channels {
            channel.capturing = false;
        }
        app.audio_tasks.push(meeting);
        assert!(!app.resume_meeting_capture(|_, channel| {
            if channel.source == CaptureSource::Microphone {
                Err("Device unplugged".into())
            } else {
                Ok(())
            }
        }));
        assert!(app.audio_tasks[0].paused);
        assert!(
            app.audio_tasks[0]
                .channels
                .iter()
                .all(|channel| !channel.capturing)
        );
        assert!(!app.microphone_enabled);
        assert!(app.translation_enabled);
    }

    #[test]
    fn microphone_failure_keeps_its_task_waiting_and_the_service_available() {
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (mut task, _controls) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::Microphone,
            },
            &[CaptureSource::Microphone],
            false,
        );
        task.channels[0].capturing = false;
        app.audio_tasks.push(task);
        app.set_microphone_input(true, None, false, |_, _| Err("Device unplugged".into()));
        app.poll_translation_tasks();
        assert!(app.translation_enabled);
        assert!(!app.microphone_enabled);
        assert_eq!(app.audio_tasks.len(), 1);
        assert_eq!(app.connection_status, "Waiting for microphone");
    }

    #[test]
    fn completed_or_failed_task_does_not_disable_the_service_or_another_task() {
        use std::sync::atomic::Ordering;
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (file, _a) = fixture_task(
            plugin_owner(crate::plugins::PluginId::VIDEO_PLAYER, "video-1"),
            &[CaptureSource::SystemAudio],
            true,
        );
        let (home, _b) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::SystemAudio,
            },
            &[CaptureSource::SystemAudio],
            false,
        );
        file.channels[0]
            .scope
            .finished
            .store(true, Ordering::Release);
        app.audio_tasks = vec![file, home];
        app.poll_translation_tasks();
        assert!(app.translation_enabled);
        assert_eq!(app.audio_tasks.len(), 1);
        assert!(app.input_capturing(CaptureSource::SystemAudio));
        app.audio_tasks[0].channels[0]
            .scope
            .failed
            .store(true, Ordering::Release);
        app.poll_translation_tasks();
        assert!(app.translation_enabled);
        assert!(app.audio_tasks.is_empty());
        assert_eq!(app.connection_status, "Ready");
    }

    #[test]
    fn changing_page_does_not_change_service_or_task_ownership() {
        let mut app = XRTranslateApp::default();
        app.enable_translation_service();
        let (task, _commands) = fixture_task(
            crate::TranslationSessionOwner::Host {
                capture_source: CaptureSource::SystemAudio,
            },
            &[CaptureSource::SystemAudio],
            false,
        );
        app.audio_tasks.push(task);
        let scope = app.audio_tasks[0].channels[0].scope.clone();
        app.navigation.page = crate::ui::Page::Plugin(crate::plugins::PluginId::OSC);
        app.navigation.page = crate::ui::Page::Translation;
        assert!(app.translation_enabled);
        assert!(scope.accepts_events());
        assert_eq!(app.audio_tasks.len(), 1);
    }

    #[test]
    fn translator_microphone_compiles_without_changing_recognition_or_monitor_policy() {
        use crate::audio_studio::{AudioLink, AudioNode, AudioNodeKind, DeviceId};
        let mut graph = graph_for_preset(AudioStudioPreset::TranslatorMicrophone);
        for node in &mut graph.nodes {
            if let AudioNodeKind::GameMicrophoneOutput { device_id, .. } = &mut node.kind {
                *device_id = Some(DeviceId::new("virtual-cable"));
            }
        }
        graph
            .links
            .iter_mut()
            .find(|link| link.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;
        graph.nodes.push(AudioNode::new(
            "monitor",
            "Monitor",
            AudioNodeKind::MonitorOutput { device_id: None },
        ));
        graph
            .links
            .push(AudioLink::new("monitor-link", "tts-gate", "monitor"));
        let plan = compile_audio_studio_route(&graph).unwrap();
        assert_eq!(plan.routes.len(), 2);
        let automatic = plan
            .routes
            .iter()
            .find(|route| route.output_device_id == "virtual-cable")
            .unwrap();
        assert!(automatic.follow_tts);
        assert!(automatic.microphone.is_some() && automatic.tts_gain.is_some());
        assert!(automatic.system_loopback.is_none() && automatic.media.is_empty());
        assert!(
            !plan
                .routes
                .iter()
                .find(|route| route.output_device_id.is_empty())
                .unwrap()
                .follow_tts
        );
        assert_eq!(
            plan.asr.as_ref().unwrap().capture_source,
            CaptureSource::Both
        );
        for node in &mut graph.nodes {
            if let AudioNodeKind::GameMicrophoneOutput { follow_tts, .. } = &mut node.kind {
                *follow_tts = false;
            }
        }
        assert_eq!(compile_audio_studio_route(&graph).unwrap().asr, plan.asr);
    }

    #[test]
    fn audio_source_gates_compile_in_wire_order_for_capture_media_and_tts() {
        use crate::audio_processing::SourceEffect;
        use crate::audio_studio::{
            AudioGraph, AudioLink, AudioNode, AudioNodeKind, AudioProcessor,
        };
        let mut graph = AudioGraph::new("test", "Ordered processors");
        graph.nodes = vec![
            AudioNode::new(
                "monitor",
                "Monitor",
                AudioNodeKind::MonitorOutput { device_id: None },
            ),
            AudioNode::new("tts", "TTS", AudioNodeKind::TextToSpeech),
            AudioNode::new(
                "gain",
                "Gain",
                AudioNodeKind::Processing {
                    processor: AudioProcessor::Gain { gain_db: -6.0 },
                },
            ),
            AudioNode::new(
                "media",
                "Media",
                AudioNodeKind::Media {
                    source: Some("track.wav".into()),
                    loop_playback: true,
                },
            ),
            AudioNode::new("mixer", "Mix", AudioNodeKind::Mixer),
            AudioNode::new("mic", "Mic", AudioNodeKind::Microphone { device_id: None }),
            AudioNode::new("asr", "ASR", AudioNodeKind::AsrTap),
        ];
        graph.links = vec![
            AudioLink::new("mic-gain", "mic", "gain"),
            AudioLink::to_mixer_input("gain-mix", "gain", "mixer", 0),
            AudioLink::to_mixer_input("tts-mix", "tts", "mixer", 1),
            AudioLink::to_mixer_input("media-mix", "media", "mixer", 2),
            AudioLink::new("mix-monitor", "mixer", "monitor"),
            AudioLink::new("gain-asr", "gain", "asr"),
        ];
        graph.initialize_source_gates();
        let gate = graph
            .nodes
            .iter_mut()
            .find(|node| node.id.0 == "mic-gate")
            .unwrap();
        gate.kind = AudioNodeKind::Processing {
            processor: AudioProcessor::NoiseGate {
                threshold_db: -30.0,
            },
        };
        let first = compile_audio_studio_route(&graph).unwrap();
        let route = &first.routes[0];
        assert_eq!(
            route.microphone.as_ref().unwrap().effects,
            vec![
                SourceEffect::NoiseGate(-30.0),
                SourceEffect::Gain(10.0f32.powf(-6.0 / 20.0))
            ]
        );
        assert_eq!(route.tts_effects, vec![SourceEffect::NoiseGate(-45.0)]);
        assert_eq!(route.media[0].effects, vec![SourceEffect::NoiseGate(-45.0)]);
        assert_eq!(
            first.asr.as_ref().unwrap().microphone_effects,
            route.microphone.as_ref().unwrap().effects
        );
        graph.nodes.reverse();
        assert_eq!(
            compile_audio_studio_route(&graph).unwrap().routes,
            first.routes
        );
    }

    #[test]
    fn unchanged_audio_studio_asr_plan_matches_running_translation_settings() {
        let system_audio_input = SystemAudioInputSelection::Endpoint {
            device_id: "loopback".into(),
        };
        let plan = AudioStudioAsrPlan {
            capture_source: CaptureSource::Both,
            microphone_device_id: Some("microphone".into()),
            system_audio_input: Some(system_audio_input.clone()),
            microphone_effects: crate::audio_processing::default_source_effects(),
            system_audio_effects: crate::audio_processing::default_source_effects(),
        };

        assert!(plan.matches_current_settings(
            CaptureSource::Both,
            "microphone",
            &system_audio_input
        ));
        assert!(!plan.matches_current_settings(
            CaptureSource::SystemAudio,
            "microphone",
            &system_audio_input
        ));
    }

    #[test]
    fn audio_studio_translation_safe_preset_executes_both_asr_and_tts_monitor_branches() {
        let mut graph = graph_for_preset(AudioStudioPreset::TranslationSafe);
        graph
            .links
            .iter_mut()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;
        let execution = compile_audio_studio_route(&graph).unwrap();

        assert_eq!(execution.routes.len(), 1);
        assert!(execution.routes[0].tts_gain.is_some());
        assert_eq!(
            execution.asr.map(|plan| plan.capture_source),
            Some(CaptureSource::SystemAudio)
        );
    }

    #[test]
    fn audio_studio_karaoke_preset_is_a_render_route_without_an_asr_branch() {
        let mut graph = graph_for_preset(AudioStudioPreset::VrchatKaraoke);
        let game_microphone = graph
            .nodes
            .iter_mut()
            .find(|node| node.id.0 == "game-microphone")
            .expect("karaoke preset must contain game microphone output");
        if let super::audio_studio::AudioNodeKind::GameMicrophoneOutput { device_id, .. } =
            &mut game_microphone.kind
        {
            *device_id = Some(super::audio_studio::DeviceId::new("virtual-microphone"));
        }
        let execution = compile_audio_studio_route(&graph).unwrap();

        let route = execution
            .routes
            .first()
            .expect("karaoke must render its mix");
        assert!(route.microphone.is_some());
        assert!(route.system_loopback.is_some());
        assert!((route.output_ceiling - 10.0_f32.powf(-1.0 / 20.0)).abs() < 0.0001);
        assert!(execution.asr.is_none());
    }

    #[test]
    fn audio_studio_tts_conversation_preset_declares_a_microphone_input() {
        let mut graph = graph_for_preset(AudioStudioPreset::TtsToGameMicrophone);
        graph
            .links
            .iter_mut()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;
        let game_microphone = graph
            .nodes
            .iter_mut()
            .find(|node| node.id.0 == "game-microphone")
            .expect("TTS preset must contain game microphone output");
        if let super::audio_studio::AudioNodeKind::GameMicrophoneOutput { device_id, .. } =
            &mut game_microphone.kind
        {
            *device_id = Some(super::audio_studio::DeviceId::new("virtual-microphone"));
        }
        let execution = compile_audio_studio_route(&graph).unwrap();

        assert_eq!(execution.routes.len(), 1);
        assert!(execution.routes[0].tts_gain.is_some());
        assert_eq!(
            execution.asr.map(|plan| plan.capture_source),
            Some(CaptureSource::Microphone)
        );
    }

    #[test]
    fn audio_studio_complete_default_keeps_asr_when_render_branch_is_unconfigured() {
        let mut graph = graph_for_preset(AudioStudioPreset::CompleteAudioSystem);
        graph
            .links
            .iter_mut()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;

        let execution = compile_audio_studio_route(&graph).unwrap();
        assert_eq!(
            execution.routes.len(),
            1,
            "the TTS monitor is ready by default"
        );
        assert_eq!(
            execution.asr.map(|plan| plan.capture_source),
            Some(CaptureSource::Both)
        );
        assert!(compile_audio_studio_asr(&graph).unwrap().is_some());
    }

    #[test]
    fn audio_studio_asr_ignores_a_switched_off_mixer_input() {
        let mut graph = graph_for_preset(AudioStudioPreset::CompleteAudioSystem);
        graph
            .links
            .iter_mut()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;
        graph
            .links
            .iter_mut()
            .find(|link| link.id.0 == "gain-rec-sys-to-asr-mixer")
            .expect("complete graph must connect recognition system audio")
            .enabled = false;

        let plan = compile_audio_studio_asr(&graph)
            .unwrap()
            .expect("complete graph must compile an ASR plan");
        assert_eq!(plan.capture_source, CaptureSource::Microphone);
        assert!(plan.microphone_device_id.is_some());
        assert!(plan.system_audio_input.is_none());
    }

    #[test]
    fn audio_studio_complete_graph_compiles_each_configured_render_sink() {
        use super::audio_studio::{AudioNodeKind, DeviceId, NodeId, SystemAudioCapture};

        let mut graph = graph_for_preset(AudioStudioPreset::CompleteAudioSystem);
        graph
            .links
            .iter_mut()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap()
            .enabled = true;
        let bgm = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == NodeId::new("bgm"))
            .expect("complete graph must contain BGM");
        if let AudioNodeKind::SystemAudio {
            capture:
                SystemAudioCapture::Application {
                    application,
                    resolved_process_id,
                },
        } = &mut bgm.kind
        {
            *application = Some(super::audio_studio::ApplicationSelection {
                id: super::audio_studio::ApplicationId::new("music"),
                display_name: "Music".into(),
            });
            *resolved_process_id = Some(42);
        } else {
            panic!("complete graph BGM must use application capture");
        }
        let game_microphone = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == NodeId::new("game-microphone"))
            .expect("complete graph must contain game microphone output");
        if let AudioNodeKind::GameMicrophoneOutput { device_id, .. } = &mut game_microphone.kind {
            *device_id = Some(DeviceId::new("virtual-microphone"));
        }

        let execution = compile_audio_studio_route(&graph).unwrap();
        assert_eq!(execution.routes.len(), 2);
        assert!(execution.routes.iter().any(|route| {
            route.output_device_id == "virtual-microphone"
                && route.microphone.is_some()
                && route.system_loopback.is_some()
                && route.tts_gain.is_some()
        }));
    }

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
            |events, _| {
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
            |events, _| {
                events.push("capture-failed");
                Err("device unavailable".into())
            },
        )
        .unwrap_err();

        assert_eq!(error, "device unavailable");
        assert_eq!(events, ["output-ready", "capture-failed"]);
    }

    #[test]
    fn typed_translation_is_independent_of_audio_and_captures_draft_on_acceptance() {
        let mut app = XRTranslateApp::default();
        app.last_error = Some("settings write failed".into());
        *app.osc_plugin.draft_input_mut() = "hello".into();
        app.apply_osc_actions(vec![crate::OscUiAction::TranslateInput {
            text: "hello".into(),
            source_lang: "en".into(),
            target_lang: "zh".into(),
        }]);
        assert_eq!(app.osc_plugin.draft_input(), "");
        assert!(
            app.text_translation
                .scopes()
                .all(|scope| scope.owner.is_host())
        );
        assert!(app.translation_enabled);
        assert!(!app.microphone_enabled);
        assert!(app.audio_tasks.is_empty());
        assert!(app.text_translation.preparing());
        assert_eq!(app.last_error.as_deref(), Some("settings write failed"));
    }

    #[test]
    fn global_stop_cancels_preparing_text() {
        let mut app = XRTranslateApp::default();
        app.text_translation
            .submit(crate::TranslationTask::text(
                "hello".into(),
                xrtranslate_engine::language::LanguageSelection::parse("en", "zh").unwrap(),
                None,
            ))
            .unwrap();
        app.translation_enabled = true;
        app.stop();
        assert!(!app.text_translation.preparing());
        assert!(app.audio_tasks.is_empty());
        assert!(!app.translation_enabled);
    }

    #[test]
    fn typing_preserves_a_plugin_task_and_its_audio_route() {
        let mut app = XRTranslateApp::default();
        let owner = crate::TranslationSessionOwner::Plugin(
            crate::session_coordinator::PluginSessionOwner::new(
                "test-media",
                "task-1",
                "Media",
                "Open",
                "Working",
            ),
        );
        let (task, _commands) = fixture_task(owner.clone(), &[CaptureSource::SystemAudio], false);
        app.audio_tasks.push(task);
        app.translation_enabled = true;
        app.capture_source = CaptureSource::SystemAudio;
        let languages = xrtranslate_engine::language::LanguageSelection::parse("ja", "en").unwrap();
        app.active_languages = Some(languages);
        app.apply_osc_actions(vec![crate::OscUiAction::TranslateInput {
            text: "hello".into(),
            source_lang: "en".into(),
            target_lang: "zh".into(),
        }]);
        assert_eq!(app.audio_tasks.len(), 1);
        assert_eq!(app.audio_tasks[0].owner, owner);
        assert_eq!(app.active_languages, Some(languages));
        assert_eq!(app.capture_source, CaptureSource::SystemAudio);
        assert!(app.translation_enabled);
        assert!(app.text_translation.preparing());
    }

    #[test]
    fn startup_syncs_audio_studio_recognition_inputs_with_saved_capture_source() {
        let mut app = XRTranslateApp::default();
        app.capture_source = CaptureSource::Microphone;
        app.selected_device_id = "test-mic".into();
        app.sync_translation_input_to_audio_studio().unwrap();

        let graph = &app.audio_studio.settings().graph;
        let mic_link = graph
            .links
            .iter()
            .find(|l| l.id.0 == "gain-mic-asr-to-asr-mixer")
            .unwrap();
        let sys_link = graph
            .links
            .iter()
            .find(|l| l.id.0 == "gain-rec-sys-to-asr-mixer")
            .unwrap();
        let bus_link = graph
            .links
            .iter()
            .find(|l| l.id.0 == "asr-mixer-to-asr")
            .unwrap();

        assert!(mic_link.enabled, "Microphone input should be enabled");
        assert!(!sys_link.enabled, "System audio input should be disabled");
        assert!(
            !bus_link.enabled,
            "Translation bus should be disabled when not translating"
        );

        // Test system audio
        app.capture_source = CaptureSource::SystemAudio;
        app.sync_translation_input_to_audio_studio().unwrap();

        let graph = &app.audio_studio.settings().graph;
        let mic_link = graph
            .links
            .iter()
            .find(|l| l.id.0 == "gain-mic-asr-to-asr-mixer")
            .unwrap();
        let sys_link = graph
            .links
            .iter()
            .find(|l| l.id.0 == "gain-rec-sys-to-asr-mixer")
            .unwrap();

        assert!(!mic_link.enabled, "Microphone input should be disabled");
        assert!(sys_link.enabled, "System audio input should be enabled");
    }
}

impl XRTranslateApp {
    fn choose_file(
        &mut self,
        dialog: file_dialog::FileDialog,
        complete: impl FnOnce(&mut Self, std::path::PathBuf) + 'static,
    ) {
        let request = dialog.start_pick();
        if let Some(result) = request.poll() {
            match result {
                Ok(Some(path)) => complete(self, path),
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        } else {
            self.pending_file_dialogs
                .push((request, Box::new(complete)));
        }
    }
    fn poll_file_dialogs(&mut self) {
        for (request, complete) in std::mem::take(&mut self.pending_file_dialogs) {
            match request.poll() {
                Some(Ok(Some(path))) => complete(self, path),
                Some(Ok(None)) => {}
                Some(Err(error)) => self.last_error = Some(error),
                None => self.pending_file_dialogs.push((request, complete)),
            }
        }
        for error in file_dialog::take_errors() {
            self.last_error = Some(error);
        }
    }
}
