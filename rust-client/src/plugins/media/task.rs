use super::{backend::MediaSource, subtitles::SubtitleTimeline};
use crate::client_settings::RecognitionSettings;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

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
    "mp3", "wav", "flac", "aac", "ogg", "m4a", "opus", "wma", "ape", "alac", "aiff", "aif", "oga",
];

/// Common formats in the picker; the all-files option also allows backend-supported formats.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "flv", "ts", "m4v", "mpeg", "mpg", "wmv", "mp3", "wav",
    "flac", "aac", "ogg", "m4a", "opus", "wma", "ape", "alac", "aiff", "aif", "oga", "srt", "vtt",
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
    #[serde(default)]
    pub subtitle_style: super::subtitles::SubtitleStyle,
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

pub(super) const DEFAULT_PAUSE_TOLERANCE: f32 = 0.5;

pub(super) fn default_video_recognition() -> RecognitionSettings {
    RecognitionSettings {
        background_noise: 0.6,
        pause_tolerance: DEFAULT_PAUSE_TOLERANCE,
        continuous_recognition: false,
        preprocessing: Some(xrtranslate_protocol::AudioPreprocessing::None),
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
            subtitle_style: Default::default(),
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
                task.subtitle_style.normalize();
                task.is_task_running = false;
            }
            return store;
        }
        Self::default()
    }

    pub fn save_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
        {
            let mut writer = BufWriter::new(&mut temporary);
            serde_json::to_writer(&mut writer, self).map_err(std::io::Error::other)?;
            writer.flush()?;
        }
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
    use crate::plugins::media::subtitles::{SubtitleCue, SubtitleMetadata};

    #[test]
    fn audio_processing_choice_roundtrips_without_changing_legacy_tasks() {
        let task = MediaTask::new(
            "Concert".into(),
            MediaSource::LocalFile("concert.mp4".into()),
            "ja".into(),
            "zh".into(),
            SubtitleMode::None,
            default_video_recognition(),
        );
        assert_eq!(
            task.recognition.preprocessing,
            Some(xrtranslate_protocol::AudioPreprocessing::None)
        );
        let mut legacy = serde_json::to_value(&task).unwrap();
        legacy["recognition"]
            .as_object_mut()
            .unwrap()
            .remove("preprocessing");
        let mut restored: MediaTask = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            restored.recognition,
            RecognitionSettings {
                preprocessing: None,
                ..task.recognition
            }
        );
        restored.recognition.preprocessing =
            Some(xrtranslate_protocol::AudioPreprocessing::VocalSeparation);
        let restored: MediaTask =
            serde_json::from_slice(&serde_json::to_vec(&restored).unwrap()).unwrap();
        assert_eq!(
            restored.recognition.preprocessing,
            Some(xrtranslate_protocol::AudioPreprocessing::VocalSeparation)
        );
        assert!(!restored.is_task_running);
    }

    #[test]
    fn saving_replaces_the_store_and_roundtrips_a_large_subtitle_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut task = MediaTask::new_with_media_type(
            "Subtitle document".into(),
            MediaSource::LocalFile("subtitles.srt".into()),
            MediaType::Subtitles,
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            default_video_recognition(),
        );
        for index in 0..256 {
            task.subtitles.add_cue_with_metadata(
                SubtitleCue {
                    id: format!("cue-{index}"),
                    start_ms: index * 1_000,
                    end_ms: (index + 1) * 1_000,
                    speaker_name: None,
                    original_text: format!("Line {index}: \"quoted\" \\ path\n第二行。"),
                    translated_text: Some(format!("译文 {index}\nMore translated text.")),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                },
                SubtitleMetadata::authored(),
            );
        }
        let mut store = MediaTaskStore { tasks: vec![task] };
        for title in ["Initial document", "Updated document"] {
            store.tasks[0].title = title.into();
            store.save_to_dir(directory.path()).unwrap();
            let serialized = std::fs::read(directory.path().join("video_tasks.json")).unwrap();
            assert!(serialized.len() > 8 * 1_024);
            assert_eq!(
                serde_json::to_value(MediaTaskStore::load_from_dir(directory.path())).unwrap(),
                serde_json::to_value(&store).unwrap(),
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }
}
