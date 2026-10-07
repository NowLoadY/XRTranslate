use super::{backend::MediaSource, subtitles::SubtitleTimeline};
use crate::client_settings::RecognitionSettings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SubtitleMode {
    #[default]
    RealtimeTranslation,
    ImportedSrt(PathBuf),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaType {
    #[default]
    Video,
    AudioOnly,
    Subtitles,
}

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma", "ape", "alac",
];

pub fn detect_media_type(path: &Path) -> MediaType {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "srt" | "vtt" => MediaType::Subtitles,
        ext if AUDIO_EXTENSIONS.contains(&ext) => MediaType::AudioOnly,
        _ => MediaType::Video,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioChannelItem {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub is_left: bool,
    pub is_right: bool,
    pub is_center: bool,
    pub playback: bool,
    pub recognition: bool,
}

impl AudioChannelItem {
    pub fn default_for_count(count: usize) -> Vec<AudioChannelItem> {
        let layout: &[&str] = match count {
            1 => &["mono"],
            2 => &["fl", "fr"],
            3 => &["fl", "fr", "lfe"],
            4 => &["fl", "fr", "bl", "br"],
            5 => &["fl", "fr", "fc", "sl", "sr"],
            6 => &["fl", "fr", "fc", "lfe", "sl", "sr"],
            8 => &["fl", "fr", "fc", "lfe", "bl", "br", "sl", "sr"],
            _ => &[],
        };
        (0..count)
            .map(|index| {
                let id = layout.get(index).copied();
                let name = match id {
                    Some("mono") => "Mono (单声道)",
                    Some("fl") if count < 4 => "FL (左声道 / Front Left)",
                    Some("fr") if count < 4 => "FR (右声道 / Front Right)",
                    Some("fl") => "FL (前左 / Front Left)",
                    Some("fr") => "FR (前右 / Front Right)",
                    Some("fc") => "FC (中置对白 / Center Dialogue)",
                    Some("lfe") => "LFE (低音炮 / Subwoofer)",
                    Some("bl") => "BL (后左 / Back Left)",
                    Some("br") => "BR (后右 / Back Right)",
                    Some("sl") if count == 8 => "SL (左侧环绕 / Side Left)",
                    Some("sr") if count == 8 => "SR (右侧环绕 / Side Right)",
                    Some("sl") => "SL (左环绕 / Surround Left)",
                    Some("sr") => "SR (右环绕 / Surround Right)",
                    _ => "",
                };
                Self {
                    index,
                    id: id.map(str::to_owned).unwrap_or_else(|| format!("c{index}")),
                    name: if id.is_some() {
                        name.into()
                    } else {
                        format!("CH {} (声道 {})", index + 1, index + 1)
                    },
                    is_left: id.map_or(index % 2 == 0, |id| {
                        matches!(id, "mono" | "fl" | "bl" | "sl")
                    }),
                    is_right: id.map_or(index % 2 != 0, |id| {
                        matches!(id, "mono" | "fr" | "br" | "sr")
                    }),
                    is_center: id.is_some_and(|id| matches!(id, "mono" | "fc" | "lfe")),
                    playback: true,
                    recognition: id.is_none_or(|id| matches!(id, "mono" | "fl" | "fr" | "fc")),
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaTask {
    pub id: String,
    pub title: String,
    pub source: MediaSource,
    #[serde(default)]
    pub media_type: MediaType,
    pub source_language: String,
    pub target_language: String,
    pub subtitle_mode: SubtitleMode,
    #[serde(default = "default_video_recognition")]
    pub recognition: RecognitionSettings,
    #[serde(default)]
    pub audio_channels: Vec<AudioChannelItem>,
    #[serde(default)]
    pub is_task_running: bool,
    pub created_at_sec: u64,
    pub last_played_sec: u64,
    pub duration_ms: i64,
    pub subtitles: SubtitleTimeline,
}

fn default_video_recognition() -> RecognitionSettings {
    RecognitionSettings {
        background_noise: 0.6,
        pause_tolerance: 1.0,
        continuous_recognition: false,
    }
}

impl MediaTask {
    #[allow(dead_code)]
    pub fn new(
        title: String,
        source: MediaSource,
        source_lang: String,
        target_lang: String,
        subtitle_mode: SubtitleMode,
        recognition: RecognitionSettings,
    ) -> Self {
        let media_type = match &source {
            MediaSource::LocalFile(p) => detect_media_type(p),
            MediaSource::NetworkStream(_) => MediaType::Video,
        };
        Self::new_with_media_type(
            title,
            source,
            media_type,
            source_lang,
            target_lang,
            subtitle_mode,
            recognition,
        )
    }

    pub fn new_with_media_type(
        title: String,
        source: MediaSource,
        media_type: MediaType,
        source_lang: String,
        target_lang: String,
        subtitle_mode: SubtitleMode,
        recognition: RecognitionSettings,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            source,
            media_type,
            source_language: source_lang,
            target_language: target_lang,
            subtitle_mode,
            recognition,
            // Until probed, an empty selection lets the shared decoder mix every channel.
            audio_channels: Vec::new(),
            is_task_running: false,
            created_at_sec: now,
            last_played_sec: now,
            duration_ms: 0,
            subtitles: SubtitleTimeline::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MediaTaskStore {
    pub tasks: Vec<MediaTask>,
}

impl MediaTaskStore {
    pub fn load_from_dir(dir: &Path) -> Self {
        let file_path = dir.join("video_tasks.json");
        if let Ok(content) = std::fs::read_to_string(&file_path)
            && let Ok(mut store) = serde_json::from_str::<Self>(&content)
        {
            for task in &mut store.tasks {
                task.is_task_running = false;
            }
            return store;
        }
        Self::default()
    }

    pub fn save_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut temporary, self).map_err(std::io::Error::other)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(dir.join("video_tasks.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }

    pub fn add_or_update(&mut self, task: MediaTask) {
        if let Some(pos) = self.tasks.iter().position(|t| t.id == task.id) {
            self.tasks[pos] = task;
        } else {
            self.tasks.insert(0, task);
        }
    }

    pub fn delete(&mut self, id: &str) {
        self.tasks.retain(|t| t.id != id);
    }

    pub fn get(&self, id: &str) -> Option<&MediaTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut MediaTask> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_task_backwards_compatible_deserialization() {
        let json_without_media_type = r#"{
            "id": "test-123",
            "title": "Old Task",
            "source": {"LocalFile": "test.mp4"},
            "source_language": "ja",
            "target_language": "zh",
            "subtitle_mode": "RealtimeTranslation",
            "created_at_sec": 1000,
            "last_played_sec": 1000,
            "duration_ms": 5000,
            "subtitles": {"cues": [], "enabled": true}
        }"#;

        let task: MediaTask = serde_json::from_str(json_without_media_type).unwrap();
        assert_eq!(task.media_type, MediaType::Video);
    }
}
