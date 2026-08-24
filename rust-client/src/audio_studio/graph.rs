use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

pub const AUDIO_GRAPH_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphId(pub String);

impl GraphId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LinkId(pub String);

impl LinkId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationSelection {
    pub id: ApplicationId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PortId(pub String);

impl PortId {
    pub const AUDIO: &'static str = "audio";
    pub const INPUT: &'static str = "input";
    pub const SIDECHAIN: &'static str = "sidechain";

    pub fn audio() -> Self {
        Self(Self::AUDIO.into())
    }

    pub fn input() -> Self {
        Self(Self::INPUT.into())
    }

    pub fn sidechain() -> Self {
        Self(Self::SIDECHAIN.into())
    }

    /// A mixer owns one stable socket per connected input. The legacy
    /// `input` port remains readable, but newly-authored graphs use
    /// `input:<index>` so each connection can be addressed independently.
    pub fn mixer_input(index: usize) -> Self {
        Self(format!("{}:{index}", Self::INPUT))
    }

    pub fn mixer_input_index(&self) -> Option<usize> {
        self.0
            .strip_prefix("input:")
            .and_then(|index| index.parse().ok())
    }

    pub fn is_mixer_input(&self) -> bool {
        self.0 == Self::INPUT || self.mixer_input_index().is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct GraphPosition {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemCapturePolicy {
    AllEndpointAudio,
    /// Pause recognition-facing capture while XRTranslate's own TTS is
    /// playing. This prevents feedback but may omit overlapping remote audio.
    SuppressDuringOwnTts,
    /// True source separation supplied by the host (for example, process
    /// loopback exclusion). This does not intentionally gate other audio.
    ExcludeOwnProcessAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrInputMode {
    Microphone,
    SystemAudio,
    #[default]
    Both,
}

impl AsrInputMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone only",
            Self::SystemAudio => "System audio only",
            Self::Both => "Microphone + system audio",
        }
    }

    pub const fn requires_microphone(self) -> bool {
        matches!(self, Self::Microphone | Self::Both)
    }

    pub const fn requires_system_audio(self) -> bool {
        matches!(self, Self::SystemAudio | Self::Both)
    }
}

impl Default for SystemCapturePolicy {
    fn default() -> Self {
        Self::ExcludeOwnProcessAudio
    }
}

/// A system-audio node deliberately distinguishes an endpoint-wide mix from
/// one application. PID is runtime-only; saved graphs retain the stable
/// executable identity and cached display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SystemAudioCapture {
    Endpoint {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<DeviceId>,
        capture_policy: SystemCapturePolicy,
    },
    Application {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        application: Option<ApplicationSelection>,
        #[serde(skip)]
        resolved_process_id: Option<u32>,
    },
}

/// VoiceMeeter's game-facing virtual output buses. The optional bus target on
/// a sink keeps ordinary render endpoints vendor-neutral; `None` preserves the
/// existing direct-render behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMeeterBus {
    #[default]
    B1,
    B2,
    B3,
}

impl VoiceMeeterBus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::B1 => "B1",
            Self::B2 => "B2",
            Self::B3 => "B3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioProcessor {
    Gain { gain_db: f32 },
    NoiseGate { threshold_db: f32 },
    Compressor { threshold_db: f32, ratio: f32 },
    Limiter { ceiling_db: f32 },
    Ducker { attenuation_db: f32 },
}

impl AudioProcessor {
    fn accepts_sidechain(&self) -> bool {
        matches!(self, Self::Ducker { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioNodeKind {
    Microphone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<DeviceId>,
    },
    SystemAudio {
        capture: SystemAudioCapture,
    },
    TextToSpeech,
    Media {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default)]
        loop_playback: bool,
    },
    Mixer,
    Processing {
        processor: AudioProcessor,
    },
    AsrTap,
    MonitorOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<DeviceId>,
    },
    GameMicrophoneOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<DeviceId>,
        /// When the selected render endpoint feeds a VoiceMeeter input strip,
        /// select which virtual microphone bus receives that strip. `None`
        /// lets host resolution choose B1 for an associated VoiceMeeter endpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voicemeeter_bus: Option<VoiceMeeterBus>,
    },
}

impl AudioNodeKind {
    pub fn is_source(&self) -> bool {
        matches!(
            self,
            Self::Microphone { .. }
                | Self::SystemAudio { .. }
                | Self::TextToSpeech
                | Self::Media { .. }
        )
    }

    pub fn is_sink(&self) -> bool {
        matches!(
            self,
            Self::AsrTap | Self::MonitorOutput { .. } | Self::GameMicrophoneOutput { .. }
        )
    }

    pub fn accepts_input(&self, port: &PortId) -> bool {
        match self {
            Self::Mixer => port.is_mixer_input(),
            Self::AsrTap
            | Self::MonitorOutput { .. }
            | Self::GameMicrophoneOutput { .. } => port.0 == PortId::INPUT,
            Self::Processing { processor } => {
                port.0 == PortId::INPUT
                    || (port.0 == PortId::SIDECHAIN && processor.accepts_sidechain())
            }
            Self::Microphone { .. }
            | Self::SystemAudio { .. }
            | Self::TextToSpeech
            | Self::Media { .. } => false,
        }
    }

    pub fn provides_output(&self, port: &PortId) -> bool {
        !self.is_sink() && port.0 == PortId::AUDIO
    }

    pub fn selected_device(&self) -> Option<&DeviceId> {
        match self {
            Self::Microphone { device_id }
            | Self::MonitorOutput { device_id }
            | Self::GameMicrophoneOutput { device_id, .. } => device_id.as_ref(),
            Self::SystemAudio {
                capture: SystemAudioCapture::Endpoint { device_id, .. },
            } => device_id.as_ref(),
            _ => None,
        }
    }

    pub fn selected_application(&self) -> Option<&ApplicationSelection> {
        match self {
            Self::SystemAudio {
                capture: SystemAudioCapture::Application { application, .. },
            } => application.as_ref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioNode {
    pub id: NodeId,
    pub label: String,
    #[serde(default)]
    pub position: GraphPosition,
    #[serde(default)]
    pub bypassed: bool,
    #[serde(flatten)]
    pub kind: AudioNodeKind,
}

impl AudioNode {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: AudioNodeKind) -> Self {
        Self {
            id: NodeId::new(id),
            label: label.into(),
            position: GraphPosition::default(),
            bypassed: false,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphEndpoint {
    pub node_id: NodeId,
    pub port_id: PortId,
}

impl GraphEndpoint {
    pub fn audio(node_id: impl Into<String>) -> Self {
        Self {
            node_id: NodeId::new(node_id),
            port_id: PortId::audio(),
        }
    }

    pub fn input(node_id: impl Into<String>) -> Self {
        Self {
            node_id: NodeId::new(node_id),
            port_id: PortId::input(),
        }
    }

    pub fn mixer_input(node_id: impl Into<String>, index: usize) -> Self {
        Self {
            node_id: NodeId::new(node_id),
            port_id: PortId::mixer_input(index),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioLink {
    pub id: LinkId,
    pub from: GraphEndpoint,
    pub to: GraphEndpoint,
    #[serde(default = "default_link_enabled")]
    pub enabled: bool,
}

const fn default_link_enabled() -> bool {
    true
}

impl AudioLink {
    pub fn new(
        id: impl Into<String>,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
    ) -> Self {
        Self::new_with_enabled(id, from_node, to_node, true)
    }

    pub fn new_with_enabled(
        id: impl Into<String>,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
        enabled: bool,
    ) -> Self {
        Self {
            id: LinkId::new(id),
            from: GraphEndpoint::audio(from_node),
            to: GraphEndpoint::input(to_node),
            enabled,
        }
    }

    pub fn to_mixer_input(
        id: impl Into<String>,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
        input_index: usize,
    ) -> Self {
        Self {
            id: LinkId::new(id),
            from: GraphEndpoint::audio(from_node),
            to: GraphEndpoint::mixer_input(to_node, input_index),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphAudioSettings {
    #[serde(default = "default_sample_rate")]
    pub sample_rate_hz: u32,
    #[serde(default = "default_buffer_frames")]
    pub buffer_frames: u16,
}

const fn default_sample_rate() -> u32 {
    48_000
}

const fn default_buffer_frames() -> u16 {
    480
}

impl Default for GraphAudioSettings {
    fn default() -> Self {
        Self {
            sample_rate_hz: default_sample_rate(),
            buffer_frames: default_buffer_frames(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioGraph {
    #[serde(default = "default_graph_format_version")]
    pub format_version: u32,
    pub id: GraphId,
    pub name: String,
    #[serde(default)]
    pub audio: GraphAudioSettings,
    #[serde(default)]
    pub nodes: Vec<AudioNode>,
    #[serde(default)]
    pub links: Vec<AudioLink>,
}

const fn default_graph_format_version() -> u32 {
    AUDIO_GRAPH_FORMAT_VERSION
}

impl AudioGraph {
    /// Returns true when an enabled, non-bypassed path reaches `node_id` from
    /// at least one audio source.
    pub fn has_enabled_source_path(&self, node_id: &NodeId) -> bool {
        let mut pending = VecDeque::from([node_id.clone()]);
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop_front() {
            if !visited.insert(current.clone()) {
                continue;
            }
            let Some(node) = self.node(&current).filter(|node| !node.bypassed) else {
                continue;
            };
            if node.kind.is_source() {
                return true;
            }
            pending.extend(
                self.links
                    .iter()
                    .filter(|link| link.enabled && link.to.node_id == current)
                    .map(|link| link.from.node_id.clone()),
            );
        }
        false
    }

    /// Returns true if `node_id` can reach an `AsrTap` sink through downstream connections.
    pub fn reaches_asr_sink(&self, node_id: &NodeId) -> bool {
        let mut pending = VecDeque::from([node_id.clone()]);
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop_front() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(node) = self.node(&current)
                && matches!(node.kind, AudioNodeKind::AsrTap)
            {
                return true;
            }
            pending.extend(
                self.links
                    .iter()
                    .filter(|link| link.from.node_id == current)
                    .map(|link| link.to.node_id.clone()),
            );
        }
        false
    }

    /// Returns true if `link` feeds into or is part of the path leading to an `AsrTap` sink.
    pub fn is_asr_pipeline_link(&self, link: &AudioLink) -> bool {
        self.reaches_asr_sink(&link.to.node_id)
    }

    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            format_version: AUDIO_GRAPH_FORMAT_VERSION,
            id: GraphId::new(id),
            name: name.into(),
            audio: GraphAudioSettings::default(),
            nodes: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn node(&self, id: &NodeId) -> Option<&AudioNode> {
        self.nodes.iter().find(|node| &node.id == id)
    }

    pub fn validate(&self) -> GraphValidation {
        let mut issues = Vec::new();
        if self.format_version != AUDIO_GRAPH_FORMAT_VERSION {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::UnsupportedGraphVersion,
                format!("unsupported audio graph version {}", self.format_version),
            ));
        }
        if self.id.0.trim().is_empty() {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::EmptyId,
                "graph ID cannot be empty",
            ));
        }
        if self.name.trim().is_empty() {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::EmptyName,
                "graph name cannot be empty",
            ));
        }
        if self.audio.sample_rate_hz == 0 || self.audio.buffer_frames == 0 {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::InvalidAudioSettings,
                "sample rate and buffer size must be greater than zero",
            ));
        }

        let mut node_ids = HashSet::new();
        let mut nodes = HashMap::new();
        for node in &self.nodes {
            if node.id.0.trim().is_empty() || node.label.trim().is_empty() {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::EmptyId,
                    "node ID and label cannot be empty",
                    &node.id,
                ));
            }
            if !node_ids.insert(node.id.clone()) {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::DuplicateNodeId,
                    "node ID is used more than once",
                    &node.id,
                ));
            }
            if node
                .kind
                .selected_device()
                .is_some_and(|device| device.0.trim().is_empty())
            {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::EmptyDeviceId,
                    "selected device ID cannot be empty",
                    &node.id,
                ));
            }
            nodes.entry(node.id.clone()).or_insert(node);
        }

        let mut link_ids = HashSet::new();
        let mut endpoints = HashSet::new();
        let mut inbound_counts: HashMap<GraphEndpoint, usize> = HashMap::new();
        let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut valid_links = Vec::new();
        for link in &self.links {
            if link.id.0.trim().is_empty() || !link_ids.insert(link.id.clone()) {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::DuplicateLinkId,
                    "link ID is empty or used more than once",
                    &link.id,
                ));
            }
            if !endpoints.insert((link.from.clone(), link.to.clone())) {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::DuplicateConnection,
                    "the same connection is present more than once",
                    &link.id,
                ));
            }
            if link.from.node_id == link.to.node_id {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::SelfConnection,
                    "a node cannot connect to itself",
                    &link.id,
                ));
                continue;
            }
            let Some(source) = nodes.get(&link.from.node_id) else {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::MissingNode,
                    "connection source node does not exist",
                    &link.id,
                ));
                continue;
            };
            let Some(target) = nodes.get(&link.to.node_id) else {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::MissingNode,
                    "connection target node does not exist",
                    &link.id,
                ));
                continue;
            };
            let output_valid = source.kind.provides_output(&link.from.port_id);
            let input_valid = target.kind.accepts_input(&link.to.port_id);
            if !output_valid || !input_valid {
                issues.push(GraphValidationIssue::for_link(
                    GraphIssueCode::InvalidDirectionOrPort,
                    "connection does not run from a valid output port to a valid input port",
                    &link.id,
                ));
                continue;
            }
            *inbound_counts.entry(link.to.clone()).or_default() += 1;
            if link.enabled {
                adjacency
                    .entry(link.from.node_id.clone())
                    .or_default()
                    .push(link.to.node_id.clone());
                valid_links.push(link);
            }
        }

        for (endpoint, count) in &inbound_counts {
            if *count > 1 {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::InputAlreadyConnected,
                    "an input socket may accept only one connection",
                    &endpoint.node_id,
                ));
            }
        }

        if graph_has_cycle(nodes.keys().cloned(), &adjacency) {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::Cycle,
                "audio graph contains a cycle",
            ));
        }

        let sink_ids: HashSet<_> = self
            .nodes
            .iter()
            .filter(|node| node.kind.is_sink())
            .map(|node| node.id.clone())
            .collect();
        let source_ids: HashSet<_> = self
            .nodes
            .iter()
            .filter(|node| node.kind.is_source())
            .map(|node| node.id.clone())
            .collect();
        if source_ids.is_empty() {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::MissingSource,
                "audio graph needs a microphone, system-audio, TTS, or media source",
            ));
        }
        if sink_ids.is_empty() {
            issues.push(GraphValidationIssue::error(
                GraphIssueCode::MissingSink,
                "audio graph needs an ASR, monitor, or game-microphone output",
            ));
        }
        let mut full_adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for link in &self.links {
            if link.from.node_id != link.to.node_id
                && nodes.contains_key(&link.from.node_id)
                && nodes.contains_key(&link.to.node_id)
            {
                full_adjacency
                    .entry(link.from.node_id.clone())
                    .or_default()
                    .push(link.to.node_id.clone());
            }
        }
        let reached_from_source = reachable_from(source_ids.iter().cloned(), &full_adjacency);
        for sink_id in &sink_ids {
            let has_incoming = self.links.iter().any(|link| &link.to.node_id == sink_id);
            if !has_incoming {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::UnconnectedSink,
                    "output has no incoming audio",
                    sink_id,
                ));
            } else if !reached_from_source.contains(sink_id) {
                issues.push(GraphValidationIssue::for_node(
                    GraphIssueCode::SinkWithoutSource,
                    "output is connected to a branch that does not originate from an audio source",
                    sink_id,
                ));
            }
        }

        for tts in self
            .nodes
            .iter()
            .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::TextToSpeech))
        {
            let reached_from_tts = reachable_from([tts.id.clone()].into_iter(), &full_adjacency);
            for asr in self
                .nodes
                .iter()
                .filter(|node| !node.bypassed && matches!(node.kind, AudioNodeKind::AsrTap))
            {
                if reached_from_tts.contains(&asr.id) {
                    issues.push(GraphValidationIssue::for_node(
                        GraphIssueCode::TtsToAsrFeedback,
                        "TTS cannot be connected to ASR because synthesized speech would be recognized again",
                        &asr.id,
                    ));
                }
            }
        }

        if !sink_ids.is_empty() && !graph_has_cycle(nodes.keys().cloned(), &full_adjacency) {
            let reverse = reverse_adjacency(&full_adjacency);
            let reaches_sink = reachable_from(sink_ids.iter().cloned(), &reverse);
            for node in &self.nodes {
                if !node.kind.is_sink() && !reaches_sink.contains(&node.id) {
                    issues.push(GraphValidationIssue::for_node(
                        GraphIssueCode::DoesNotReachSink,
                        "node does not route to any output",
                        &node.id,
                    ));
                }
            }
        }

        GraphValidation { issues }
    }
}

fn graph_has_cycle(
    nodes: impl Iterator<Item = NodeId>,
    adjacency: &HashMap<NodeId, Vec<NodeId>>,
) -> bool {
    let all_nodes: Vec<_> = nodes.collect();
    let mut indegree: HashMap<NodeId, usize> =
        all_nodes.iter().cloned().map(|node| (node, 0)).collect();
    for targets in adjacency.values() {
        for target in targets {
            *indegree.entry(target.clone()).or_default() += 1;
        }
    }
    let mut queue: VecDeque<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(node, _)| node.clone())
        .collect();
    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(targets) = adjacency.get(&node) {
            for target in targets {
                if let Some(degree) = indegree.get_mut(target) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(target.clone());
                    }
                }
            }
        }
    }
    visited != indegree.len()
}

fn reverse_adjacency(adjacency: &HashMap<NodeId, Vec<NodeId>>) -> HashMap<NodeId, Vec<NodeId>> {
    let mut reverse = HashMap::new();
    for (source, targets) in adjacency {
        for target in targets {
            reverse
                .entry(target.clone())
                .or_insert_with(Vec::new)
                .push(source.clone());
        }
    }
    reverse
}

fn reachable_from(
    starts: impl Iterator<Item = NodeId>,
    adjacency: &HashMap<NodeId, Vec<NodeId>>,
) -> HashSet<NodeId> {
    let mut reached = HashSet::new();
    let mut queue: VecDeque<_> = starts.collect();
    while let Some(node) = queue.pop_front() {
        if !reached.insert(node.clone()) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            queue.extend(next.iter().cloned());
        }
    }
    reached
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphIssueSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphIssueCode {
    UnsupportedGraphVersion,
    EmptyId,
    EmptyName,
    InvalidAudioSettings,
    DuplicateNodeId,
    DuplicateLinkId,
    DuplicateConnection,
    EmptyDeviceId,
    MissingNode,
    InvalidDirectionOrPort,
    SelfConnection,
    InputAlreadyConnected,
    Cycle,
    MissingSource,
    MissingSink,
    UnconnectedSink,
    SinkWithoutSource,
    DoesNotReachSink,
    DeviceUnavailable,
    CapabilityUnavailable,
    ApplicationSelectionRequired,
    ApplicationUnavailable,
    DirectEndpointFeedback,
    TtsToAsrFeedback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphValidationIssue {
    pub severity: GraphIssueSeverity,
    pub code: GraphIssueCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<LinkId>,
}

impl GraphValidationIssue {
    pub fn error(code: GraphIssueCode, message: impl Into<String>) -> Self {
        Self {
            severity: GraphIssueSeverity::Error,
            code,
            message: message.into(),
            node_id: None,
            link_id: None,
        }
    }

    pub(crate) fn for_node(
        code: GraphIssueCode,
        message: impl Into<String>,
        node_id: &NodeId,
    ) -> Self {
        Self {
            node_id: Some(node_id.clone()),
            ..Self::error(code, message)
        }
    }

    fn for_link(code: GraphIssueCode, message: impl Into<String>, link_id: &LinkId) -> Self {
        Self {
            link_id: Some(link_id.clone()),
            ..Self::error(code, message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GraphValidation {
    pub issues: Vec<GraphValidationIssue>,
}

impl GraphValidation {
    pub fn is_valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == GraphIssueSeverity::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_graph() -> AudioGraph {
        let mut graph = AudioGraph::new("simple", "Simple");
        graph.nodes.push(AudioNode::new(
            "mic",
            "Microphone",
            AudioNodeKind::Microphone { device_id: None },
        ));
        graph
            .nodes
            .push(AudioNode::new("asr", "ASR", AudioNodeKind::AsrTap));
        graph.links.push(AudioLink::new("mic-asr", "mic", "asr"));
        graph
    }

    #[test]
    fn valid_source_to_sink_graph_passes() {
        assert!(simple_graph().validate().is_valid());
    }

    #[test]
    fn source_to_source_is_rejected_as_wrong_direction() {
        let mut graph = simple_graph();
        graph.links.clear();
        graph
            .nodes
            .push(AudioNode::new("tts", "TTS", AudioNodeKind::TextToSpeech));
        graph.links.push(AudioLink::new("bad", "mic", "tts"));
        let validation = graph.validate();
        assert!(
            validation
                .issues
                .iter()
                .any(|issue| issue.code == GraphIssueCode::InvalidDirectionOrPort)
        );
    }

    #[test]
    fn cycles_are_rejected() {
        let mut graph = AudioGraph::new("cycle", "Cycle");
        graph.nodes.extend([
            AudioNode::new("a", "A", AudioNodeKind::Mixer),
            AudioNode::new("b", "B", AudioNodeKind::Mixer),
            AudioNode::new("sink", "Sink", AudioNodeKind::AsrTap),
        ]);
        graph.links.extend([
            AudioLink::new("a-b", "a", "b"),
            AudioLink::new("b-a", "b", "a"),
            AudioLink::new("b-sink", "b", "sink"),
        ]);
        assert!(
            graph
                .validate()
                .issues
                .iter()
                .any(|issue| issue.code == GraphIssueCode::Cycle)
        );
    }

    #[test]
    fn every_output_must_be_fed_and_every_branch_must_reach_one() {
        let mut graph = simple_graph();
        graph.nodes.push(AudioNode::new(
            "unused",
            "Unused",
            AudioNodeKind::TextToSpeech,
        ));
        graph.nodes.push(AudioNode::new(
            "monitor",
            "Monitor",
            AudioNodeKind::MonitorOutput { device_id: None },
        ));
        let validation = graph.validate();
        assert!(validation.issues.iter().any(|issue| {
            issue.code == GraphIssueCode::DoesNotReachSink
                && issue.node_id.as_ref() == Some(&NodeId::new("unused"))
        }));
        assert!(validation.issues.iter().any(|issue| {
            issue.code == GraphIssueCode::UnconnectedSink
                && issue.node_id.as_ref() == Some(&NodeId::new("monitor"))
        }));
    }

    #[test]
    fn mixer_accepts_multiple_inputs_but_a_sink_does_not() {
        let mut graph = AudioGraph::new("mix", "Mix");
        graph.nodes.extend([
            AudioNode::new("mic", "Mic", AudioNodeKind::Microphone { device_id: None }),
            AudioNode::new(
                "media",
                "Media",
                AudioNodeKind::Media {
                    source: None,
                    loop_playback: false,
                },
            ),
            AudioNode::new("mixer", "Mixer", AudioNodeKind::Mixer),
            AudioNode::new("sink", "Sink", AudioNodeKind::AsrTap),
        ]);
        graph.links.extend([
            AudioLink::to_mixer_input("mic-mix", "mic", "mixer", 0),
            AudioLink::to_mixer_input("media-mix", "media", "mixer", 1),
            AudioLink::new("mix-sink", "mixer", "sink"),
        ]);
        assert!(graph.validate().is_valid());

        graph.links.push(AudioLink::new("mic-sink", "mic", "sink"));
        assert!(
            graph
                .validate()
                .issues
                .iter()
                .any(|issue| issue.code == GraphIssueCode::InputAlreadyConnected)
        );
    }

    #[test]
    fn processing_only_graph_does_not_count_as_a_sourced_output() {
        let mut graph = AudioGraph::new("no-source", "No source");
        graph.nodes.extend([
            AudioNode::new("mixer", "Mixer", AudioNodeKind::Mixer),
            AudioNode::new("sink", "Sink", AudioNodeKind::AsrTap),
        ]);
        graph
            .links
            .push(AudioLink::new("mixer-sink", "mixer", "sink"));
        let validation = graph.validate();
        assert!(
            validation
                .issues
                .iter()
                .any(|issue| issue.code == GraphIssueCode::MissingSource)
        );
        assert!(validation.issues.iter().any(|issue| {
            issue.code == GraphIssueCode::SinkWithoutSource
                && issue.node_id.as_ref() == Some(&NodeId::new("sink"))
        }));
    }

    #[test]
    fn serde_uses_stable_kind_and_port_names() {
        let graph = simple_graph();
        let mut value = serde_json::to_value(&graph).unwrap();
        assert_eq!(value["format_version"], AUDIO_GRAPH_FORMAT_VERSION);
        assert_eq!(value["nodes"][0]["kind"], "microphone");
        assert_eq!(value["links"][0]["from"]["port_id"], "audio");
        assert_eq!(value["links"][0]["to"]["port_id"], "input");
        assert_eq!(value["links"][0]["enabled"], true);
        assert_eq!(
            serde_json::from_value::<AudioGraph>(value.clone()).unwrap(),
            graph
        );

        value["links"][0].as_object_mut().unwrap().remove("enabled");
        let legacy = serde_json::from_value::<AudioGraph>(value).unwrap();
        assert!(legacy.links[0].enabled);
    }

    #[test]
    fn voicemeeter_bus_round_trips() {
        let selected = AudioNodeKind::GameMicrophoneOutput {
            device_id: Some(DeviceId::new("virtual-input")),
            voicemeeter_bus: Some(VoiceMeeterBus::B2),
        };
        let value = serde_json::to_value(&selected).unwrap();
        assert_eq!(value["voicemeeter_bus"], "b2");
        assert_eq!(
            serde_json::from_value::<AudioNodeKind>(value).unwrap(),
            selected
        );
    }
}
