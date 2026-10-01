//! Reference persistence and active voice selection across TTS providers.
use crate::model_runtime::NativeTtsAdapter;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};
use tracing::{info, warn};
use xrtranslate_protocol::tts::{VoiceSelection, VoiceStatus};

pub(crate) struct VoiceLibrary {
    directory: PathBuf,
    adapter: Option<NativeTtsAdapter>,
    status: RwLock<VoiceStatus>,
    operation: tokio::sync::Mutex<()>,
}

impl VoiceLibrary {
    pub(crate) async fn open(
        directory: PathBuf,
        provider: String,
        adapter: Option<NativeTtsAdapter>,
    ) -> Self {
        let selection: VoiceSelection = fs::read(directory.join("selection.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let library = Self {
            directory,
            adapter,
            status: RwLock::new(VoiceStatus {
                provider,
                selection: selection.clone(),
                ..Default::default()
            }),
            operation: tokio::sync::Mutex::new(()),
        };
        if let Some(adapter) = &library.adapter {
            restore_persisted_voice_clones(&library.directory, adapter).await;
            if selection.voice_id.is_some() {
                if let Err(error) = library.register_selection(&selection).await {
                    warn!(%error, "could not restore selected voice");
                }
            }
        }
        if let Some(adapter) = &library.adapter {
            let ready = adapter.has_voice(&voice_name(&selection)).await;
            let personal_available = adapter.has_voice(MICROPHONE_VOICE_NAME).await;
            let mut status = library.status.write().expect("voice status lock");
            status.ready = ready;
            status.personal_available = personal_available;
        }
        library
    }

    pub(crate) fn active_voice(&self) -> (String, bool) {
        let status = self.status.read().expect("voice status lock");
        (voice_name(&status.selection), status.ready)
    }

    pub(crate) fn status(&self) -> VoiceStatus {
        self.status.read().expect("voice status lock").clone()
    }

    pub(crate) async fn select(&self, selection: VoiceSelection) -> Result<VoiceStatus, String> {
        let _operation = self.operation.lock().await;
        self.register_selection(&selection).await?;
        self.save_selection(selection)?;
        Ok(self.status())
    }

    async fn register_selection(&self, selection: &VoiceSelection) -> Result<(), String> {
        let adapter = self
            .adapter
            .as_ref()
            .ok_or("Configure a TTS provider in Settings first.")?;
        let name = voice_name(selection);
        if let Some(id) = &selection.voice_id {
            if !adapter.has_voice(&name).await {
                let (wav, transcript) =
                    xrtranslate_assets::voices::VoiceCatalog::new(&self.directory).reference(id)?;
                adapter
                    .register_voice(&name, wav.into_owned(), transcript.trim())
                    .await
                    .map_err(|e| e.to_string())?;
            }
        } else if !adapter.has_voice(&name).await {
            return Err("Record your voice before selecting it.".into());
        }
        Ok(())
    }

    pub(crate) async fn register_personal(
        &self,
        wav: Vec<u8>,
        transcript: &str,
    ) -> Result<(), String> {
        let _operation = self.operation.lock().await;
        let adapter = self
            .adapter
            .as_ref()
            .ok_or("Configure a TTS provider in Settings first.")?;
        adapter
            .register_voice(MICROPHONE_VOICE_NAME, wav.clone(), transcript)
            .await
            .map_err(|e| e.to_string())?;
        save_persisted_voice_clone(&self.directory, MICROPHONE_VOICE_NAME, &wav, transcript)
            .map_err(|e| e.to_string())?;
        self.status
            .write()
            .expect("voice status lock")
            .personal_available = true;
        self.save_selection(VoiceSelection::default())
    }

    fn save_selection(&self, selection: VoiceSelection) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let path = self.directory.join("selection.json");
        let temporary = path.with_extension("tmp");
        fs::write(
            &temporary,
            serde_json::to_vec(&selection).map_err(|e| e.to_string())?,
        )
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|e| e.to_string())?;
        let mut status = self.status.write().expect("voice status lock");
        status.selection = selection;
        status.ready = true;
        Ok(())
    }
}

fn voice_name(selection: &VoiceSelection) -> String {
    selection.voice_id.as_ref().map_or_else(
        || MICROPHONE_VOICE_NAME.into(),
        |id| format!("reference:{id}"),
    )
}

pub(crate) async fn voice_status(
    axum::extract::State(state): axum::extract::State<crate::BackendState>,
) -> Result<axum::Json<VoiceStatus>, (axum::http::StatusCode, String)> {
    state
        .prepare_speech()
        .await
        .map(|speech| axum::Json(speech.voices.status()))
        .map_err(|error| (axum::http::StatusCode::BAD_GATEWAY, error))
}

pub(crate) async fn select_voice(
    axum::extract::State(state): axum::extract::State<crate::BackendState>,
    axum::Json(selection): axum::Json<VoiceSelection>,
) -> Result<axum::Json<VoiceStatus>, (axum::http::StatusCode, String)> {
    state
        .prepare_speech()
        .await
        .map_err(|error| (axum::http::StatusCode::BAD_GATEWAY, error))?
        .voices
        .select(selection)
        .await
        .map(axum::Json)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error))
}

pub(crate) const MICROPHONE_VOICE_NAME: &str = "xrtranslate_microphone";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PersistedVoiceMetadata {
    pub(crate) voice_name: String,
    pub(crate) transcript: String,
    #[serde(default)]
    pub(crate) created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedVoiceClone {
    pub(crate) voice_name: String,
    pub(crate) transcript: String,
    pub(crate) wav_bytes: Vec<u8>,
}

pub(crate) fn sanitize_voice_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.trim().is_empty() {
        "voice".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn save_persisted_voice_clone(
    voice_clones_dir: &Path,
    voice_name: &str,
    wav: &[u8],
    transcript: &str,
) -> Result<(), std::io::Error> {
    fs::create_dir_all(voice_clones_dir)?;
    let file_stem = sanitize_voice_file_name(voice_name);
    let wav_path = voice_clones_dir.join(format!("{file_stem}.wav"));
    let meta_path = voice_clones_dir.join(format!("{file_stem}.json"));

    fs::write(&wav_path, wav)?;

    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let metadata = PersistedVoiceMetadata {
        voice_name: voice_name.to_owned(),
        transcript: transcript.to_owned(),
        created_at_ms,
    };
    let json_bytes = serde_json::to_vec_pretty(&metadata)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&meta_path, json_bytes)?;
    Ok(())
}

pub(crate) fn load_persisted_voice_clones(voice_clones_dir: &Path) -> Vec<PersistedVoiceClone> {
    if !voice_clones_dir.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(voice_clones_dir) else {
        return Vec::new();
    };

    let mut clones = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(metadata) = serde_json::from_str::<PersistedVoiceMetadata>(&content) else {
                continue;
            };
            if metadata.voice_name != MICROPHONE_VOICE_NAME {
                continue;
            }
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let wav_path = voice_clones_dir.join(format!("{file_stem}.wav"));
            if !wav_path.is_file() {
                continue;
            }
            let Ok(wav_bytes) = fs::read(&wav_path) else {
                continue;
            };
            clones.push(PersistedVoiceClone {
                voice_name: metadata.voice_name,
                transcript: metadata.transcript,
                wav_bytes,
            });
        }
    }
    clones.sort_by(|a, b| a.voice_name.cmp(&b.voice_name));
    clones
}

pub(crate) async fn restore_persisted_voice_clones(
    voice_clones_dir: &Path,
    adapter: &NativeTtsAdapter,
) -> usize {
    let clones = load_persisted_voice_clones(voice_clones_dir);
    let mut restored = 0;
    for clone in clones {
        match adapter
            .register_voice(&clone.voice_name, clone.wav_bytes, &clone.transcript)
            .await
        {
            Ok(()) => {
                info!(voice = %clone.voice_name, "restored persisted voice clone");
                restored += 1;
            }
            Err(error) => {
                warn!(
                    voice = %clone.voice_name,
                    %error,
                    "failed to restore persisted voice clone into active TTS provider"
                );
            }
        }
    }
    restored
}
