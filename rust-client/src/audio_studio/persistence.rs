#[cfg(test)]
use super::test_graphs::{AudioStudioPreset, graph_for_preset};
use super::{
    default_graph::{DEFAULT_AUDIO_GRAPH_ID, default_graph},
    graph::{AudioGraph, AudioNodeKind, DeviceId, GraphId, NodeId, PortId, SystemAudioCapture},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub const AUDIO_STUDIO_SCHEMA_VERSION: u32 = 6;
pub const AUDIO_STUDIO_SETTINGS_PATH: &str = "runtime/audio_studio.json";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone_device_id: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio_device_id: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_device_id: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_microphone_device_id: Option<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStudioSettings {
    #[serde(default)]
    pub device_defaults: DeviceDefaults,
    #[serde(default = "default_graph")]
    pub graph: AudioGraph,
    /// Inactive graphs. The active graph stays in `graph` for host consumers.
    #[serde(default)]
    pub graphs: Vec<AudioGraph>,
}

#[cfg(test)]
fn preset_graph(preset: AudioStudioPreset) -> AudioGraph {
    let mut graph = graph_for_preset(preset);
    graph.id = GraphId::new(DEFAULT_AUDIO_GRAPH_ID);
    graph
}

impl Default for AudioStudioSettings {
    fn default() -> Self {
        Self {
            device_defaults: DeviceDefaults::default(),
            graph: default_graph(),
            graphs: Vec::new(),
        }
    }
}

impl AudioStudioSettings {
    #[cfg(test)]
    pub fn replace_with_preset(&mut self, preset: AudioStudioPreset) {
        self.graph = preset_graph(preset);
    }

    pub fn all_graphs(&self) -> impl Iterator<Item = &AudioGraph> {
        std::iter::once(&self.graph).chain(&self.graphs)
    }

    pub fn select_graph(&mut self, id: &GraphId) -> Result<(), String> {
        if &self.graph.id == id {
            return Ok(());
        }
        let index = self
            .graphs
            .iter()
            .position(|graph| &graph.id == id)
            .ok_or("Graph not found")?;
        std::mem::swap(&mut self.graph, &mut self.graphs[index]);
        Ok(())
    }

    pub fn create_graph(&mut self, duplicate: bool) {
        let number = (1..)
            .find(|number| {
                !self
                    .all_graphs()
                    .any(|graph| graph.id.0 == format!("user-{number}"))
            })
            .unwrap();
        let name = if duplicate {
            format!("{} (copy)", self.graph.name)
        } else {
            format!("Graph {number}")
        };
        let mut graph = if duplicate {
            self.graph.clone()
        } else {
            AudioGraph::new("", "")
        };
        graph.id = GraphId::new(format!("user-{number}"));
        graph.name = name;
        self.graphs.push(std::mem::replace(&mut self.graph, graph));
    }

    pub fn delete_selected_graph(&mut self) -> Result<(), String> {
        if self.graph.id.0 == DEFAULT_AUDIO_GRAPH_ID {
            return Err("The default graph cannot be deleted".into());
        }
        let removed = self.graph.id.clone();
        self.select_graph(&GraphId::new(DEFAULT_AUDIO_GRAPH_ID))?;
        self.graphs.retain(|graph| graph.id != removed);
        Ok(())
    }

    pub fn reset_selected_graph(&mut self) {
        let id = self.graph.id.clone();
        let name = self.graph.name.clone();
        self.graph = default_graph();
        self.graph.id = id;
        if self.graph.id.0 != DEFAULT_AUDIO_GRAPH_ID {
            self.graph.name = name;
        }
    }

    pub fn normalize(&mut self) {
        // v3 adds conditional app-microphone output. Older executors must not
        // silently interpret it as a mixed microphone/TTS route.
        fn clear_empty(selection: &mut Option<DeviceId>) {
            if selection
                .as_ref()
                .is_some_and(|device| device.0.trim().is_empty())
            {
                *selection = None;
            }
        }

        if !self
            .all_graphs()
            .any(|graph| graph.id.0 == DEFAULT_AUDIO_GRAPH_ID)
        {
            self.graphs.push(default_graph());
        }
        clear_empty(&mut self.device_defaults.microphone_device_id);
        clear_empty(&mut self.device_defaults.system_audio_device_id);
        clear_empty(&mut self.device_defaults.monitor_device_id);
        clear_empty(&mut self.device_defaults.game_microphone_device_id);
        for graph in std::iter::once(&mut self.graph).chain(&mut self.graphs) {
            if graph.format_version == 2 {
                graph.format_version = super::graph::AUDIO_GRAPH_FORMAT_VERSION;
            }
            graph.initialize_source_gates();
            for node in &mut graph.nodes {
                match &mut node.kind {
                    AudioNodeKind::Microphone { device_id }
                    | AudioNodeKind::MonitorOutput { device_id }
                    | AudioNodeKind::GameMicrophoneOutput { device_id, .. } => {
                        clear_empty(device_id);
                    }
                    AudioNodeKind::SystemAudio {
                        capture: SystemAudioCapture::Endpoint { device_id, .. },
                    } => clear_empty(device_id),
                    AudioNodeKind::SystemAudio {
                        capture:
                            SystemAudioCapture::Application {
                                resolved_process_id,
                                ..
                            },
                    } => *resolved_process_id = None,
                    _ => {}
                }
            }
            let mixer_ids = graph
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, AudioNodeKind::Mixer))
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();
            for mixer_id in mixer_ids {
                normalize_mixer_ports(graph, &mixer_id);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedDocument {
    schema_version: u32,
    settings: AudioStudioSettings,
}

#[derive(Debug, Deserialize)]
struct PersistedDocumentHeader {
    schema_version: u32,
}

#[derive(Debug, Clone)]
pub struct AudioStudioRepository {
    path: PathBuf,
}

impl AudioStudioRepository {
    pub fn open(project_root: &Path) -> Self {
        Self {
            path: project_root.join(AUDIO_STUDIO_SETTINGS_PATH),
        }
    }

    #[cfg(test)]
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<AudioStudioSettings, AudioStudioPersistenceError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(AudioStudioSettings::default());
            }
            Err(error) => return Err(AudioStudioPersistenceError::Io(error)),
        };
        let header: PersistedDocumentHeader =
            serde_json::from_slice(&bytes).map_err(AudioStudioPersistenceError::InvalidJson)?;
        if header.schema_version < 5 {
            let settings = AudioStudioSettings::default();
            self.save(&settings)?;
            return Ok(settings);
        }
        if header.schema_version > AUDIO_STUDIO_SCHEMA_VERSION {
            return Err(AudioStudioPersistenceError::UnsupportedVersion(
                header.schema_version,
            ));
        }
        let mut document: PersistedDocument =
            serde_json::from_slice(&bytes).map_err(AudioStudioPersistenceError::InvalidJson)?;
        document.settings.normalize();
        if header.schema_version < AUDIO_STUDIO_SCHEMA_VERSION {
            self.save(&document.settings)?;
        }
        Ok(document.settings)
    }

    pub fn save(&self, settings: &AudioStudioSettings) -> Result<(), AudioStudioPersistenceError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(AudioStudioPersistenceError::Io)?;
        }
        let mut normalized = settings.clone();
        normalized.normalize();
        let document = PersistedDocument {
            schema_version: AUDIO_STUDIO_SCHEMA_VERSION,
            settings: normalized,
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(AudioStudioPersistenceError::InvalidJson)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(AudioStudioPersistenceError::Io)?;
        replace_file(&temporary, &self.path).map_err(AudioStudioPersistenceError::Io)
    }
}

fn normalize_mixer_ports(graph: &mut AudioGraph, mixer_id: &NodeId) {
    let mut reserved = graph
        .links
        .iter()
        .filter(|link| &link.to.node_id == mixer_id)
        .filter_map(|link| link.to.port_id.mixer_input_index())
        .collect::<std::collections::HashSet<_>>();
    let mut retained = std::collections::HashSet::new();
    for link in graph
        .links
        .iter_mut()
        .filter(|link| &link.to.node_id == mixer_id)
    {
        let keep = link
            .to
            .port_id
            .mixer_input_index()
            .filter(|index| retained.insert(*index));
        if keep.is_some() {
            continue;
        }
        let index = (0..).find(|index| !reserved.contains(index)).unwrap();
        reserved.insert(index);
        retained.insert(index);
        link.to.port_id = PortId::mixer_input(index);
    }
}

fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    match fs::rename(temporary, target) {
        Ok(()) => Ok(()),
        Err(_first_error) if target.exists() => {
            let backup = target.with_extension("json.bak");
            if backup.exists() {
                fs::remove_file(&backup)?;
            }
            fs::rename(target, &backup)?;
            if let Err(error) = fs::rename(temporary, target) {
                let _ = fs::rename(&backup, target);
                return Err(error);
            }
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
pub enum AudioStudioPersistenceError {
    Io(io::Error),
    InvalidJson(serde_json::Error),
    UnsupportedVersion(u32),
}

impl fmt::Display for AudioStudioPersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Audio Studio settings I/O failed: {error}"),
            Self::InvalidJson(error) => {
                write!(formatter, "Audio Studio settings are invalid: {error}")
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported Audio Studio settings version {version}"
                )
            }
        }
    }
}

impl std::error::Error for AudioStudioPersistenceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_path(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "xrtranslate-audio-studio-{name}-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn repository_uses_the_core_runtime_path() {
        assert_eq!(AUDIO_STUDIO_SETTINGS_PATH, "runtime/audio_studio.json");
    }

    #[test]
    fn v2_graph_load_preserves_customizations_and_upgrades_output_semantics_version() {
        let path = test_path("v2-graph");
        let repository = AudioStudioRepository::at_path(&path);
        let mut settings = AudioStudioSettings::default();
        settings.graph.nodes[0].label = "My capture source".into();
        settings.graph.format_version = 2;
        let document = PersistedDocument {
            schema_version: 5,
            settings: settings.clone(),
        };
        let mut document = serde_json::to_value(document).unwrap();
        document["settings"]
            .as_object_mut()
            .unwrap()
            .remove("graphs");
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let loaded = repository.load().unwrap();
        settings.graph.format_version = super::super::graph::AUDIO_GRAPH_FORMAT_VERSION;
        assert_eq!(loaded, settings);
        assert!(loaded.graph.validate().is_valid());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_file_loads_the_global_audio_graph() {
        let path = test_path("missing");
        let loaded = AudioStudioRepository::at_path(path).load().unwrap();
        assert_eq!(loaded.graph.id.0, DEFAULT_AUDIO_GRAPH_ID);
        assert_eq!(
            loaded.graph.name,
            AudioStudioPreset::CompleteAudioSystem.display_name()
        );
    }

    #[test]
    fn save_and_load_round_trip_current_schema() {
        let path = test_path("roundtrip");
        let repository = AudioStudioRepository::at_path(&path);
        let mut settings = AudioStudioSettings::default();
        settings.device_defaults.monitor_device_id = Some(DeviceId::new("monitor-1"));
        settings.graph.nodes[0].label = "My input".into();
        let original = settings.graph.clone();
        settings.create_graph(true);
        let copy_id = settings.graph.id.clone();
        assert_ne!(copy_id, original.id);
        assert_eq!(settings.graph.nodes, original.nodes);
        settings.graph.name = "Custom route".into();
        settings.create_graph(false);
        assert!(settings.graph.nodes.is_empty());
        settings.select_graph(&copy_id).unwrap();
        repository.save(&settings).unwrap();
        let mut loaded = repository.load().unwrap();
        assert_eq!(loaded, settings);
        loaded.select_graph(&original.id).unwrap();
        assert_eq!(loaded.graph, original);
        assert!(loaded.delete_selected_graph().is_err());
        loaded.select_graph(&copy_id).unwrap();
        loaded.delete_selected_graph().unwrap();
        assert_eq!(loaded.graph, original);
        assert!(!loaded.all_graphs().any(|graph| graph.id == copy_id));
        repository.save(&loaded).unwrap();
        assert_eq!(repository.load().unwrap(), loaded);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn older_schema_is_replaced_with_the_current_global_graph() {
        let path = test_path("legacy");
        let value = serde_json::json!({
            "schema_version": 3,
            "settings": {
                "selected_graph_id": "translation-safe",
                "graphs": []
            }
        });
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let repository = AudioStudioRepository::at_path(&path);
        let loaded = repository.load().unwrap();
        assert_eq!(loaded.graph.id.0, DEFAULT_AUDIO_GRAPH_ID);
        assert_eq!(
            loaded.graph.name,
            AudioStudioPreset::CompleteAudioSystem.display_name()
        );

        let rewritten: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(rewritten["schema_version"], AUDIO_STUDIO_SCHEMA_VERSION);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn application_audio_selection_round_trips_without_a_pid() {
        let path = test_path("application");
        let repository = AudioStudioRepository::at_path(&path);
        let mut settings = AudioStudioSettings::default();
        settings.replace_with_preset(AudioStudioPreset::VrchatKaraoke);
        let bgm = settings
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id.0 == "bgm")
            .unwrap();
        bgm.kind = crate::audio_studio::AudioNodeKind::SystemAudio {
            capture: crate::audio_studio::SystemAudioCapture::Application {
                application: Some(crate::audio_studio::ApplicationSelection {
                    id: crate::audio_studio::ApplicationId::new("c:\\music.exe"),
                    display_name: "Music".into(),
                }),
                resolved_process_id: Some(99),
            },
        };
        repository.save(&settings).unwrap();
        let loaded = repository.load().unwrap();
        let loaded_bgm = loaded
            .graph
            .nodes
            .iter()
            .find(|node| node.id.0 == "bgm")
            .unwrap();
        assert!(matches!(
            &loaded_bgm.kind,
            crate::audio_studio::AudioNodeKind::SystemAudio {
                capture: crate::audio_studio::SystemAudioCapture::Application {
                    resolved_process_id: None,
                    ..
                }
            }
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn normalize_clears_empty_device_ids() {
        let mut settings = AudioStudioSettings::default();
        settings.device_defaults.monitor_device_id = Some(DeviceId::new(""));
        settings.replace_with_preset(AudioStudioPreset::VrchatKaraoke);
        let karaoke = &mut settings.graph;
        let bgm = karaoke
            .nodes
            .iter_mut()
            .find(|node| node.id.0 == "bgm")
            .unwrap();
        if let crate::audio_studio::graph::AudioNodeKind::SystemAudio {
            capture: crate::audio_studio::graph::SystemAudioCapture::Endpoint { device_id, .. },
        } = &mut bgm.kind
        {
            *device_id = Some(DeviceId::new(""));
        }

        settings.normalize();

        assert!(settings.device_defaults.monitor_device_id.is_none());
        let karaoke = &settings.graph;
        let bgm = karaoke
            .nodes
            .iter()
            .find(|node| node.id.0 == "bgm")
            .unwrap();
        assert!(bgm.kind.selected_device().is_none());
    }

    #[test]
    fn normalize_assigns_stable_ports_to_legacy_mixer_inputs() {
        let mut settings = AudioStudioSettings::default();
        for link in settings
            .graph
            .links
            .iter_mut()
            .filter(|link| link.to.node_id == NodeId::new("asr-input-mixer"))
        {
            link.to.port_id = PortId::input();
        }

        settings.normalize();

        let ports = settings
            .graph
            .links
            .iter()
            .filter(|link| link.to.node_id == NodeId::new("asr-input-mixer"))
            .map(|link| link.to.port_id.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ports.len(), 2);
        assert!(ports.iter().all(|port| port.mixer_input_index().is_some()));
    }

    #[test]
    fn future_schema_is_not_silently_downgraded() {
        let path = test_path("future");
        let value = serde_json::json!({
            "schema_version": 99,
            "settings": AudioStudioSettings::default(),
        });
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let error = AudioStudioRepository::at_path(&path).load().unwrap_err();
        assert!(matches!(
            error,
            AudioStudioPersistenceError::UnsupportedVersion(99)
        ));
        fs::remove_file(path).unwrap();
    }
}
