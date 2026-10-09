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
