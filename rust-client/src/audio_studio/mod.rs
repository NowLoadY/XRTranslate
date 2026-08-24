//! Core Audio Studio domain boundary.
//!
//! Audio Studio owns the serializable route graph, presets, controller state,
//! and persistence. Device discovery and audio execution stay behind typed
//! host actions. Like Prompt Studio, this is always-available infrastructure,
//! not an optional plugin.

pub mod controller;
pub mod graph;
pub mod persistence;
pub mod presets;

#[allow(unused_imports)]
pub use controller::{
    AudioDeviceRole, AudioStudioController, AudioStudioControllerError, AudioStudioHostAction,
    AudioStudioHostEvent, AudioStudioLifecycle, AudioStudioSignalLevels, AudioStudioUiAction,
    AudioStudioUiSnapshot, HostAudioApplication, HostAudioCapabilities, HostAudioDevice,
    HostAudioSnapshot, RouteRisk, RouteRiskCode, RouteRiskReport, RouteRiskSeverity,
    VoiceMeeterEdition, VoiceMeeterInputSnapshot, VoiceMeeterSnapshot, VoiceMeeterStripIndex,
    analyze_route_risks, validate_for_host,
};
#[allow(unused_imports)]
pub use graph::{
    AUDIO_GRAPH_FORMAT_VERSION, ApplicationId, ApplicationSelection, AsrInputMode, AudioGraph,
    AudioLink, AudioNode, AudioNodeKind, AudioProcessor, DeviceId, GraphAudioSettings,
    GraphEndpoint, GraphId, GraphIssueCode, GraphIssueSeverity, GraphPosition, GraphValidation,
    GraphValidationIssue, LinkId, NodeId, PortId, SystemAudioCapture, SystemCapturePolicy,
    VoiceMeeterBus,
};
#[allow(unused_imports)]
pub use persistence::{
    AUDIO_STUDIO_SCHEMA_VERSION, AUDIO_STUDIO_SETTINGS_PATH, AudioStudioPersistenceError,
    AudioStudioRepository, AudioStudioSettings, DeviceDefaults, GLOBAL_AUDIO_GRAPH_ID,
};
#[allow(unused_imports)]
pub use presets::{AudioStudioPreset, graph_for_preset};
