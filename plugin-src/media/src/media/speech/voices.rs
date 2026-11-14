//! Project-owned reference audio. Synthesis continues through the existing TTS service.
use super::reference::{Prepared, References};
use crate::media::{backend, controller::MediaController, task::ProjectVoice};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{collections::HashSet, io::Write, sync::mpsc};

type LoadResult = (String, Result<Vec<Prepared>, String>);

#[derive(Default)]
pub(in crate::media) struct Library {
    loading: Option<mpsc::Receiver<LoadResult>>,
    attempted: HashSet<String>,
    preview: Option<(String, Box<dyn backend::MediaBackend>)>,
}

impl Library {
    pub(in crate::media) fn busy(&self) -> bool {
        self.loading.is_some()
    }

    pub(in crate::media) fn previewing(&self, id: &str) -> bool {
        self.preview.as_ref().is_some_and(|(voice, _)| voice == id)
    }

    pub(in crate::media) fn stop_preview(&mut self) {
        self.preview = None;
    }
}

pub(super) fn saved(task: &super::MediaTask, id: &str) -> Result<Prepared, String> {
    let voice = task
        .speech_voices
        .iter()
        .find(|voice| voice.id == id)
        .ok_or("Voice no longer exists.")?;
    if std::fs::metadata(&voice.reference)
        .map_err(|e| e.to_string())?
        .len()
        > 2_000_000
    {
        return Err("Invalid voice reference audio.".into());
    }
    Ok(Prepared {
        id: voice.id.clone(),
        audio: STANDARD.encode(std::fs::read(&voice.reference).map_err(|e| e.to_string())?),
        range: voice.range,
    })
}

impl MediaController {
    pub(super) fn save_project_voice(
        &mut self,
        task_id: &str,
        voice: Prepared,
    ) -> Result<(), String> {
        let task = self
            .store
            .get(task_id)
            .ok_or("Media project no longer exists")?;
        if task.speech_voices.iter().any(|old| old.id == voice.id) {
            return Ok(());
        }
        let migrated = task.speech_voices_migrated;
        let directory = self.storage_dir.join("media-voices");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
        file.write_all(&STANDARD.decode(&voice.audio).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        let path = directory.join(format!("{}.wav", uuid::Uuid::new_v4()));
        file.persist(&path).map_err(|e| e.to_string())?;
        self.store
            .get_mut(task_id)
            .unwrap()
            .speech_voices
            .push(ProjectVoice {
                id: voice.id,
                name: String::new(),
                reference: path.clone(),
                range: voice.range,
            });
        self.store.get_mut(task_id).unwrap().speech_voices_migrated = true;
        if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
            self.store.get_mut(task_id).unwrap().speech_voices.pop();
            self.store.get_mut(task_id).unwrap().speech_voices_migrated = migrated;
            let _ = std::fs::remove_file(path);
            return Err(error.to_string());
        }
        Ok(())
    }

    pub(in crate::media) fn collect_project_voices(&mut self, selected: bool) {
        if self.voice_library.busy() {
            return;
        }
        let Some(mut task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .cloned()
        else {
            return;
        };
        if selected && task.speech_reference_range.is_none() {
            return;
        }
        if !selected {
            task.speech_reference_range = None;
        }
        self.voice_library.attempted.insert(task.id.clone());
        let (send, receive) = mpsc::channel();
        let ctx = self.context.clone();
        let worker = std::thread::Builder::new()
            .name("media-voices".into())
            .spawn(move || {
                let result = (|| {
                    let mut references = References::default();
                    let mut prepared = Vec::new();
                    if selected {
                        let [start, end] = task.speech_reference_range.unwrap();
                        prepared.push(Prepared {
                            id: format!("selected:{start}:{end}"),
                            audio: super::reference::audio(&task, start, end)?,
                            range: [start, end],
                        });
                    } else {
                        for index in 0..task.subtitles.count() {
                            if let Ok(voice) = references.choose(&task, index) {
                                references.remember(&task, index, voice.clone());
                                if !prepared.iter().any(|old: &Prepared| old.id == voice.id) {
                                    prepared.push(voice);
                                }
                            }
                        }
                    }
                    Ok(prepared)
                })();
                let _ = send.send((task.id, result));
                ctx.request_repaint();
            });
        match worker {
            Ok(_) => self.voice_library.loading = Some(receive),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(in crate::media) fn poll_project_voices(&mut self) {
        if self.voice_library.loading.is_some() || self.voice_library.preview.is_some() {
            self.context
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if let Some((_, player)) = &mut self.voice_library.preview {
            player.tick();
            if let Some(error) = player.take_error() {
                self.error = Some(error);
                self.voice_library.preview = None;
            } else if player.get_duration_ms() > 0
                && player.get_time_ms() >= player.get_duration_ms().saturating_sub(50)
            {
                self.voice_library.preview = None;
            }
        }
        if let Some(result) =
            self.voice_library
                .loading
                .as_ref()
                .and_then(|worker| match worker.try_recv() {
                    Ok(result) => Some(Ok(result)),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Voice preparation stopped.".to_owned()))
                    }
                })
        {
            self.voice_library.loading = None;
            let result = result.and_then(|(task, voices)| {
                for voice in voices? {
                    self.save_project_voice(&task, voice)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                self.error = Some(error);
            }
        }
        // Older projects used temporary references. Recover reusable source voices once,
        // without touching their existing dubbing or enrolling a model in the UI thread.
        if self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .is_some_and(|task| {
                task.speech_use_source_voice
                    && task.speech_track.is_some()
                    && task.speech_voices.is_empty()
                    && !task.speech_voices_migrated
                    && task
                        .separated_audio
                        .as_ref()
                        .is_some_and(|stems| stems.vocals.is_file())
                    && !self.voice_library.attempted.contains(&task.id)
            })
        {
            self.collect_project_voices(false);
        }
    }

    pub(in crate::media) fn audition_project_voice(&mut self, id: &str) {
        if self.voice_library.previewing(id) {
            self.voice_library.stop_preview();
            return;
        }
        let path = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .and_then(|task| task.speech_voices.iter().find(|voice| voice.id == id))
            .map(|voice| voice.reference.clone());
        let Some(path) = path else {
            return;
        };
        self.preview_voice_audio(id, path);
    }

    pub(in crate::media) fn audition_speech_take(&mut self, id: &str, previous: bool) {
        let clip = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get(id))
            .and_then(|task| task.speech_track.as_ref())
            .and_then(|track| track.clips.iter().find(|clip| clip.id == id));
        let Some(clip) = clip.and_then(|clip| {
            if previous {
                clip.previous.as_deref()
            } else {
                Some(clip)
            }
        }) else {
            return;
        };
        let (id, path) = (clip.id.clone(), clip.path.clone());
        self.preview_voice_audio(&id, path);
    }

    fn preview_voice_audio(&mut self, id: &str, path: std::path::PathBuf) {
        if self.voice_library.previewing(id) {
            self.voice_library.stop_preview();
            return;
        }
        self.voice_library.stop_preview();
        let result = (|| {
            let mut player = backend::create()?;
            player.set_audio_only_mode(true);
            player.set_volume(self.volume);
            player.load_local_file(path)?;
            player.play();
            Ok::<_, String>(player)
        })();
        match result {
            Ok(player) => {
                if let Some(video) = &mut self.backend {
                    video.pause();
                }
                self.voice_library.preview = Some((id.into(), player));
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(in crate::media) fn remove_project_voice(&mut self, id: &str) {
        if self.speech_job.is_some() {
            return;
        }
        let Some(task_id) = self.active_task_id.clone() else {
            return;
        };
        let Some(task) = self.store.get_mut(&task_id) else {
            return;
        };
        if task.speech_track.as_ref().is_some_and(|track| {
            track.clips.iter().any(|clip| {
                [Some(clip), clip.previous.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|clip| {
                        clip.origin
                            .as_ref()
                            .and_then(|origin| origin.voice_id.as_deref())
                            == Some(id)
                    })
            })
        }) {
            return;
        }
        let Some(index) = task.speech_voices.iter().position(|voice| voice.id == id) else {
            return;
        };
        let voice = task.speech_voices.remove(index);
        let migrated = std::mem::replace(&mut task.speech_voices_migrated, true);
        if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
            self.store
                .get_mut(&task_id)
                .unwrap()
                .speech_voices
                .insert(index, voice);
            self.store.get_mut(&task_id).unwrap().speech_voices_migrated = migrated;
            self.error = Some(error.to_string());
            return;
        }
        if self.voice_library.previewing(id) {
            self.voice_library.stop_preview();
        }
        self.remove_voice_file(&voice.reference);
    }

    pub(in crate::media) fn remove_voice_file(&self, path: &std::path::Path) {
        if let Ok(root) = self.storage_dir.join("media-voices").canonicalize()
            && path
                .canonicalize()
                .is_ok_and(|path| path.parent() == Some(root.as_path()))
        {
            let _ = std::fs::remove_file(path);
        }
    }

    pub(in crate::media) fn rename_project_voice(&mut self, id: &str, name: String) {
        let Some(task) = self
            .active_task_id
            .as_deref()
            .and_then(|id| self.store.get_mut(id))
        else {
            return;
        };
        let Some(voice) = task.speech_voices.iter_mut().find(|voice| voice.id == id) else {
            return;
        };
        let old = std::mem::replace(&mut voice.name, name.trim().chars().take(100).collect());
        if let Err(error) = self.store.save_to_dir(&self.storage_dir) {
            self.store
                .get_mut(self.active_task_id.as_deref().unwrap())
                .unwrap()
                .speech_voices
                .iter_mut()
                .find(|voice| voice.id == id)
                .unwrap()
                .name = old;
            self.error = Some(error.to_string());
        }
    }
}
