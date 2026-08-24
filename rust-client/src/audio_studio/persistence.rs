use super::{
    graph::{AudioGraph, AudioNodeKind, DeviceId, GraphId, NodeId, PortId, SystemAudioCapture},
    presets::{AudioStudioPreset, graph_for_preset},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub const AUDIO_STUDIO_SCHEMA_VERSION: u32 = 5;
pub const AUDIO_STUDIO_SETTINGS_PATH: &str = "runtime/audio_studio.json";
pub const GLOBAL_AUDIO_GRAPH_ID: &str = "audio-system";

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
}

fn preset_graph(preset: AudioStudioPreset) -> AudioGraph {
    let mut graph = graph_for_preset(preset);
    graph.id = GraphId::new(GLOBAL_AUDIO_GRAPH_ID);
    graph
}

fn default_graph() -> AudioGraph {
    preset_graph(AudioStudioPreset::CompleteAudioSystem)
}

impl Default for AudioStudioSettings {
    fn default() -> Self {
        Self {
            device_defaults: DeviceDefaults::default(),
            graph: default_graph(),
        }
    }
}

impl AudioStudioSettings {
    pub fn replace_with_preset(&mut self, preset: AudioStudioPreset) {
        self.graph = preset_graph(preset);
    }

    pub fn normalize(&mut self) {
        fn clear_empty(selection: &mut Option<DeviceId>) {
            if selection
                .as_ref()
                .is_some_and(|device| device.0.trim().is_empty())
            {
                *selection = None;
            }
        }

        self.graph.id = GraphId::new(GLOBAL_AUDIO_GRAPH_ID);
        clear_empty(&mut self.device_defaults.microphone_device_id);
        clear_empty(&mut self.device_defaults.system_audio_device_id);
        clear_empty(&mut self.device_defaults.monitor_device_id);
        clear_empty(&mut self.device_defaults.game_microphone_device_id);
        for node in &mut self.graph.nodes {
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
        let mixer_ids = self
            .graph
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, AudioNodeKind::Mixer))
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        for mixer_id in mixer_ids {
            normalize_mixer_ports(&mut self.graph, &mixer_id);
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
        if header.schema_version < AUDIO_STUDIO_SCHEMA_VERSION {
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
    fn missing_file_loads_the_global_audio_graph() {
        let path = test_path("missing");
        let loaded = AudioStudioRepository::at_path(path).load().unwrap();
        assert_eq!(loaded.graph.id.0, GLOBAL_AUDIO_GRAPH_ID);
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
        repository.save(&settings).unwrap();
        let loaded = repository.load().unwrap();
        assert_eq!(loaded, settings);
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
        assert_eq!(loaded.graph.id.0, GLOBAL_AUDIO_GRAPH_ID);
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
