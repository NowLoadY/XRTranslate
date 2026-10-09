use super::{
    graph::{
        ApplicationId, AsrInputMode, AudioGraph, AudioLink, AudioNode, AudioNodeKind,
        AudioProcessor, DeviceId, GraphEndpoint, GraphId, GraphIssueCode, GraphPosition,
        GraphValidation, GraphValidationIssue, LinkId, NodeId, SystemAudioCapture,
        SystemCapturePolicy, VoiceMeeterBus,
    },
    persistence::{
        AudioStudioPersistenceError, AudioStudioRepository, AudioStudioSettings, DeviceDefaults,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, fmt, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioDeviceRole {
    MicrophoneCapture,
    SystemAudioCapture,
    MonitorRender,
    GameMicrophoneSink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMeeterEdition {
    Standard,
    Banana,
    Potato,
}

impl VoiceMeeterEdition {
    pub const fn supported_buses(self) -> &'static [VoiceMeeterBus] {
        match self {
            Self::Standard => &[VoiceMeeterBus::B1],
            Self::Banana => &[VoiceMeeterBus::B1, VoiceMeeterBus::B2],
            Self::Potato => &[VoiceMeeterBus::B1, VoiceMeeterBus::B2, VoiceMeeterBus::B3],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VoiceMeeterStripIndex(pub u8);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceMeeterInputSnapshot {
    pub strip_index: VoiceMeeterStripIndex,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
}

/// Present only when VoiceMeeter is installed. `running` is intentionally
/// independent from installation so the host can start it on route demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceMeeterSnapshot {
    pub edition: VoiceMeeterEdition,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub inputs: Vec<VoiceMeeterInputSnapshot>,
    #[serde(default)]
    pub buses: Vec<VoiceMeeterBus>,
}

impl VoiceMeeterSnapshot {
    pub fn supports_bus(&self, bus: VoiceMeeterBus) -> bool {
        self.edition.supported_buses().contains(&bus)
            && (self.buses.is_empty() || self.buses.contains(&bus))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAudioDevice {
    pub id: DeviceId,
    pub name: String,
    pub role: AudioDeviceRole,
    #[serde(default)]
    pub is_default: bool,
    /// VoiceMeeter input strip fed by this render endpoint, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voicemeeter_strip_index: Option<VoiceMeeterStripIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAudioApplication {
    pub id: ApplicationId,
    pub display_name: String,
    pub process_id: u32,
    #[serde(default)]
    pub active: bool,
}

impl HostAudioDevice {
    pub fn requires_voicemeeter(&self) -> bool {
        self.name.to_ascii_lowercase().contains("voicemeeter")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostAudioCapabilities {
    pub microphone_capture: bool,
    pub system_audio_capture: bool,
    pub application_audio_capture: bool,
    pub tts_feedback_suppression: bool,
    pub exclude_own_process_audio: bool,
    pub tts_source: bool,
    pub media_source: bool,
    pub monitor_output: bool,
    pub game_microphone_output: bool,
    /// The host can publish a game-visible microphone without a third-party
    /// virtual audio driver.
    pub game_microphone_without_external_driver: bool,
    /// The first host implementation has one render sink. A future mixer may
    /// report true without changing the persisted graph format.
    pub multiple_render_sinks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostAudioSnapshot {
    #[serde(default)]
    pub discovery_complete: bool,
    #[serde(default)]
    pub devices: Vec<HostAudioDevice>,
    #[serde(default)]
    pub applications: Vec<HostAudioApplication>,
    #[serde(default)]
    pub capabilities: HostAudioCapabilities,
    /// Host workflow state mirrored by the Translation Bus node.
    #[serde(default)]
    pub translation_workflow_running: bool,
    /// A plugin-owned translation session cannot be stopped or replaced from
    /// Audio Studio. The value is a user-facing owner label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_workflow_locked_by: Option<String>,
    /// `None` means VoiceMeeter is not installed; an installed but stopped
    /// instance is represented by `Some` with `running == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voicemeeter: Option<VoiceMeeterSnapshot>,
}

impl HostAudioSnapshot {
    fn automatic_device(&self, role: AudioDeviceRole) -> Option<&HostAudioDevice> {
        let mut candidates = self.devices.iter().filter(|device| device.role == role);
        let first = candidates.next()?;
        if first.is_default {
            return Some(first);
        }
        let mut only = Some(first);
        for candidate in candidates {
            if candidate.is_default {
                return Some(candidate);
            }
            only = None;
        }
        only
    }

    fn supports_role(&self, role: AudioDeviceRole, selected: Option<&DeviceId>) -> bool {
        selected.map_or_else(
            || self.automatic_device(role).is_some(),
            |selected| {
                self.devices
                    .iter()
                    .any(|device| device.role == role && selected == &device.id)
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioStudioLifecycle {
    Inactive,
    Activating {
        request_id: u64,
        graph_id: GraphId,
    },
    Active {
        request_id: u64,
        graph_id: GraphId,
    },
    Deactivating {
        request_id: u64,
        graph_id: GraphId,
    },
    Error {
        graph_id: Option<GraphId>,
        message: String,
    },
}

impl Default for AudioStudioLifecycle {
    fn default() -> Self {
        Self::Inactive
    }
}

impl AudioStudioLifecycle {
    pub fn graph_id(&self) -> Option<&GraphId> {
        match self {
            Self::Activating { graph_id, .. }
            | Self::Active { graph_id, .. }
            | Self::Deactivating { graph_id, .. } => Some(graph_id),
            Self::Error { graph_id, .. } => graph_id.as_ref(),
            Self::Inactive => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRiskSeverity {
    Blocking,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRiskCode {
    DirectEndpointFeedback,
    EndpointPlaybackReshare,
    AsrContentContamination,
    AcousticLeakage,
    RecognitionGapDuringTts,
    ApplicationReturnPath,
    TtsSharedToApplication,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRisk {
    pub severity: RouteRiskSeverity,
    pub code: RouteRiskCode,
    pub summary: String,
    pub detail: String,
    pub remediation: String,
    #[serde(default)]
    pub node_ids: Vec<NodeId>,
    #[serde(default)]
    pub link_ids: Vec<LinkId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouteRiskReport {
    pub risks: Vec<RouteRisk>,
}

impl RouteRiskReport {
    pub fn blocking_count(&self) -> usize {
        self.risks
            .iter()
            .filter(|risk| risk.severity == RouteRiskSeverity::Blocking)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.risks
            .iter()
            .filter(|risk| risk.severity == RouteRiskSeverity::Warning)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStudioUiSnapshot {
    pub graphs: Vec<(GraphId, String)>,
    pub selected_graph: AudioGraph,
    /// Host-resolved execution form of the selected graph. The editor keeps
    /// automatic device/application choices symbolic in `selected_graph`.
    pub resolved_graph: AudioGraph,
    pub lifecycle: AudioStudioLifecycle,
    pub live_routing_matches_graph: bool,
    pub validation: GraphValidation,
    pub risk_report: RouteRiskReport,
    pub host_audio: HostAudioSnapshot,
    pub device_defaults: DeviceDefaults,
    pub dirty: bool,
    pub last_error: Option<String>,
    /// Host-sampled signal envelopes used only for the graph visualization.
    /// These values are ephemeral and never become graph configuration.
    pub signal_levels: AudioStudioSignalLevels,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioStudioSignalLevels {
    pub microphone: f32,
    pub system_audio: f32,
    pub tts: f32,
    pub media: f32,
    pub output: f32,
    /// Peak levels before the source gate, sampled only while Audio Studio is open.
    pub microphone_input: Option<f32>,
    pub system_audio_input: Option<f32>,
    pub tts_input: Option<f32>,
    pub media_input: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum AudioStudioUiAction {
    SelectGraph(GraphId),
    NewGraph,
    DuplicateGraph,
    RenameGraph(String),
    DeleteGraph,
    ResetToDefault,
    ReplaceSelectedGraph(AudioGraph),
    AddNode(AudioNode),
    UpdateNode(AudioNode),
    RemoveNode(NodeId),
    SetNodeDevice {
        node_id: NodeId,
        device_id: Option<DeviceId>,
    },
    SetSystemAudioCapture {
        node_id: NodeId,
        capture: SystemAudioCapture,
    },
    SetNodeVoiceMeeterBus {
        node_id: NodeId,
        bus: Option<VoiceMeeterBus>,
    },
    MoveNode {
        node_id: NodeId,
        position: GraphPosition,
    },
    Connect {
        from: GraphEndpoint,
        to: GraphEndpoint,
    },
    Rewire {
        link_id: LinkId,
        from: GraphEndpoint,
        to: GraphEndpoint,
    },
    DeleteLink(LinkId),
    SetLinkEnabled {
        link_id: LinkId,
        enabled: bool,
    },
    SetDeviceDefaults(DeviceDefaults),
    Save,
    /// Discovery is requested by opening the application selector. It does
    /// not mutate graph state and never creates an undo entry.
    DiscoverApplications,
    ChooseMedia(NodeId),
    SetNodeGain {
        node_id: NodeId,
        gain_db: f32,
    },
    SetNoiseGateThreshold {
        node_id: NodeId,
        threshold_db: f32,
    },
    EnqueueTts {
        node_id: NodeId,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioStudioHostAction {
    DiscoverApplications,
    ConfigureAsrInput {
        graph: AudioGraph,
    },
    ChooseMedia {
        graph_id: GraphId,
        node_id: NodeId,
    },
    ActivateGraph {
        request_id: u64,
        graph: AudioGraph,
    },
    DeactivateGraph {
        request_id: u64,
    },
    EnqueueTts {
        graph_id: GraphId,
        node_id: NodeId,
        text: String,
    },
    SetTranslationWorkflowEnabled(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AudioStudioHostEvent {
    Activated {
        request_id: u64,
    },
    ActivationFailed {
        request_id: u64,
        message: String,
    },
    Deactivated {
        request_id: u64,
    },
    DeactivationFailed {
        request_id: u64,
        message: String,
    },
    MediaSelected {
        graph_id: GraphId,
        node_id: NodeId,
        source: String,
    },
}

pub struct AudioStudioController {
    repository: AudioStudioRepository,
    settings: AudioStudioSettings,
    lifecycle: AudioStudioLifecycle,
    dirty: bool,
    last_error: Option<String>,
    next_request_id: u64,
    graph_revision: u64,
    pending_graph_revision: Option<u64>,
    active_graph_revision: Option<u64>,
    failed_graph_revision: Option<u64>,
}

impl AudioStudioController {
    pub fn open(project_root: &Path) -> Self {
        Self::from_repository(AudioStudioRepository::open(project_root))
    }

    pub fn from_repository(repository: AudioStudioRepository) -> Self {
        match repository.load() {
            Ok(settings) => Self {
                repository,
                settings,
                lifecycle: AudioStudioLifecycle::Inactive,
                dirty: false,
                last_error: None,
                next_request_id: 1,
                graph_revision: 0,
                pending_graph_revision: None,
                active_graph_revision: None,
                failed_graph_revision: None,
            },
            Err(error) => Self {
                repository,
                settings: AudioStudioSettings::default(),
                lifecycle: AudioStudioLifecycle::Inactive,
                dirty: false,
                last_error: Some(error.to_string()),
                next_request_id: 1,
                graph_revision: 0,
                pending_graph_revision: None,
                active_graph_revision: None,
                failed_graph_revision: None,
            },
        }
    }

    #[allow(dead_code)]
    pub fn settings(&self) -> &AudioStudioSettings {
        &self.settings
    }

    #[allow(dead_code)]
    pub fn lifecycle(&self) -> &AudioStudioLifecycle {
        &self.lifecycle
    }

    pub fn snapshot(&self, host_audio: &HostAudioSnapshot) -> AudioStudioUiSnapshot {
        let selected_graph = self.settings.graph.clone();
        let resolved_graph =
            resolve_graph_devices(&selected_graph, &self.settings.device_defaults, host_audio);
        let risk_report = analyze_route_risks(&resolved_graph);
        let mut graphs = self
            .settings
            .all_graphs()
            .map(|graph| (graph.id.clone(), graph.name.clone()))
            .collect::<Vec<_>>();
        graphs.sort_by(|(left, _), (right, _)| left.0.cmp(&right.0));
        AudioStudioUiSnapshot {
            graphs,
            validation: validate_for_host(&resolved_graph, host_audio),
            risk_report,
            selected_graph,
            resolved_graph,
            lifecycle: self.lifecycle.clone(),
            live_routing_matches_graph: matches!(
                self.lifecycle,
                AudioStudioLifecycle::Active { .. }
            ) && self.active_graph_revision
                == Some(self.graph_revision),
            host_audio: host_audio.clone(),
            device_defaults: self.settings.device_defaults.clone(),
            dirty: self.dirty,
            last_error: self.last_error.clone(),
            signal_levels: AudioStudioSignalLevels::default(),
        }
    }

    pub fn handle_ui_action(
        &mut self,
        action: AudioStudioUiAction,
        host_audio: &HostAudioSnapshot,
    ) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        let changes_routing = matches!(
            &action,
            AudioStudioUiAction::SelectGraph(_)
                | AudioStudioUiAction::NewGraph
                | AudioStudioUiAction::DuplicateGraph
                | AudioStudioUiAction::DeleteGraph
                | AudioStudioUiAction::ResetToDefault
                | AudioStudioUiAction::ReplaceSelectedGraph(_)
                | AudioStudioUiAction::AddNode(_)
                | AudioStudioUiAction::UpdateNode(_)
                | AudioStudioUiAction::RemoveNode(_)
                | AudioStudioUiAction::SetNodeDevice { .. }
                | AudioStudioUiAction::SetSystemAudioCapture { .. }
                | AudioStudioUiAction::SetNodeVoiceMeeterBus { .. }
                | AudioStudioUiAction::SetNodeGain { .. }
                | AudioStudioUiAction::SetNoiseGateThreshold { .. }
                | AudioStudioUiAction::Connect { .. }
                | AudioStudioUiAction::Rewire { .. }
                | AudioStudioUiAction::DeleteLink(_)
                | AudioStudioUiAction::SetLinkEnabled { .. }
                | AudioStudioUiAction::SetDeviceDefaults(_)
        );
        let may_change_asr_input = matches!(
            &action,
            AudioStudioUiAction::SelectGraph(_)
                | AudioStudioUiAction::NewGraph
                | AudioStudioUiAction::DuplicateGraph
                | AudioStudioUiAction::DeleteGraph
                | AudioStudioUiAction::ResetToDefault
                | AudioStudioUiAction::ReplaceSelectedGraph(_)
                | AudioStudioUiAction::AddNode(_)
                | AudioStudioUiAction::UpdateNode(_)
                | AudioStudioUiAction::RemoveNode(_)
                | AudioStudioUiAction::SetNodeDevice { .. }
                | AudioStudioUiAction::SetSystemAudioCapture { .. }
                | AudioStudioUiAction::SetNodeGain { .. }
                | AudioStudioUiAction::SetNoiseGateThreshold { .. }
                | AudioStudioUiAction::SetDeviceDefaults(_)
                | AudioStudioUiAction::Connect { .. }
                | AudioStudioUiAction::Rewire { .. }
                | AudioStudioUiAction::DeleteLink(_)
                | AudioStudioUiAction::SetLinkEnabled { .. }
        );
        if matches!(
            action,
            AudioStudioUiAction::SelectGraph(_)
                | AudioStudioUiAction::NewGraph
                | AudioStudioUiAction::DuplicateGraph
                | AudioStudioUiAction::DeleteGraph
                | AudioStudioUiAction::ResetToDefault
        ) && host_audio.translation_workflow_locked_by.is_some()
        {
            return Err(AudioStudioControllerError::InvalidEdit(
                "Stop the active translation session before switching graphs".into(),
            ));
        }
        let routing_before = changes_routing.then(|| {
            (
                self.settings.graph.clone(),
                self.settings.device_defaults.clone(),
            )
        });
        match action {
            AudioStudioUiAction::SelectGraph(id) => {
                self.settings
                    .select_graph(&id)
                    .map_err(AudioStudioControllerError::InvalidEdit)?;
                self.dirty = true;
            }
            AudioStudioUiAction::NewGraph | AudioStudioUiAction::DuplicateGraph => {
                self.settings
                    .create_graph(matches!(action, AudioStudioUiAction::DuplicateGraph));
                self.dirty = true;
            }
            AudioStudioUiAction::DeleteGraph => {
                self.settings
                    .delete_selected_graph()
                    .map_err(AudioStudioControllerError::InvalidEdit)?;
                self.dirty = true;
            }
            AudioStudioUiAction::RenameGraph(name) => {
                if self.settings.graph.id.0 == super::DEFAULT_AUDIO_GRAPH_ID {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "Duplicate the default graph to rename it".into(),
                    ));
                }
                let name = name.trim();
                if name.is_empty() {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "Graph name cannot be empty".into(),
                    ));
                }
                self.settings.graph.name = name.into();
                self.dirty = true;
            }
            AudioStudioUiAction::ResetToDefault => {
                self.settings.reset_selected_graph();
                self.dirty = true;
            }
            AudioStudioUiAction::ReplaceSelectedGraph(graph) => {
                if graph.id != self.settings.graph.id {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "replacement graph ID must match the selected audio graph".into(),
                    ));
                }
                self.settings.graph = graph;
                self.settings.normalize();
                self.dirty = true;
            }
            AudioStudioUiAction::AddNode(node) => {
                let graph = self.selected_graph_mut();
                if graph.nodes.iter().any(|existing| existing.id == node.id) {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "node ID {} already exists",
                        node.id.0
                    )));
                }
                graph.nodes.push(node);
                graph.initialize_source_gates();
                self.dirty = true;
            }
            AudioStudioUiAction::UpdateNode(node) => {
                let graph = self.selected_graph_mut();
                let Some(existing) = graph.nodes.iter_mut().find(|item| item.id == node.id) else {
                    return Err(AudioStudioControllerError::NodeNotFound(node.id));
                };
                *existing = node;
                self.dirty = true;
            }
            AudioStudioUiAction::RemoveNode(node_id) => self.remove_node(node_id)?,
            AudioStudioUiAction::SetNodeDevice { node_id, device_id } => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && self.selected_graph().reaches_asr_sink(&node_id)
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                let graph = self.selected_graph_mut();
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
                match &mut node.kind {
                    AudioNodeKind::Microphone { device_id: current }
                    | AudioNodeKind::MonitorOutput { device_id: current }
                    | AudioNodeKind::GameMicrophoneOutput {
                        device_id: current, ..
                    } => {
                        *current = device_id;
                    }
                    _ => {
                        return Err(AudioStudioControllerError::InvalidEdit(
                            "this node does not select an audio device".into(),
                        ));
                    }
                }
                self.dirty = true;
            }
            AudioStudioUiAction::SetSystemAudioCapture { node_id, capture } => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && self.selected_graph().reaches_asr_sink(&node_id)
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                let graph = self.selected_graph_mut();
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
                let AudioNodeKind::SystemAudio { capture: current } = &mut node.kind else {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "only a System Audio node can select an endpoint or application".into(),
                    ));
                };
                *current = capture;
                self.dirty = true;
            }
            AudioStudioUiAction::SetNodeVoiceMeeterBus { node_id, bus } => {
                let graph = self.selected_graph_mut();
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
                let AudioNodeKind::GameMicrophoneOutput {
                    voicemeeter_bus, ..
                } = &mut node.kind
                else {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "VoiceMeeter buses can only be selected for an app microphone output"
                            .into(),
                    ));
                };
                *voicemeeter_bus = bus;
                self.dirty = true;
            }
            AudioStudioUiAction::SetNodeGain { node_id, gain_db } => {
                let graph = self.selected_graph_mut();
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
                let AudioNodeKind::Processing {
                    processor:
                        AudioProcessor::Gain {
                            gain_db: current_db,
                        },
                } = &mut node.kind
                else {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "Gain can only be set on a Gain processing node".into(),
                    ));
                };
                *current_db = gain_db;
                self.dirty = true;
            }
            AudioStudioUiAction::SetNoiseGateThreshold {
                node_id,
                threshold_db,
            } => {
                if !threshold_db.is_finite() || !(-80.0..=0.0).contains(&threshold_db) {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "Noise gate threshold must be between -80 and 0 dBFS".into(),
                    ));
                }
                let node = self
                    .selected_graph_mut()
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
                let AudioNodeKind::Processing {
                    processor:
                        AudioProcessor::NoiseGate {
                            threshold_db: current,
                        },
                } = &mut node.kind
                else {
                    return Err(AudioStudioControllerError::InvalidEdit(
                        "Select a noise gate to adjust its threshold".into(),
                    ));
                };
                *current = threshold_db;
                self.dirty = true;
            }
            AudioStudioUiAction::MoveNode { node_id, position } => {
                let node = self
                    .selected_graph_mut()
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == node_id)
                    .ok_or(AudioStudioControllerError::NodeNotFound(node_id))?;
                node.position = position;
                self.dirty = true;
            }
            AudioStudioUiAction::Connect { from, to } => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && self.selected_graph().reaches_asr_sink(&to.node_id)
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                self.connect(from, to)?
            }
            AudioStudioUiAction::Rewire { link_id, from, to } => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && let Some(link) = self.selected_graph().links.iter().find(|l| l.id == link_id)
                    && (self.selected_graph().is_asr_pipeline_link(link)
                        || self.selected_graph().reaches_asr_sink(&to.node_id))
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                self.rewire(link_id, from, to)?
            }
            AudioStudioUiAction::DeleteLink(link_id) => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && let Some(link) = self.selected_graph().links.iter().find(|l| l.id == link_id)
                    && self.selected_graph().is_asr_pipeline_link(link)
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                self.delete_link(link_id)?
            }
            AudioStudioUiAction::SetLinkEnabled { link_id, enabled } => {
                if let Some(owner) = &host_audio.translation_workflow_locked_by
                    && let Some(link) = self.selected_graph().links.iter().find(|l| l.id == link_id)
                    && self.selected_graph().is_asr_pipeline_link(link)
                {
                    return Err(AudioStudioControllerError::InvalidEdit(format!(
                        "the translation pipeline is currently in use by {owner}"
                    )));
                }
                self.set_link_enabled(link_id, enabled)?
            }
            AudioStudioUiAction::SetDeviceDefaults(defaults) => {
                self.settings.device_defaults = defaults;
                self.dirty = true;
            }
            AudioStudioUiAction::Save => self.save()?,
            AudioStudioUiAction::DiscoverApplications => {
                return Ok(vec![AudioStudioHostAction::DiscoverApplications]);
            }
            AudioStudioUiAction::ChooseMedia(node_id) => {
                return self.choose_media(node_id);
            }
            AudioStudioUiAction::EnqueueTts { node_id, text } => {
                return self.enqueue_tts(node_id, text);
            }
        }
        let routing_changed = routing_before.as_ref().is_some_and(|(graph, defaults)| {
            !same_routing_configuration(graph, &self.settings.graph)
                || defaults != &self.settings.device_defaults
        });
        if routing_changed {
            self.graph_revision = self.graph_revision.saturating_add(1);
            self.failed_graph_revision = None;
        }
        let mut host_actions = Vec::new();
        if may_change_asr_input && routing_changed {
            let graph = resolve_graph_devices(
                self.selected_graph(),
                &self.settings.device_defaults,
                host_audio,
            );
            let active_asr_count = graph
                .nodes
                .iter()
                .filter(|node| {
                    !node.bypassed
                        && matches!(node.kind, AudioNodeKind::AsrTap)
                        && graph.has_enabled_source_path(&node.id)
                })
                .count();
            if active_asr_count == 1 {
                host_actions.push(AudioStudioHostAction::ConfigureAsrInput { graph });
                if !host_audio.translation_workflow_running {
                    host_actions.push(AudioStudioHostAction::SetTranslationWorkflowEnabled(true));
                }
            } else if active_asr_count == 0 && host_audio.translation_workflow_running {
                host_actions.push(AudioStudioHostAction::SetTranslationWorkflowEnabled(false));
            }
        }
        if routing_changed {
            host_actions.extend(self.reconcile_live_routing(host_audio)?);
        }
        Ok(host_actions)
    }

    pub fn handle_host_event(&mut self, event: AudioStudioHostEvent) {
        match event {
            AudioStudioHostEvent::Activated { request_id } => {
                if let AudioStudioLifecycle::Activating {
                    request_id: expected,
                    graph_id,
                } = &self.lifecycle
                    && *expected == request_id
                {
                    self.lifecycle = AudioStudioLifecycle::Active {
                        request_id,
                        graph_id: graph_id.clone(),
                    };
                    self.active_graph_revision = self.pending_graph_revision.take();
                    self.failed_graph_revision = None;
                }
            }
            AudioStudioHostEvent::ActivationFailed {
                request_id,
                message,
            } => {
                if matches!(self.lifecycle, AudioStudioLifecycle::Activating { request_id: expected, .. } if expected == request_id)
                {
                    self.pending_graph_revision = None;
                    self.failed_graph_revision = Some(self.graph_revision);
                    self.record_lifecycle_error(message);
                }
            }
            AudioStudioHostEvent::Deactivated { request_id } => {
                if matches!(self.lifecycle, AudioStudioLifecycle::Deactivating { request_id: expected, .. } if expected == request_id)
                {
                    self.lifecycle = AudioStudioLifecycle::Inactive;
                    self.active_graph_revision = None;
                }
            }
            AudioStudioHostEvent::DeactivationFailed {
                request_id,
                message,
            } => {
                if matches!(self.lifecycle, AudioStudioLifecycle::Deactivating { request_id: expected, .. } if expected == request_id)
                {
                    self.record_lifecycle_error(message);
                }
            }
            AudioStudioHostEvent::MediaSelected {
                graph_id,
                node_id,
                source,
            } => {
                if self.settings.graph.id == graph_id
                    && let Some(node) = self
                        .settings
                        .graph
                        .nodes
                        .iter_mut()
                        .find(|n| n.id == node_id)
                    && let AudioNodeKind::Media { source: value, .. } = &mut node.kind
                {
                    *value = Some(source);
                    self.dirty = true;
                    self.graph_revision = self.graph_revision.saturating_add(1);
                    self.failed_graph_revision = None;
                }
            }
        }
    }

    /// No idle worker is owned by Audio Studio. The returned action asks the
    /// host to tear down the only active route during application shutdown.
    #[allow(dead_code)]
    pub fn shutdown(&mut self) -> Option<AudioStudioHostAction> {
        let (request_id, graph_id) = match &self.lifecycle {
            AudioStudioLifecycle::Activating {
                request_id,
                graph_id,
            }
            | AudioStudioLifecycle::Active {
                request_id,
                graph_id,
            } => (*request_id, graph_id.clone()),
            _ => return None,
        };
        self.lifecycle = AudioStudioLifecycle::Deactivating {
            request_id,
            graph_id,
        };
        Some(AudioStudioHostAction::DeactivateGraph { request_id })
    }

    pub fn save(&mut self) -> Result<(), AudioStudioPersistenceError> {
        self.settings.normalize();
        self.repository.save(&self.settings)?;
        self.dirty = false;
        Ok(())
    }

    /// Makes rendered output a direct consequence of the enabled graph. A
    /// complete path to a monitor/app-microphone sink runs automatically; the
    /// last such path being disconnected or switched off stops all routes.
    pub fn reconcile_live_routing(
        &mut self,
        host: &HostAudioSnapshot,
    ) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        if matches!(
            self.lifecycle,
            AudioStudioLifecycle::Activating { .. } | AudioStudioLifecycle::Deactivating { .. }
        ) {
            return Ok(Vec::new());
        }

        let graph =
            resolve_graph_devices(self.selected_graph(), &self.settings.device_defaults, host);
        let has_render_route = has_enabled_render_route(&graph);
        let validation = validate_for_host(&graph, host);
        if !has_render_route || !validation.is_valid() {
            return match self.lifecycle {
                AudioStudioLifecycle::Active { .. } => self.deactivate(),
                AudioStudioLifecycle::Error { .. } if !has_render_route => {
                    self.lifecycle = AudioStudioLifecycle::Inactive;
                    self.last_error = None;
                    self.failed_graph_revision = None;
                    Ok(Vec::new())
                }
                _ => Ok(Vec::new()),
            };
        }

        if self.failed_graph_revision == Some(self.graph_revision) {
            return Ok(Vec::new());
        }
        match self.lifecycle {
            AudioStudioLifecycle::Active { .. } if self.live_routing_matches_current_graph() => {
                Ok(Vec::new())
            }
            AudioStudioLifecycle::Active { .. } => self.activate(host, true),
            AudioStudioLifecycle::Inactive | AudioStudioLifecycle::Error { .. } => {
                self.activate(host, false)
            }
            AudioStudioLifecycle::Activating { .. } | AudioStudioLifecycle::Deactivating { .. } => {
                Ok(Vec::new())
            }
        }
    }

    fn live_routing_matches_current_graph(&self) -> bool {
        self.active_graph_revision == Some(self.graph_revision)
    }

    /// Atomically synchronizes the translation page's ASR source selection
    /// into the graph-owned input state. Only active sources that can reach
    /// the graph's single active ASR node are eligible, so unrelated BGM and
    /// game-microphone branches are never rewritten.
    pub fn sync_translation_input(
        &mut self,
        mode: AsrInputMode,
        microphone_device_id: Option<DeviceId>,
        system_capture: SystemAudioCapture,
    ) -> Result<(), AudioStudioControllerError> {
        let mut next = self.settings.clone();
        let graph = &mut next.graph;
        let inputs = recognition_inputs(graph).map_err(AudioStudioControllerError::InvalidEdit)?;
        let microphone_inputs = inputs
            .iter()
            .filter(|input| input.kind == RecognitionInputKind::Microphone)
            .collect::<Vec<_>>();
        let system_inputs = inputs
            .iter()
            .filter(|input| input.kind == RecognitionInputKind::SystemAudio)
            .collect::<Vec<_>>();
        if microphone_inputs.len() > 1 || system_inputs.len() > 1 {
            return Err(AudioStudioControllerError::InvalidEdit(
                "translation input sync requires one independently switchable mixer input per source type"
                    .into(),
            ));
        }
        if mode.requires_microphone() && microphone_inputs.is_empty() {
            return Err(AudioStudioControllerError::InvalidEdit(
                "the recognition mixer has no independently switchable microphone input".into(),
            ));
        }
        if mode.requires_system_audio() && system_inputs.is_empty() {
            return Err(AudioStudioControllerError::InvalidEdit(
                "the recognition mixer has no independently switchable system-audio input".into(),
            ));
        }

        let microphone_id = microphone_inputs
            .first()
            .map(|input| input.source_node_id.clone());
        let system_audio_id = system_inputs
            .first()
            .map(|input| input.source_node_id.clone());
        let input_states = inputs
            .iter()
            .map(|input| {
                let enabled = match input.kind {
                    RecognitionInputKind::Microphone => mode.requires_microphone(),
                    RecognitionInputKind::SystemAudio => mode.requires_system_audio(),
                };
                (input.link_id.clone(), enabled)
            })
            .collect::<Vec<_>>();
        for (link_id, enabled) in input_states {
            graph
                .links
                .iter_mut()
                .find(|link| link.id == link_id)
                .expect("the recognition input link was collected from this graph")
                .enabled = enabled;
        }

        if let Some(microphone_id) = microphone_id.as_ref() {
            let microphone = graph
                .nodes
                .iter_mut()
                .find(|node| &node.id == microphone_id)
                .expect("the upstream microphone was collected from this graph");
            let AudioNodeKind::Microphone { device_id } = &mut microphone.kind else {
                unreachable!("the upstream microphone kind was checked above");
            };
            *device_id = microphone_device_id;
        }
        if let Some(system_audio_id) = system_audio_id.as_ref() {
            let system_audio = graph
                .nodes
                .iter_mut()
                .find(|node| &node.id == system_audio_id)
                .expect("the upstream system-audio source was collected from this graph");
            let AudioNodeKind::SystemAudio { capture } = &mut system_audio.kind else {
                unreachable!("the upstream system-audio kind was checked above");
            };
            *capture = synchronized_system_capture(capture, system_capture);
        }

        if next == self.settings {
            return Ok(());
        }
        self.repository.save(&next)?;
        self.settings = next;
        self.graph_revision = self.graph_revision.saturating_add(1);
        self.dirty = false;
        Ok(())
    }

    /// Synchronizes the active translation running state to incoming links of ASR (AsrTap) nodes.
    pub fn sync_translation_workflow_running(
        &mut self,
        is_running: bool,
    ) -> Result<(), AudioStudioControllerError> {
        let mut next = self.settings.clone();
        let graph = &mut next.graph;
        let asr_nodes = graph
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, AudioNodeKind::AsrTap))
            .map(|node| node.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if asr_nodes.is_empty() {
            return Ok(());
        }
        let mut changed = false;
        for link in &mut graph.links {
            if asr_nodes.contains(&link.to.node_id) {
                if link.enabled != is_running {
                    link.enabled = is_running;
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(());
        }
        if next == self.settings {
            return Ok(());
        }
        self.repository.save(&next)?;
        self.settings = next;
        self.graph_revision = self.graph_revision.saturating_add(1);
        self.dirty = false;
        Ok(())
    }

    fn activate(
        &mut self,
        host: &HostAudioSnapshot,
        allow_replace: bool,
    ) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        if matches!(
            self.lifecycle,
            AudioStudioLifecycle::Activating { .. } | AudioStudioLifecycle::Deactivating { .. }
        ) || (!allow_replace && matches!(self.lifecycle, AudioStudioLifecycle::Active { .. }))
        {
            return Err(AudioStudioControllerError::LifecycleBusy);
        }
        let graph =
            resolve_graph_devices(self.selected_graph(), &self.settings.device_defaults, host);
        let validation = validate_for_host(&graph, host);
        if !validation.is_valid() {
            return Err(AudioStudioControllerError::InvalidGraph(validation));
        }
        let request_id = self.next_id();
        self.lifecycle = AudioStudioLifecycle::Activating {
            request_id,
            graph_id: graph.id.clone(),
        };
        self.pending_graph_revision = Some(self.graph_revision);
        self.last_error = None;
        Ok(vec![AudioStudioHostAction::ActivateGraph {
            request_id,
            graph,
        }])
    }

    fn deactivate(&mut self) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        let (request_id, graph_id) = match &self.lifecycle {
            AudioStudioLifecycle::Active {
                request_id,
                graph_id,
            }
            | AudioStudioLifecycle::Activating {
                request_id,
                graph_id,
            } => (*request_id, graph_id.clone()),
            AudioStudioLifecycle::Inactive | AudioStudioLifecycle::Error { .. } => {
                return Ok(Vec::new());
            }
            AudioStudioLifecycle::Deactivating { .. } => {
                return Err(AudioStudioControllerError::LifecycleBusy);
            }
        };
        self.lifecycle = AudioStudioLifecycle::Deactivating {
            request_id,
            graph_id,
        };
        Ok(vec![AudioStudioHostAction::DeactivateGraph { request_id }])
    }

    fn connect(
        &mut self,
        from: GraphEndpoint,
        mut to: GraphEndpoint,
    ) -> Result<(), AudioStudioControllerError> {
        let link_id = self.next_unique_link_id()?;
        let graph = self.selected_graph_mut();
        assign_dynamic_mixer_port(graph, &mut to);
        let link = AudioLink {
            id: link_id,
            from,
            to,
            enabled: true,
        };
        graph.links.push(link.clone());
        let validation = graph.validate();
        if validation.issues.iter().any(|issue| {
            issue.link_id.as_ref() == Some(&link.id)
                || issue.code == GraphIssueCode::Cycle
                || issue.code == GraphIssueCode::InputAlreadyConnected
        }) {
            graph.links.pop();
            return Err(AudioStudioControllerError::InvalidConnection(validation));
        }
        self.dirty = true;
        Ok(())
    }

    fn delete_link(&mut self, link_id: LinkId) -> Result<(), AudioStudioControllerError> {
        let graph = self.selected_graph_mut();
        let old_len = graph.links.len();
        graph.links.retain(|link| link.id != link_id);
        if graph.links.len() == old_len {
            return Err(AudioStudioControllerError::LinkNotFound(link_id));
        }
        self.dirty = true;
        Ok(())
    }

    fn set_link_enabled(
        &mut self,
        link_id: LinkId,
        enabled: bool,
    ) -> Result<(), AudioStudioControllerError> {
        let graph = self.selected_graph_mut();
        let link = graph
            .links
            .iter_mut()
            .find(|link| link.id == link_id)
            .ok_or_else(|| AudioStudioControllerError::LinkNotFound(link_id.clone()))?;
        if link.enabled != enabled {
            let previous = link.enabled;
            link.enabled = enabled;
            let validation = graph.validate();
            if validation
                .issues
                .iter()
                .any(|issue| issue.code == GraphIssueCode::Cycle)
            {
                graph
                    .links
                    .iter_mut()
                    .find(|link| link.id == link_id)
                    .expect("the toggled link still belongs to this graph")
                    .enabled = previous;
                return Err(AudioStudioControllerError::InvalidConnection(validation));
            }
            self.dirty = true;
        }
        Ok(())
    }

    fn rewire(
        &mut self,
        link_id: LinkId,
        from: GraphEndpoint,
        mut to: GraphEndpoint,
    ) -> Result<(), AudioStudioControllerError> {
        let next_id = self.next_unique_link_id()?;
        let graph = self.selected_graph_mut();
        let Some(index) = graph.links.iter().position(|link| link.id == link_id) else {
            return Err(AudioStudioControllerError::LinkNotFound(link_id));
        };
        let previous = graph.links.remove(index);
        assign_dynamic_mixer_port(graph, &mut to);
        let replacement = AudioLink {
            id: next_id,
            from,
            to,
            enabled: previous.enabled,
        };
        graph.links.push(replacement.clone());
        let validation = graph.validate();
        if validation.issues.iter().any(|issue| {
            issue.link_id.as_ref() == Some(&replacement.id)
                || issue.code == GraphIssueCode::Cycle
                || issue.code == GraphIssueCode::InputAlreadyConnected
        }) {
            graph.links.pop();
            graph.links.insert(index, previous);
            return Err(AudioStudioControllerError::InvalidConnection(validation));
        }
        self.dirty = true;
        Ok(())
    }

    fn remove_node(&mut self, node_id: NodeId) -> Result<(), AudioStudioControllerError> {
        let graph = self.selected_graph_mut();
        let old_len = graph.nodes.len();
        graph.nodes.retain(|node| node.id != node_id);
        if graph.nodes.len() == old_len {
            return Err(AudioStudioControllerError::NodeNotFound(node_id));
        }
        graph
            .links
            .retain(|link| link.from.node_id != node_id && link.to.node_id != node_id);
        self.dirty = true;
        Ok(())
    }

    fn choose_media(
        &self,
        node_id: NodeId,
    ) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        let graph = self.selected_graph();
        let node = graph
            .node(&node_id)
            .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
        if !matches!(node.kind, AudioNodeKind::Media { .. }) {
            return Err(AudioStudioControllerError::InvalidEdit(
                "media can only be selected for a media node".into(),
            ));
        }
        Ok(vec![AudioStudioHostAction::ChooseMedia {
            graph_id: graph.id.clone(),
            node_id,
        }])
    }

    fn enqueue_tts(
        &self,
        node_id: NodeId,
        text: String,
    ) -> Result<Vec<AudioStudioHostAction>, AudioStudioControllerError> {
        if text.trim().is_empty() {
            return Err(AudioStudioControllerError::InvalidEdit(
                "TTS text cannot be empty".into(),
            ));
        }
        let graph = self.selected_graph();
        let node = graph
            .node(&node_id)
            .ok_or_else(|| AudioStudioControllerError::NodeNotFound(node_id.clone()))?;
        if !matches!(node.kind, AudioNodeKind::TextToSpeech) {
            return Err(AudioStudioControllerError::InvalidEdit(
                "TTS text can only be sent to a TTS node".into(),
            ));
        }
        if !matches!(self.lifecycle, AudioStudioLifecycle::Active { ref graph_id, .. } if graph_id == &graph.id)
        {
            return Err(AudioStudioControllerError::GraphNotActive);
        }
        Ok(vec![AudioStudioHostAction::EnqueueTts {
            graph_id: graph.id.clone(),
            node_id,
            text,
        }])
    }

    fn selected_graph(&self) -> &AudioGraph {
        &self.settings.graph
    }

    fn selected_graph_mut(&mut self) -> &mut AudioGraph {
        &mut self.settings.graph
    }

    fn record_lifecycle_error(&mut self, message: String) {
        let graph_id = self.lifecycle.graph_id().cloned();
        self.last_error = Some(message.clone());
        self.lifecycle = AudioStudioLifecycle::Error { graph_id, message };
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        id
    }

    fn next_unique_link_id(&mut self) -> Result<LinkId, AudioStudioControllerError> {
        loop {
            let candidate = LinkId::new(format!("link-{}", self.next_id()));
            if !self
                .selected_graph()
                .links
                .iter()
                .any(|link| link.id == candidate)
            {
                return Ok(candidate);
            }
        }
    }
}

fn assign_dynamic_mixer_port(graph: &AudioGraph, endpoint: &mut GraphEndpoint) {
    let is_mixer = graph
        .node(&endpoint.node_id)
        .is_some_and(|node| matches!(node.kind, AudioNodeKind::Mixer));
    if !is_mixer || endpoint.port_id.0 != super::graph::PortId::INPUT {
        return;
    }
    let used = graph
        .links
        .iter()
        .filter(|link| link.to.node_id == endpoint.node_id)
        .filter_map(|link| link.to.port_id.mixer_input_index())
        .collect::<std::collections::HashSet<_>>();
    let index = (0..).find(|index| !used.contains(index)).unwrap();
    endpoint.port_id = super::graph::PortId::mixer_input(index);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecognitionInputKind {
    Microphone,
    SystemAudio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecognitionInput {
    link_id: LinkId,
    kind: RecognitionInputKind,
    source_node_id: NodeId,
}

fn recognition_inputs(graph: &AudioGraph) -> Result<Vec<RecognitionInput>, String> {
    let active_asrs = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap))
        .collect::<Vec<_>>();
    let [asr] = active_asrs.as_slice() else {
        return Err("translation input sync requires exactly one active ASR node".into());
    };
    let asr_inputs = graph
        .links
        .iter()
        .filter(|link| link.to.node_id == asr.id)
        .collect::<Vec<_>>();
    let [mixer_to_asr] = asr_inputs.as_slice() else {
        return Err("the ASR node must have exactly one recognition-mixer input".into());
    };
    let mixer = graph
        .node(&mixer_to_asr.from.node_id)
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::Mixer))
        .ok_or_else(|| "ASR input selection requires a dedicated recognition mixer".to_owned())?;

    let mut inputs = Vec::new();
    for link in graph
        .links
        .iter()
        .filter(|link| link.to.node_id == mixer.id)
    {
        let (kind, source_node_id) = classify_recognition_branch(graph, &link.from.node_id)?;
        inputs.push(RecognitionInput {
            link_id: link.id.clone(),
            kind,
            source_node_id,
        });
    }
    if inputs.is_empty() {
        return Err("the recognition mixer has no configurable input connections".into());
    }
    Ok(inputs)
}

fn classify_recognition_branch(
    graph: &AudioGraph,
    start: &NodeId,
) -> Result<(RecognitionInputKind, NodeId), String> {
    let mut queue = VecDeque::from([start.clone()]);
    let mut visited = std::collections::HashSet::new();
    let mut sources = Vec::new();
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let node = graph
            .node(&current)
            .ok_or_else(|| format!("recognition input references missing node {}", current.0))?;
        if node.bypassed {
            continue;
        }
        if node.kind.is_source() {
            sources.push((node.id.clone(), &node.kind));
            continue;
        }
        queue.extend(
            graph
                .links
                .iter()
                .filter(|link| link.to.node_id == current)
                .map(|link| link.from.node_id.clone()),
        );
    }
    let [(source_node_id, source_kind)] = sources.as_slice() else {
        return Err(format!(
            "recognition mixer input from {} must resolve to exactly one microphone or system-audio source",
            start.0
        ));
    };
    let kind = match source_kind {
        AudioNodeKind::Microphone { .. } => RecognitionInputKind::Microphone,
        AudioNodeKind::SystemAudio { .. } => RecognitionInputKind::SystemAudio,
        _ => {
            return Err(format!(
                "recognition mixer input from {} is not a microphone or system-audio source",
                start.0
            ));
        }
    };
    Ok((kind, source_node_id.clone()))
}

/// Derives the simplified Translation-page selection from the enabled input
/// sockets of the graph's dedicated recognition mixer.
pub fn derive_asr_input_mode(graph: &AudioGraph) -> Result<AsrInputMode, String> {
    let inputs = recognition_inputs(graph)?;
    let enabled = |kind| {
        inputs.iter().any(|input| {
            input.kind == kind
                && graph
                    .links
                    .iter()
                    .find(|link| link.id == input.link_id)
                    .is_some_and(|link| link.enabled)
        })
    };
    match (
        enabled(RecognitionInputKind::Microphone),
        enabled(RecognitionInputKind::SystemAudio),
    ) {
        (true, true) => Ok(AsrInputMode::Both),
        (true, false) => Ok(AsrInputMode::Microphone),
        (false, true) => Ok(AsrInputMode::SystemAudio),
        (false, false) => Err("enable at least one recognition-mixer input".into()),
    }
}

fn same_routing_configuration(left: &AudioGraph, right: &AudioGraph) -> bool {
    left.format_version == right.format_version
        && left.id == right.id
        && left.audio == right.audio
        && left.links == right.links
        && left.nodes.len() == right.nodes.len()
        && left.nodes.iter().zip(&right.nodes).all(|(left, right)| {
            left.id == right.id && left.bypassed == right.bypassed && left.kind == right.kind
        })
}

fn synchronized_system_capture(
    current: &SystemAudioCapture,
    requested: SystemAudioCapture,
) -> SystemAudioCapture {
    match requested {
        SystemAudioCapture::Endpoint { device_id, .. } => SystemAudioCapture::Endpoint {
            device_id,
            capture_policy: match current {
                SystemAudioCapture::Endpoint { capture_policy, .. } => *capture_policy,
                SystemAudioCapture::Application { .. } => SystemCapturePolicy::SuppressDuringOwnTts,
            },
        },
        SystemAudioCapture::Application { application, .. } => SystemAudioCapture::Application {
            application,
            resolved_process_id: None,
        },
    }
}

fn resolve_graph_devices(
    graph: &AudioGraph,
    defaults: &DeviceDefaults,
    host: &HostAudioSnapshot,
) -> AudioGraph {
    let mut resolved = graph.clone();
    for node in &mut resolved.nodes {
        let (role, selected, configured_default) = match &mut node.kind {
            AudioNodeKind::Microphone { device_id } => (
                AudioDeviceRole::MicrophoneCapture,
                device_id,
                defaults.microphone_device_id.as_ref(),
            ),
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Endpoint { device_id, .. },
            } => (
                AudioDeviceRole::SystemAudioCapture,
                device_id,
                defaults.system_audio_device_id.as_ref(),
            ),
            AudioNodeKind::SystemAudio {
                capture:
                    SystemAudioCapture::Application {
                        application,
                        resolved_process_id,
                    },
            } => {
                *resolved_process_id = application.as_ref().and_then(|selection| {
                    host.applications
                        .iter()
                        .find(|candidate| candidate.id == selection.id)
                        .map(|candidate| candidate.process_id)
                });
                continue;
            }
            AudioNodeKind::MonitorOutput { device_id } => (
                AudioDeviceRole::MonitorRender,
                device_id,
                defaults.monitor_device_id.as_ref(),
            ),
            AudioNodeKind::GameMicrophoneOutput { device_id, .. } => (
                AudioDeviceRole::GameMicrophoneSink,
                device_id,
                defaults.game_microphone_device_id.as_ref(),
            ),
            _ => continue,
        };
        // An empty host device ID is the executor's representation of the OS
        // default endpoint. Keep that state as `None` in the graph instead of
        // turning it into an explicitly selected (and invalid) device.
        if selected
            .as_ref()
            .is_some_and(|device| device.0.trim().is_empty())
        {
            *selected = None;
        }
        if selected.is_none() {
            *selected = configured_default
                .filter(|configured| !configured.0.trim().is_empty())
                .filter(|configured| {
                    host.devices
                        .iter()
                        .any(|device| device.role == role && &device.id == *configured)
                })
                .cloned()
                .or_else(|| {
                    host.automatic_device(role)
                        .filter(|device| !device.id.0.trim().is_empty())
                        .map(|device| device.id.clone())
                });
        }
        if let AudioNodeKind::GameMicrophoneOutput {
            device_id,
            voicemeeter_bus,
            ..
        } = &mut node.kind
            && voicemeeter_bus.is_none()
            && device_id.as_ref().is_some_and(|selected| {
                host.devices.iter().any(|device| {
                    device.role == AudioDeviceRole::GameMicrophoneSink
                        && &device.id == selected
                        && device.voicemeeter_strip_index.is_some()
                })
            })
        {
            *voicemeeter_bus = Some(VoiceMeeterBus::B1);
        }
    }
    resolved
}

fn path_between(
    graph: &AudioGraph,
    start: &NodeId,
    end: &NodeId,
) -> Option<(Vec<NodeId>, Vec<LinkId>)> {
    let active = |node_id: &NodeId| graph.node(node_id).is_some_and(|node| !node.bypassed);
    if !active(start) || !active(end) {
        return None;
    }
    let mut queue = VecDeque::from([(start.clone(), vec![start.clone()], Vec::new())]);
    let mut visited = std::collections::HashSet::new();
    while let Some((current, nodes, links)) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        if &current == end {
            return Some((nodes, links));
        }
        for link in graph
            .links
            .iter()
            .filter(|link| link.enabled && link.from.node_id == current && active(&link.to.node_id))
        {
            let mut next_nodes = nodes.clone();
            next_nodes.push(link.to.node_id.clone());
            let mut next_links = links.clone();
            next_links.push(link.id.clone());
            queue.push_back((link.to.node_id.clone(), next_nodes, next_links));
        }
    }
    None
}

fn has_enabled_render_route(graph: &AudioGraph) -> bool {
    graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && node.kind.is_source())
        .any(|source| {
            graph.nodes.iter().any(|sink| {
                !sink.bypassed
                    && matches!(
                        sink.kind,
                        AudioNodeKind::MonitorOutput { .. }
                            | AudioNodeKind::GameMicrophoneOutput { .. }
                    )
                    && path_between(graph, &source.id, &sink.id).is_some()
            })
        })
}

pub fn analyze_route_risks(graph: &AudioGraph) -> RouteRiskReport {
    fn risk(
        severity: RouteRiskSeverity,
        code: RouteRiskCode,
        summary: impl Into<String>,
        detail: impl Into<String>,
        remediation: impl Into<String>,
        path: (Vec<NodeId>, Vec<LinkId>),
    ) -> RouteRisk {
        RouteRisk {
            severity,
            code,
            summary: summary.into(),
            detail: detail.into(),
            remediation: remediation.into(),
            node_ids: path.0,
            link_ids: path.1,
        }
    }

    let mut report = RouteRiskReport::default();
    let sinks = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && node.kind.is_sink())
        .collect::<Vec<_>>();
    let microphones = graph
        .nodes
        .iter()
        .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::Microphone { .. }))
        .collect::<Vec<_>>();
    for source in graph.nodes.iter().filter(|node| !node.bypassed) {
        match &source.kind {
            AudioNodeKind::SystemAudio {
                capture:
                    SystemAudioCapture::Endpoint {
                        device_id,
                        capture_policy,
                    },
            } => {
                for sink in &sinks {
                    let Some(path) = path_between(graph, &source.id, &sink.id) else {
                        continue;
                    };
                    if matches!(
                        sink.kind,
                        AudioNodeKind::MonitorOutput { .. }
                            | AudioNodeKind::GameMicrophoneOutput { .. }
                    ) {
                        let sink_device = sink.kind.selected_device();
                        let same_endpoint = match (device_id.as_ref(), sink_device) {
                            (Some(left), Some(right)) => left == right,
                            (None, None) => {
                                matches!(sink.kind, AudioNodeKind::MonitorOutput { .. })
                            }
                            _ => false,
                        };
                        if same_endpoint {
                            report.risks.push(risk(
                                RouteRiskSeverity::Blocking,
                                RouteRiskCode::DirectEndpointFeedback,
                                "Direct digital feedback loop",
                                format!(
                                    "{} captures the same output endpoint that {} renders into.",
                                    source.label, sink.label
                                ),
                                "Choose a different output, normally a virtual microphone feed, or remove this connection.",
                                path.clone(),
                            ));
                        }
                    }
                    match sink.kind {
                        AudioNodeKind::GameMicrophoneOutput { .. } => {
                            report.risks.push(risk(
                                RouteRiskSeverity::Warning,
                                RouteRiskCode::EndpointPlaybackReshare,
                                "Whole output device will be reshared",
                                format!(
                                    "{} includes every app on the selected output device, so game audio, remote voices, notifications and music can all reach {}.",
                                    source.label, sink.label
                                ),
                                "Use Application audio capture when only one music player should be shared.",
                                path.clone(),
                            ));
                            if microphones.iter().any(|microphone| {
                                path_between(graph, &microphone.id, &sink.id).is_some()
                            }) {
                                report.risks.push(risk(
                                    RouteRiskSeverity::Warning,
                                    RouteRiskCode::AcousticLeakage,
                                    "Speaker sound may re-enter the microphone",
                                    "If the captured output is played through speakers, the physical microphone can pick it up again and create doubling or acoustic feedback.",
                                    "Use headphones and disable Windows Listen/sidetone monitoring for the virtual microphone.",
                                    path.clone(),
                                ));
                            }
                        }
                        AudioNodeKind::AsrTap => {
                            report.risks.push(risk(
                                RouteRiskSeverity::Warning,
                                RouteRiskCode::AsrContentContamination,
                                "ASR receives the entire output mix",
                                "Music, game audio, notifications and multiple speakers on this endpoint can reduce recognition accuracy.",
                                "Capture one application, or keep speech recognition on a dedicated microphone/source.",
                                path.clone(),
                            ));
                            if *capture_policy == SystemCapturePolicy::SuppressDuringOwnTts {
                                report.risks.push(risk(
                                    RouteRiskSeverity::Info,
                                    RouteRiskCode::RecognitionGapDuringTts,
                                    "ASR pauses while XRTranslate TTS plays",
                                    "This prevents TTS self-recognition but also omits overlapping remote speech.",
                                    "Use application/process separation when overlapping speech must be preserved.",
                                    path,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            AudioNodeKind::SystemAudio {
                capture:
                    SystemAudioCapture::Application {
                        application,
                        resolved_process_id,
                    },
            } => {
                if *resolved_process_id == Some(std::process::id()) {
                    for sink in &sinks {
                        if let Some(path) = path_between(graph, &source.id, &sink.id) {
                            report.risks.push(risk(
                                RouteRiskSeverity::Blocking,
                                RouteRiskCode::DirectEndpointFeedback,
                                "XRTranslate cannot capture its own routed audio",
                                "The selected application is XRTranslate itself, so routed output can be captured and amplified repeatedly.",
                                "Choose the music/player application instead of XRTranslate.",
                                path,
                            ));
                        }
                    }
                }
                for sink in sinks
                    .iter()
                    .filter(|sink| matches!(sink.kind, AudioNodeKind::GameMicrophoneOutput { .. }))
                {
                    let Some(path) = path_between(graph, &source.id, &sink.id) else {
                        continue;
                    };
                    let name = application
                        .as_ref()
                        .map(|application| application.display_name.as_str())
                        .unwrap_or("the selected application");
                    report.risks.push(risk(
                        RouteRiskSeverity::Info,
                        RouteRiskCode::ApplicationReturnPath,
                        format!("Only {name} is shared"),
                        "Other applications on the Windows output device are excluded. If the selected application also consumes this virtual microphone, it can still create an application-level return path.",
                        "Select a media player rather than the receiving voice/game application.",
                        path,
                    ));
                    if microphones
                        .iter()
                        .any(|microphone| path_between(graph, &microphone.id, &sink.id).is_some())
                    {
                        let path = path_between(graph, &source.id, &sink.id)
                            .expect("the application path was just resolved");
                        report.risks.push(risk(
                            RouteRiskSeverity::Warning,
                            RouteRiskCode::AcousticLeakage,
                            "Speaker playback may leak into the physical microphone",
                            "Application capture removes digital system-audio cross-talk, but speakers can still be heard acoustically by the microphone.",
                            "Use headphones when mixing application audio with a live microphone.",
                            path,
                        ));
                    }
                }
            }
            AudioNodeKind::Microphone { .. } => {
                for sink in sinks
                    .iter()
                    .filter(|sink| matches!(sink.kind, AudioNodeKind::MonitorOutput { .. }))
                {
                    if let Some(path) = path_between(graph, &source.id, &sink.id) {
                        report.risks.push(risk(
                            RouteRiskSeverity::Warning,
                            RouteRiskCode::AcousticLeakage,
                            "Live microphone monitoring can feed back",
                            "Rendering the physical microphone locally can feed speakers back into the same microphone.",
                            "Use headphones or remove the microphone-to-monitor connection.",
                            path,
                        ));
                    }
                }
            }
            AudioNodeKind::TextToSpeech => {
                for sink in sinks
                    .iter()
                    .filter(|sink| matches!(sink.kind, AudioNodeKind::GameMicrophoneOutput { .. }))
                {
                    if let Some(path) = path_between(graph, &source.id, &sink.id) {
                        report.risks.push(risk(
                            RouteRiskSeverity::Info,
                            RouteRiskCode::TtsSharedToApplication,
                            "TTS is intentionally shared to the app microphone",
                            "Other participants will hear XRTranslate synthesized speech through this route.",
                            "Remove this connection if TTS should only be heard locally.",
                            path,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    report.risks.sort_by_key(|risk| match risk.severity {
        RouteRiskSeverity::Blocking => 0,
        RouteRiskSeverity::Warning => 1,
        RouteRiskSeverity::Info => 2,
    });
    report
}

pub fn validate_for_host(graph: &AudioGraph, host: &HostAudioSnapshot) -> GraphValidation {
    let mut validation = graph.validate();
    if !host.discovery_complete {
        validation.issues.push(GraphValidationIssue::error(
            GraphIssueCode::CapabilityUnavailable,
            "audio devices have not been discovered yet",
        ));
        return validation;
    }
    let render_sinks = graph
        .nodes
        .iter()
        .filter(|node| {
            !node.bypassed
                && matches!(
                    node.kind,
                    AudioNodeKind::MonitorOutput { .. }
                        | AudioNodeKind::GameMicrophoneOutput { .. }
                )
        })
        .count();
    if render_sinks > 1 && !host.capabilities.multiple_render_sinks {
        validation.issues.push(GraphValidationIssue::error(
            GraphIssueCode::CapabilityUnavailable,
            "the host currently supports one rendered output per audio graph",
        ));
    }
    for node in &graph.nodes {
        if node.bypassed || !node_is_on_active_route(graph, &node.id) {
            continue;
        }
        if matches!(
            node.kind,
            AudioNodeKind::GameMicrophoneOutput {
                follow_tts: true,
                ..
            }
        ) {
            let sources = graph
                .nodes
                .iter()
                .filter(|source| {
                    source.kind.is_source() && path_between(graph, &source.id, &node.id).is_some()
                })
                .collect::<Vec<_>>();
            if sources.len() != 2
                || !sources
                    .iter()
                    .any(|source| matches!(source.kind, AudioNodeKind::Microphone { .. }))
                || !sources
                    .iter()
                    .any(|source| matches!(source.kind, AudioNodeKind::TextToSpeech))
            {
                validation.issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::CapabilityUnavailable,
                    "Automatic translator microphone requires only microphone and TTS sources. Use the Translator microphone preset or edit the graph.",
                    &node.id,
                ));
            }
        }
        let (capability, role, internal_output) = match &node.kind {
            AudioNodeKind::Microphone { .. } => (
                host.capabilities.microphone_capture,
                Some(AudioDeviceRole::MicrophoneCapture),
                false,
            ),
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Endpoint { capture_policy, .. },
            } => {
                match capture_policy {
                    SystemCapturePolicy::SuppressDuringOwnTts
                        if !host.capabilities.tts_feedback_suppression =>
                    {
                        validation.issues.push(GraphValidationIssue::for_node(
                            GraphIssueCode::CapabilityUnavailable,
                            "the host cannot suppress system-audio capture during XRTranslate TTS",
                            &node.id,
                        ));
                    }
                    SystemCapturePolicy::ExcludeOwnProcessAudio
                        if !host.capabilities.exclude_own_process_audio =>
                    {
                        validation.issues.push(GraphValidationIssue::for_node(
                            GraphIssueCode::CapabilityUnavailable,
                            "the host cannot exclude XRTranslate TTS from system-audio capture",
                            &node.id,
                        ));
                    }
                    _ => {}
                }
                (
                    host.capabilities.system_audio_capture,
                    Some(AudioDeviceRole::SystemAudioCapture),
                    false,
                )
            }
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Application { application, .. },
            } => {
                match application {
                    None => validation.issues.push(GraphValidationIssue::for_node(
                        GraphIssueCode::ApplicationSelectionRequired,
                        "select an application to capture",
                        &node.id,
                    )),
                    Some(selection)
                        if !host
                            .applications
                            .iter()
                            .any(|application| application.id == selection.id) =>
                    {
                        validation.issues.push(GraphValidationIssue::for_node(
                            GraphIssueCode::ApplicationUnavailable,
                            format!(
                                "{} is not running or has no Windows audio session",
                                selection.display_name
                            ),
                            &node.id,
                        ));
                    }
                    _ => {}
                }
                (host.capabilities.application_audio_capture, None, false)
            }
            AudioNodeKind::TextToSpeech => (host.capabilities.tts_source, None, false),
            AudioNodeKind::Media { .. } => (host.capabilities.media_source, None, false),
            AudioNodeKind::MonitorOutput { .. } => (
                host.capabilities.monitor_output,
                Some(AudioDeviceRole::MonitorRender),
                false,
            ),
            AudioNodeKind::GameMicrophoneOutput {
                device_id,
                voicemeeter_bus,
                ..
            } => {
                if let Some(bus) = voicemeeter_bus {
                    match &host.voicemeeter {
                        None => validation.issues.push(GraphValidationIssue::for_node(
                            GraphIssueCode::CapabilityUnavailable,
                            "VoiceMeeter is not installed; clear the VoiceMeeter bus target or install it",
                            &node.id,
                        )),
                        Some(voicemeeter) if !voicemeeter.supports_bus(*bus) => {
                            validation.issues.push(GraphValidationIssue::for_node(
                                GraphIssueCode::CapabilityUnavailable,
                                format!(
                                    "VoiceMeeter {} is unavailable in the installed edition",
                                    bus.label()
                                ),
                                &node.id,
                            ));
                        }
                        Some(_) => {
                            let has_strip = device_id.as_ref().is_some_and(|device_id| {
                                host.devices.iter().any(|device| {
                                    device.role == AudioDeviceRole::GameMicrophoneSink
                                        && &device.id == device_id
                                        && device.voicemeeter_strip_index.is_some()
                                })
                            });
                            if !has_strip {
                                validation.issues.push(GraphValidationIssue::for_node(
                                    GraphIssueCode::DeviceUnavailable,
                                    "select a VoiceMeeter input endpoint before choosing a VoiceMeeter bus",
                                    &node.id,
                                ));
                            }
                        }
                    }
                }
                (
                    host.capabilities.game_microphone_output,
                    Some(AudioDeviceRole::GameMicrophoneSink),
                    host.capabilities.game_microphone_without_external_driver,
                )
            }
            AudioNodeKind::Mixer | AudioNodeKind::Processing { .. } | AudioNodeKind::AsrTap => {
                (true, None, false)
            }
        };
        if !capability {
            validation.issues.push(GraphValidationIssue::for_node(
                GraphIssueCode::CapabilityUnavailable,
                "this audio-node capability is unavailable on the host",
                &node.id,
            ));
        }
        if let Some(role) = role {
            let selected = node.kind.selected_device();
            let found = host.supports_role(role, selected);
            if !found && !(internal_output && selected.is_none()) {
                let message = if role == AudioDeviceRole::GameMicrophoneSink
                    && selected.is_none()
                    && host
                        .devices
                        .iter()
                        .filter(|device| {
                            device.role == AudioDeviceRole::GameMicrophoneSink
                                && !device.id.0.trim().is_empty()
                        })
                        .count()
                        > 1
                {
                    "select the virtual microphone feed output paired with the target app's microphone input"
                } else {
                    "the selected or default audio device is unavailable"
                };
                validation.issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::DeviceUnavailable,
                    message,
                    &node.id,
                ));
            }
        }
    }
    for risk in analyze_route_risks(graph)
        .risks
        .into_iter()
        .filter(|risk| risk.severity == RouteRiskSeverity::Blocking)
    {
        let mut issue =
            GraphValidationIssue::error(GraphIssueCode::DirectEndpointFeedback, risk.summary);
        issue.node_id = risk.node_ids.first().cloned();
        validation.issues.push(issue);
    }
    validation
}

fn node_is_on_active_route(graph: &AudioGraph, node_id: &NodeId) -> bool {
    let Some(node) = graph.node(node_id) else {
        return false;
    };
    if node.bypassed {
        return false;
    }
    let active_sinks = graph
        .nodes
        .iter()
        .filter(|sink| !sink.bypassed && sink.kind.is_sink())
        .filter(|sink| {
            graph
                .nodes
                .iter()
                .filter(|source| !source.bypassed && source.kind.is_source())
                .any(|source| path_between(graph, &source.id, &sink.id).is_some())
        })
        .collect::<Vec<_>>();
    if node.kind.is_sink() {
        return active_sinks.iter().any(|sink| sink.id == node.id);
    }
    active_sinks
        .iter()
        .any(|sink| path_between(graph, node_id, &sink.id).is_some())
}

#[derive(Debug)]
pub enum AudioStudioControllerError {
    Persistence(AudioStudioPersistenceError),
    NodeNotFound(NodeId),
    LinkNotFound(LinkId),
    LifecycleBusy,
    GraphNotActive,
    InvalidEdit(String),
    InvalidConnection(GraphValidation),
    InvalidGraph(GraphValidation),
}

impl fmt::Display for AudioStudioControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persistence(error) => error.fmt(formatter),
            Self::NodeNotFound(id) => write!(formatter, "audio node {} was not found", id.0),
            Self::LinkNotFound(id) => write!(formatter, "audio link {} was not found", id.0),
            Self::LifecycleBusy => formatter.write_str("the audio graph lifecycle is busy"),
            Self::GraphNotActive => formatter.write_str("the selected audio graph is not active"),
            Self::InvalidEdit(message) => formatter.write_str(message),
            Self::InvalidConnection(validation) => write!(
                formatter,
                "the connection is not valid{}",
                validation_suffix(validation)
            ),
            Self::InvalidGraph(validation) => write!(
                formatter,
                "the audio graph is not ready to activate{}",
                validation_suffix(validation)
            ),
        }
    }
}

fn validation_suffix(validation: &GraphValidation) -> String {
    validation
        .issues
        .first()
        .map(|issue| format!(": {}", issue.message))
        .unwrap_or_default()
}

impl std::error::Error for AudioStudioControllerError {}

impl From<AudioStudioPersistenceError> for AudioStudioControllerError {
    fn from(value: AudioStudioPersistenceError) -> Self {
        Self::Persistence(value)
    }
}
