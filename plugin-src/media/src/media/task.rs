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
    TranslateExisting,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaType {
    #[default]
    Video,
    AudioOnly,
    Subtitles,
}

pub const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "vtt"];

pub use crate::media_import::AUDIO_EXTENSIONS;

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

pub use xrtranslate_client_core::playback::AudioChannelItem;

/// The source shared by decoding, preprocessing and original-audio playback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioTrackSelection {
    #[default]
    Original,
    Vocals,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackTrack {
    Original(Option<i64>),
    Vocals,
    Background,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechTrack {
    pub path: PathBuf,
    pub language: String,
    #[serde(default)]
    pub clips: Vec<SpeechClip>,
    /// The WAV is a playback/export cache; clips are the editable source.
    #[serde(default = "speech_rendered")]
    pub rendered: bool,
}

fn speech_rendered() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpeechClip {
    pub id: String,
    pub path: PathBuf,
    pub start_ms: i64,
    pub duration_ms: i64,
    pub text: String,
    #[serde(default)]
    pub peaks: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SpeechOrigin>,
    /// One previous take is enough to compare or undo a regeneration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<SpeechClip>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpeechOrigin {
    pub cue_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectVoice {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub reference: PathBuf,
    pub range: [i64; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaTask {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) optimization: Option<super::workspace::Workspace>,
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
    #[serde(default = "super::subtitles::default_subtitle_char_limit")]
    pub subtitle_char_limit: usize,
    #[serde(default = "default_video_recognition")]
    pub recognition: RecognitionSettings,
    #[serde(default)]
    pub audio_channels: Vec<AudioChannelItem>,
    #[serde(default)]
    pub audio_track: AudioTrackSelection,
    #[serde(default)]
    pub original_audio_track: Option<i64>,
    #[serde(default)]
    pub separated_audio: Option<xrtranslate_protocol::SeparatedAudio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_track: Option<SpeechTrack>,
    #[serde(default)]
    pub speech_use_source_voice: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_reference_range: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speech_voices: Vec<ProjectVoice>,
    /// Legacy projects can recover their source references once; an intentionally
    /// emptied voice library must stay empty when the project is reopened.
    #[serde(default)]
    pub speech_voices_migrated: bool,
    #[serde(default)]
    pub play_speech_track: bool,
    /// None preserves the playback choice of tasks saved before multi-selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_tracks: Option<Vec<PlaybackTrack>>,
    #[serde(default)]
    pub audio_mix: super::mixing::MixSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_export_settings: Option<super::video_export::ExportSettings>,
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
    pub(super) fn translation_request(&self) -> super::MediaTranslationRequest {
        use super::MediaTranslationRequest;
        match &self.source {
            MediaSource::NetworkStream(_) => MediaTranslationRequest::LiveStream {
                source_language: self.source_language.clone(),
                target_language: self.target_language.clone(),
                recognition: self.recognition.clone(),
                audio_channels: self.audio_channels.clone(),
            },
            _ => MediaTranslationRequest::ImportMedia {
                source: match self.source_audio() {
                    super::backend::PlaybackAudio::External(path) => MediaSource::LocalFile(path),
                    _ => self.source.clone(),
                },
                audio_track: (self.audio_track == AudioTrackSelection::Original)
                    .then_some(self.original_audio_track)
                    .flatten(),
                source_language: self.source_language.clone(),
                target_language: self.target_language.clone(),
                recognition: self.recognition.clone(),
                audio_channels: self.audio_channels.clone(),
            },
        }
    }

    pub(super) fn translates_subtitles(&self) -> bool {
        self.media_type == MediaType::Subtitles
            || matches!(
                self.subtitle_mode,
                SubtitleMode::ImportedSrt(_) | SubtitleMode::TranslateExisting
            )
    }

    pub(super) fn playback_audio(&self) -> super::backend::PlaybackAudio {
        use super::backend::PlaybackAudio;
        let mut sources = self
            .playback_sources()
            .into_iter()
            .filter_map(|source| {
                let audio = match source {
                    PlaybackTrack::Original(id) => Some(PlaybackAudio::Original(id)),
                    PlaybackTrack::Vocals => self
                        .separated_audio
                        .as_ref()
                        .filter(|stems| stems.vocals.is_file())
                        .map(|stems| PlaybackAudio::External(stems.vocals.clone())),
                    PlaybackTrack::Background => self
                        .separated_audio
                        .as_ref()
                        .filter(|stems| stems.background.is_file())
                        .map(|stems| PlaybackAudio::External(stems.background.clone())),
                }?;
                let duck_intervals = if source == PlaybackTrack::Vocals
                    && self.audio_mix.duck_vocals
                    && self.play_speech_track
                    && self.audio_mix.speech > 0
                {
                    self.speech_track
                        .as_ref()
                        .filter(|track| track.rendered && track.path.is_file())
                        .map(|track| {
                            track
                                .clips
                                .iter()
                                .filter(|clip| clip.duration_ms > 0)
                                .map(|clip| {
                                    [
                                        clip.start_ms,
                                        clip.start_ms.saturating_add(clip.duration_ms),
                                    ]
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                Some(
                    audio.adjusted(xrtranslate_client_core::playback::AudioMixSettings {
                        volume: self.audio_mix.volume(source),
                        duck_intervals,
                    }),
                )
            })
            .collect::<Vec<_>>();
        if self.play_speech_track
            && let Some(track) = &self.speech_track
            && track.path.is_file()
        {
            sources.push(PlaybackAudio::External(track.path.clone()).adjusted(
                xrtranslate_client_core::playback::AudioMixSettings {
                    volume: self.audio_mix.speech,
                    ..Default::default()
                },
            ));
        }
        if sources.len() == 1 {
            sources.remove(0)
        } else {
            PlaybackAudio::Mix(sources)
        }
    }

    pub(super) fn playback_sources(&self) -> Vec<PlaybackTrack> {
        self.playback_tracks.clone().unwrap_or_else(|| {
            if self.play_speech_track {
                Vec::new()
            } else {
                vec![match self.audio_track {
                    AudioTrackSelection::Original => {
                        PlaybackTrack::Original(self.original_audio_track)
                    }
                    AudioTrackSelection::Vocals => PlaybackTrack::Vocals,
                    AudioTrackSelection::Background => PlaybackTrack::Background,
                }]
            }
        })
    }

    pub(super) fn plays_recognition_source(&self) -> bool {
        let source = match self.audio_track {
            AudioTrackSelection::Original => PlaybackTrack::Original(self.original_audio_track),
            AudioTrackSelection::Vocals => PlaybackTrack::Vocals,
            AudioTrackSelection::Background => PlaybackTrack::Background,
        };
        !self.play_speech_track && self.playback_sources() == [source]
    }

    pub(super) fn playback_channels(&self) -> &[AudioChannelItem] {
        if self.plays_recognition_source() {
            &self.audio_channels
        } else {
            &[]
        }
    }

    fn source_audio(&self) -> super::backend::PlaybackAudio {
        use super::backend::PlaybackAudio;
        match (self.audio_track, &self.separated_audio) {
            (AudioTrackSelection::Vocals, Some(stems)) => {
                PlaybackAudio::External(stems.vocals.clone())
            }
            (AudioTrackSelection::Background, Some(stems)) => {
                PlaybackAudio::External(stems.background.clone())
            }
            _ => PlaybackAudio::Original(self.original_audio_track),
        }
    }

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
            MediaSource::OnlineFile(_) => MediaType::AudioOnly,
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
            optimization: None,
            title,
            source,
            media_type,
            source_language: source_lang,
            target_language: target_lang,
            subtitle_mode,
            recognition,
            // Until probed, an empty selection lets the shared decoder mix every channel.
            subtitle_style: Default::default(),
            subtitle_char_limit: super::subtitles::default_subtitle_char_limit(),
            audio_channels: Vec::new(),
            audio_track: AudioTrackSelection::Original,
            original_audio_track: None,
            separated_audio: None,
            speech_track: None,
            speech_use_source_voice: false,
            speech_reference_range: None,
            speech_voices: Vec::new(),
            speech_voices_migrated: true,
            play_speech_track: false,
            playback_tracks: None,
            audio_mix: Default::default(),
            video_export_settings: None,
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
                let limits = super::subtitles::SUBTITLE_CHAR_LIMIT_RANGE;
                task.subtitle_char_limit = task
                    .subtitle_char_limit
                    .clamp(*limits.start(), *limits.end());
                task.subtitles.normalize_text();
                task.is_task_running = false;
                if let Some(workspace) = &mut task.optimization {
                    workspace.recover();
                }
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
    use crate::media::subtitles::{SubtitleCue, SubtitleMetadata};

    #[test]
    fn preview_mix_is_persisted_without_changing_recognition() {
        use super::super::backend::PlaybackAudio;
        let directory = tempfile::tempdir().unwrap();
        let mut task = MediaTask::new(
            "Preview".into(),
            MediaSource::LocalFile("video.mp4".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            default_video_recognition(),
        );
        let background = directory.path().join("background.wav");
        let speech = directory.path().join("speech.wav");
        std::fs::write(&background, []).unwrap();
        std::fs::write(&speech, []).unwrap();
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals: directory.path().join("vocals.wav"),
            background: background.clone(),
        });
        task.speech_track = Some(SpeechTrack {
            path: speech.clone(),
            language: "zh".into(),
            clips: Vec::new(),
            rendered: true,
        });
        let request = task.translation_request();
        assert_eq!(task.playback_audio(), PlaybackAudio::Original(None));
        task.play_speech_track = true;
        assert_eq!(
            task.playback_audio(),
            PlaybackAudio::External(speech.clone()),
            "legacy speech-only selection"
        );
        task.playback_tracks = Some(vec![PlaybackTrack::Background]);
        let store = MediaTaskStore { tasks: vec![task] };
        store.save_to_dir(directory.path()).unwrap();
        let mut restored = MediaTaskStore::load_from_dir(directory.path())
            .tasks
            .remove(0);
        assert_eq!(
            restored.playback_audio(),
            PlaybackAudio::Mix(vec![
                PlaybackAudio::External(background.clone()),
                PlaybackAudio::External(speech),
            ])
        );
        assert_eq!(restored.translation_request(), request);
        assert!(!restored.plays_recognition_source());
        restored.play_speech_track = false;
        assert_eq!(
            restored.playback_audio(),
            PlaybackAudio::External(background)
        );
        restored.playback_tracks = Some(Vec::new());
        assert_eq!(restored.playback_audio(), PlaybackAudio::Mix(Vec::new()));
        assert_eq!(restored.translation_request(), request);
    }

    #[test]
    fn vocal_ducking_follows_audible_clips_and_keeps_recognition_unchanged() {
        use xrtranslate_client_core::playback::{AudioMixSettings, PlaybackAudio};
        let dir = tempfile::tempdir().unwrap();
        let vocals = dir.path().join("vocals.wav");
        let speech = dir.path().join("speech.wav");
        for path in [&vocals, &speech] {
            std::fs::write(path, []).unwrap();
        }
        let mut task = MediaTask::new(
            "Mix".into(),
            MediaSource::LocalFile("video.mp4".into()),
            "en".into(),
            "zh".into(),
            SubtitleMode::None,
            default_video_recognition(),
        );
        let recognition = task.translation_request();
        task.separated_audio = Some(xrtranslate_protocol::SeparatedAudio {
            vocals: vocals.clone(),
            background: dir.path().join("background.wav"),
        });
        task.playback_tracks = Some(vec![PlaybackTrack::Vocals]);
        task.play_speech_track = true;
        task.audio_mix.vocals = 70;
        task.audio_mix.speech = 80;
        task.audio_mix.duck_vocals = true;
        task.speech_track = Some(SpeechTrack {
            path: speech.clone(),
            language: "zh".into(),
            rendered: true,
            clips: vec![SpeechClip {
                id: "one".into(),
                path: speech,
                start_ms: 400,
                duration_ms: 600,
                text: "Hello".into(),
                peaks: Vec::new(),
                ..Default::default()
            }],
        });
        let vocal_settings = |task: &MediaTask| {
            let PlaybackAudio::Mix(tracks) = task.playback_audio() else {
                panic!("expected mix")
            };
            let PlaybackAudio::Adjusted { settings, .. } = &tracks[0] else {
                panic!("expected vocal gain")
            };
            settings.clone()
        };
        assert_eq!(
            vocal_settings(&task),
            AudioMixSettings {
                volume: 70,
                duck_intervals: vec![[400, 1000]]
            }
        );
        task.speech_track.as_mut().unwrap().clips[0].start_ms = 1000;
        assert_eq!(vocal_settings(&task).duck_intervals, [[1000, 1600]]);
        task.audio_mix.speech = 0;
        assert!(vocal_settings(&task).duck_intervals.is_empty());
        task.audio_mix.speech = 80;
        task.speech_track.as_mut().unwrap().clips.clear();
        assert!(vocal_settings(&task).duck_intervals.is_empty());
        let restored: MediaTask =
            serde_json::from_value(serde_json::to_value(&task).unwrap()).unwrap();
        assert_eq!(restored.audio_mix, task.audio_mix);
        assert_eq!(restored.translation_request(), recognition);
        // Older projects preserve their sound without a migration step.
        let mut old = serde_json::to_value(&task).unwrap();
        old.as_object_mut().unwrap().remove("audio_mix");
        assert_eq!(
            serde_json::from_value::<MediaTask>(old).unwrap().audio_mix,
            Default::default()
        );
    }

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
        legacy
            .as_object_mut()
            .unwrap()
            .remove("subtitle_char_limit");
        legacy["recognition"]
            .as_object_mut()
            .unwrap()
            .remove("preprocessing");
        let mut restored: MediaTask = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.subtitle_char_limit, 200);
        restored.subtitle_char_limit = 160;
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
        assert_eq!(restored.subtitle_char_limit, 160);
        assert_eq!(
            restored.recognition.preprocessing,
            Some(xrtranslate_protocol::AudioPreprocessing::VocalSeparation)
        );
        assert!(!restored.is_task_running);
    }

    #[test]
    fn loading_legacy_tasks_cleans_paragraph_breaks_before_display() {
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
        task.subtitles =
            SubtitleTimeline::parse("1\n00:00:01,000 --> 00:00:02,000\nFirst.\n").unwrap();
        let mut legacy = serde_json::to_value(MediaTaskStore { tasks: vec![task] }).unwrap();
        let cue = &mut legacy["tasks"][0]["subtitles"]["cues"][0];
        cue["original_text"] = "First.\n\nSecond.".into();
        cue["translated_text"] = "第一句。\r\r第二句。".into();
        std::fs::write(
            directory.path().join("video_tasks.json"),
            legacy.to_string(),
        )
        .unwrap();
        let restored = MediaTaskStore::load_from_dir(directory.path());
        let cue = &restored.tasks[0].subtitles.cues()[0];
        assert_eq!(cue.original_text, "First.\nSecond.");
        assert_eq!(cue.translated_text.as_deref(), Some("第一句。\n第二句。"));
        assert_eq!((cue.start_ms, cue.end_ms), (1000, 2000));
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
