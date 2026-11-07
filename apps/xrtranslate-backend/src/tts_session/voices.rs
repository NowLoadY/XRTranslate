//! Reference persistence and active voice selection across TTS providers.
use crate::model_runtime::TtsAdapter;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};
use tracing::{info, warn};
use xrtranslate_protocol::tts::{VoiceSelection, VoiceStatus};

#[derive(Clone, Default, Serialize, Deserialize)]
struct SavedSelection {
    #[serde(flatten)]
    selection: VoiceSelection,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    online_voices: std::collections::BTreeMap<String, String>,
}

pub(crate) struct VoiceLibrary {
    directory: PathBuf,
    selection_key: String,
    adapter: Option<TtsAdapter>,
    status: RwLock<VoiceStatus>,
    online_voices: RwLock<std::collections::BTreeMap<String, String>>,
    operation: tokio::sync::Mutex<()>,
}

impl VoiceLibrary {
    pub(crate) async fn open(
        directory: PathBuf,
        provider: String,
        selection_key: String,
        adapter: Option<TtsAdapter>,
    ) -> Self {
        let supports_cloning = adapter.as_ref().is_some_and(TtsAdapter::supports_cloning);
        let supports_reference_audio = adapter
            .as_ref()
            .is_some_and(TtsAdapter::supports_reference_audio);
        let preset_voices = adapter
            .as_ref()
            .map(TtsAdapter::preset_voices)
            .unwrap_or_default();
        let selections: std::collections::BTreeMap<String, SavedSelection> =
            fs::read(directory.join("selections.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
        let scope = adapter.as_ref().and_then(TtsAdapter::voice_scope);
        let scoped_key = scope.as_ref().map_or_else(
            || selection_key.clone(),
            |scope| format!("{selection_key}:{scope}"),
        );
        let saved = selections.get(&scoped_key).cloned().or_else(|| {
            // Older online selections contain presets, never enrolled IDs.
            selections.get(&selection_key).cloned()
        });
        let mut selection = saved
            .as_ref()
            .map(|saved| saved.selection.clone())
            .unwrap_or_else(|| {
                if scope.is_none() && supports_cloning {
                    fs::read(directory.join("selection.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                        .unwrap_or_default()
                } else {
                    VoiceSelection {
                        preset: preset_voices.first().cloned(),
                        ..Default::default()
                    }
                }
            });
        if !supports_cloning {
            selection.voice_id = None;
            if !selection
                .preset
                .as_ref()
                .is_some_and(|voice| preset_voices.contains(voice))
            {
                selection.preset = preset_voices.first().cloned();
            }
        } else if preset_voices.is_empty() {
            selection.preset = None;
        }
        let library = Self {
            directory,
            selection_key: scoped_key,
            adapter,
            status: RwLock::new(VoiceStatus {
                provider,
                selection: selection.clone(),
                supports_cloning,
                supports_reference_audio,
                preset_voices,
                ..Default::default()
            }),
            online_voices: RwLock::new(saved.map(|saved| saved.online_voices).unwrap_or_default()),
            operation: tokio::sync::Mutex::new(()),
        };
        if let Some(adapter) = &library.adapter {
            // Cloud voices already exist; opening a session must not enroll them again.
            if adapter.is_local() {
                restore_persisted_voice_clones(&library.directory, adapter).await;
                if selection.voice_id.is_some()
                    && let Err(error) = library.register_selection(&selection).await
                {
                    warn!(%error, "could not restore selected voice");
                }
            }
            let ready = library.has_voice(&voice_name(&selection)).await;
            let personal_available = library.has_voice(MICROPHONE_VOICE_NAME).await;
            let mut status = library.status.write().expect("voice status lock");
            status.ready = ready;
            status.personal_available = personal_available;
        }
        library
    }

    pub(crate) fn active_voice(&self) -> (String, bool) {
        let status = self.status.read().expect("voice status lock");
        (self.resolved_voice(&status.selection), status.ready)
    }

    fn resolved_voice(&self, selection: &VoiceSelection) -> String {
        let name = voice_name(selection);
        self.online_voices
            .read()
            .expect("voice IDs lock")
            .get(&name)
            .cloned()
            .unwrap_or(name)
    }

    async fn has_voice(&self, name: &str) -> bool {
        if self
            .online_voices
            .read()
            .expect("voice IDs lock")
            .contains_key(name)
        {
            return true;
        }
        match &self.adapter {
            Some(adapter) => adapter.has_voice(name).await,
            None => false,
        }
    }

    pub(crate) fn status(&self) -> VoiceStatus {
        self.status.read().expect("voice status lock").clone()
    }

    pub(crate) async fn select(&self, selection: VoiceSelection) -> Result<VoiceStatus, String> {
        let _operation = self.operation.lock().await;
        self.prepare_selection(&selection).await?;
        self.save_selection(selection, None)?;
        Ok(self.status())
    }

    pub(super) async fn register_selection(
        &self,
        selection: &VoiceSelection,
    ) -> Result<String, String> {
        let _operation = self.operation.lock().await;
        self.prepare_selection(selection).await?;
        Ok(self.resolved_voice(selection))
    }

    async fn prepare_selection(&self, selection: &VoiceSelection) -> Result<(), String> {
        let adapter = self
            .adapter
            .as_ref()
            .ok_or("Configure a TTS provider on the Welcome Page first.")?;
        if let Some(preset) = &selection.preset {
            return if selection.voice_id.is_none() && adapter.preset_voices().contains(preset) {
                Ok(())
            } else {
                Err("Select a preset supported by the active TTS model.".into())
            };
        }
        if !adapter.supports_cloning() {
            return Err("Select a preset voice for this TTS model.".into());
        }
        let name = voice_name(selection);
        if !self.has_voice(&name).await {
            let Some(id) = &selection.voice_id else {
                return Err("Record your voice before selecting it.".into());
            };
            let (wav, transcript) =
                xrtranslate_assets::voices::VoiceCatalog::new(&self.directory).reference(id)?;
            let registered = adapter
                .register_voice(&name, wav.into_owned(), transcript.trim())
                .await
                .map_err(|e| e.to_string())?;
            if !adapter.is_local() {
                if let Err(error) =
                    self.save_selection(self.status().selection, Some((&name, &registered)))
                {
                    adapter.delete_reference_voice(&registered, "").await;
                    return Err(error);
                }
            }
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
            .ok_or("Configure a TTS provider on the Welcome Page first.")?;
        let registered = adapter
            .register_voice(MICROPHONE_VOICE_NAME, wav.clone(), transcript)
            .await
            .map_err(|e| e.to_string())?;
        let previous = self
            .online_voices
            .read()
            .expect("voice IDs lock")
            .get(MICROPHONE_VOICE_NAME)
            .cloned();
        let registration =
            (!adapter.is_local()).then_some((MICROPHONE_VOICE_NAME, registered.as_str()));
        let saved =
            save_persisted_voice_clone(&self.directory, MICROPHONE_VOICE_NAME, &wav, transcript)
                .map_err(|e| e.to_string())
                .and_then(|()| self.save_selection(VoiceSelection::default(), registration));
        if let Err(error) = saved {
            if registration.is_some() {
                adapter.delete_reference_voice(&registered, "").await;
            }
            return Err(error);
        }
        self.status
            .write()
            .expect("voice status lock")
            .personal_available = true;
        if let Some(previous) = previous.filter(|previous| previous != &registered) {
            adapter.delete_reference_voice(&previous, "").await;
        }
        Ok(())
    }

    fn save_selection(
        &self,
        selection: VoiceSelection,
        registration: Option<(&str, &str)>,
    ) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let path = self.directory.join("selections.json");
        let mut selections: std::collections::BTreeMap<String, SavedSelection> = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut online_voices = self.online_voices.read().expect("voice IDs lock").clone();
        if let Some((name, id)) = registration {
            online_voices.insert(name.to_owned(), id.to_owned());
        }
        selections.insert(
            self.selection_key.clone(),
            SavedSelection {
                selection: selection.clone(),
                online_voices: online_voices.clone(),
            },
        );
        let temporary = path.with_extension("tmp");
        fs::write(
            &temporary,
            serde_json::to_vec(&selections).map_err(|e| e.to_string())?,
        )
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|e| e.to_string())?;
        *self.online_voices.write().expect("voice IDs lock") = online_voices;
        let mut status = self.status.write().expect("voice status lock");
        status.selection = selection;
        // Preparing another voice for preview does not change active voice readiness.
        if registration.is_none()
            || registration.is_some_and(|(name, _)| name == voice_name(&status.selection))
        {
            status.ready = true;
        }
        Ok(())
    }
}

pub(super) fn voice_name(selection: &VoiceSelection) -> String {
    if let Some(preset) = &selection.preset {
        return preset.clone();
    }
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
    adapter: &TtsAdapter,
) -> usize {
    if !adapter.is_local() {
        return 0;
    }
    let clones = load_persisted_voice_clones(voice_clones_dir);
    let mut restored = 0;
    for clone in clones {
        match adapter
            .register_voice(&clone.voice_name, clone.wav_bytes, &clone.transcript)
            .await
        {
            Ok(_) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use xrtranslate_assets::remote::{RemoteConnector, remote_model};
    use xrtranslate_inference::{RemoteTtsAdapter, ReqwestClient};

    #[tokio::test]
    async fn online_clone_reuses_survives_restart_and_replaces_only_after_success() {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/api/v1/services/audio/tts/SpeechSynthesizer",
            listener.local_addr().unwrap()
        );
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sequence = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler = {
            let (requests, fail, sequence) = (requests.clone(), fail.clone(), sequence.clone());
            move |Json(body): Json<Value>| {
                let (requests, fail, sequence) = (requests.clone(), fail.clone(), sequence.clone());
                async move {
                    requests.lock().unwrap().push(body.clone());
                    let output = match body["input"]["action"].as_str().unwrap() {
                        "create_voice" => {
                            if fail.load(std::sync::atomic::Ordering::SeqCst) {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(json!({"message":"invalid sample"})),
                                );
                            }
                            let id = sequence.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            json!({"output":{"voice_id":format!("qwen-audio-3.1-tts-plus-xrt-{id}")}})
                        }
                        "query_voice" => json!({"output":{"status":"OK"}}),
                        "delete_voice" => json!({"output":{}}),
                        _ => panic!("unexpected action"),
                    };
                    (StatusCode::OK, Json(output))
                }
            }
        };
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/api/v1/services/audio/tts/customization", post(handler)),
            )
            .await
            .unwrap();
        });
        let model =
            remote_model(RemoteConnector::DashscopeSpeech, "qwen-audio-3.1-tts-plus").unwrap();
        let adapter = |key: &str| {
            TtsAdapter::remote(
                RemoteTtsAdapter::new(
                    ReqwestClient::with_default_direct_timeout().unwrap(),
                    endpoint.clone(),
                    key.into(),
                    model,
                ),
                vec!["en".into(), "zh".into()],
            )
        };
        let directory = tempfile::tempdir().unwrap();
        let open = |key: &str| {
            VoiceLibrary::open(
                directory.path().to_owned(),
                "qwen".into(),
                "qwen:plus".into(),
                Some(adapter(key)),
            )
        };
        let library = open("first-account").await;
        assert!(library.status().supports_cloning);
        assert!(library.status().selection.preset.is_some());
        assert!(library.active_voice().1);
        let wav = xrtranslate_inference::pcm16_mono_16khz_to_wav(&vec![0; 16000 * 2 * 5]).unwrap();
        library
            .register_personal(wav.clone(), "Reference speech")
            .await
            .unwrap();
        let first = library.active_voice();
        assert!(first.0.ends_with("-0") && first.1);
        assert!(library.status().personal_available);
        assert!(library.status().selection.voice_id.is_none());
        assert!(library.status().selection.preset.is_none());
        for _ in 0..3 {
            assert_eq!(
                library
                    .register_selection(&VoiceSelection::default())
                    .await
                    .unwrap(),
                first.0
            );
        }
        drop(library);
        let library = open("first-account").await;
        assert_eq!(library.active_voice(), first);
        assert_eq!(
            requests.lock().unwrap().len(),
            2,
            "restart and synthesis reuse enrollment"
        );
        fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            library
                .register_personal(wav.clone(), "Rejected replacement")
                .await
                .is_err()
        );
        assert_eq!(library.active_voice(), first);
        assert_eq!(open("first-account").await.active_voice(), first);
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        library
            .register_personal(wav, "New reference")
            .await
            .unwrap();
        let second = library.active_voice();
        assert!(second.0.ends_with("-1") && second.1);
        assert_eq!(open("first-account").await.active_voice(), second);
        assert_eq!(
            requests.lock().unwrap().last().unwrap()["input"]["voice_id"],
            first.0
        );
        assert_eq!(
            requests.lock().unwrap().last().unwrap()["input"]["action"],
            "delete_voice"
        );
        let other = open("another-account").await;
        assert!(other.status().selection.preset.is_some());
        assert!(!other.status().personal_available);
        assert_eq!(open("first-account").await.active_voice(), second);
        let saved = fs::read_to_string(directory.path().join("selections.json")).unwrap();
        assert!(!saved.contains("first-account"));
        let preset = library.status().preset_voices[0].clone();
        library
            .select(VoiceSelection {
                preset: Some(preset.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(library.active_voice(), (preset, true));
        library.select(VoiceSelection::default()).await.unwrap();
        assert_eq!(library.active_voice(), second);
        assert_eq!(requests.lock().unwrap().len(), 6);
        server.abort();
    }

    #[test]
    fn previous_selections_remain_readable() {
        let saved: SavedSelection =
            serde_json::from_value(json!({"voice_id":null,"preset":"longanlingxin_v3.1"})).unwrap();
        assert_eq!(
            saved.selection.preset.as_deref(),
            Some("longanlingxin_v3.1")
        );
        assert!(saved.online_voices.is_empty());
    }
}
