use std::{fmt, str::FromStr};

#[derive(Debug)]
pub enum MeetingStoreError {
    Database(rusqlite::Error),
    NotFound {
        entity: &'static str,
        id: String,
    },
    InvalidTransition {
        from: MeetingStatus,
        to: MeetingStatus,
    },
    InvalidData(String),
    LockPoisoned,
}

impl fmt::Display for MeetingStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "meeting database error: {error}"),
            Self::NotFound { entity, id } => write!(f, "{entity} not found: {id}"),
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid meeting status transition: {from} -> {to}")
            }
            Self::InvalidData(message) => f.write_str(message),
            Self::LockPoisoned => f.write_str("meeting database lock was poisoned"),
        }
    }
}

impl std::error::Error for MeetingStoreError {}

impl From<rusqlite::Error> for MeetingStoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

pub type Result<T> = std::result::Result<T, MeetingStoreError>;

macro_rules! text_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = MeetingStoreError;

            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(MeetingStoreError::InvalidData(format!(
                        "unknown {} value: {value}", stringify!($name)
                    ))),
                }
            }
        }
    };
}

text_enum!(MeetingStatus {
    Draft => "draft",
    Live => "live",
    Paused => "paused",
    Ended => "ended",
    Interrupted => "interrupted",
    Imported => "imported",
    Processing => "processing",
    Failed => "failed",
});

text_enum!(MeetingSourceKind {
    LiveCapture => "live_capture",
    ImportedAudio => "imported_audio",
});

text_enum!(SegmentSource {
    Microphone => "microphone",
    SystemAudio => "system_audio",
    ImportedAudio => "imported_audio",
});

text_enum!(MarkerKind {
    KeyDecision => "key_decision",
    ActionItem => "action_item",
    Note => "note",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub id: String,
    pub name: String,
    pub status: MeetingStatus,
    pub source_kind: MeetingSourceKind,
    pub input_source: Option<String>,
    pub source_language: String,
    pub target_language: String,
    /// Original imported file, if this meeting came from an audio file.
    pub audio_source_path: Option<String>,
    /// Retained live recording, when recording was explicitly enabled.
    pub recording_path: Option<String>,
    pub can_reprocess: bool,
    pub failure_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub last_activity_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMeeting {
    pub name: String,
    pub source_kind: MeetingSourceKind,
    pub input_source: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub audio_source_path: Option<String>,
    pub recording_path: Option<String>,
    pub can_reprocess: bool,
}

impl NewMeeting {
    pub fn live(
        name: impl Into<String>,
        input_source: Option<String>,
        source_language: impl Into<String>,
        target_language: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source_kind: MeetingSourceKind::LiveCapture,
            input_source,
            source_language: source_language.into(),
            target_language: target_language.into(),
            audio_source_path: None,
            recording_path: None,
            can_reprocess: false,
        }
    }

    pub fn imported_audio(
        name: impl Into<String>,
        audio_path: impl Into<String>,
        source_language: impl Into<String>,
        target_language: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source_kind: MeetingSourceKind::ImportedAudio,
            input_source: None,
            source_language: source_language.into(),
            target_language: target_language.into(),
            audio_source_path: Some(audio_path.into()),
            recording_path: None,
            can_reprocess: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub id: String,
    pub meeting_id: String,
    pub sequence: i64,
    pub title: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub id: String,
    /// Stable runtime/import key used to revise a partial ASR segment in place.
    pub external_key: String,
    pub meeting_id: String,
    pub topic_id: String,
    pub sequence: i64,
    pub original_text: String,
    pub translated_text: Option<String>,
    /// Actual language facts; unknown source or target fields remain empty.
    pub source_language: String,
    pub target_language: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub source: SegmentSource,
    /// Stable UUID/epoch for one diarizer lifetime or import processing pass.
    pub recognition_run_id: String,
    /// Stable anonymous token emitted by diarization, never overwritten by merges.
    pub speaker_token: Option<String>,
    /// User-facing identity assigned manually or provisionally by voice matching.
    pub canonical_speaker_id: Option<String>,
    pub is_final: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSegment {
    pub meeting_id: String,
    /// Stable across partial/final ASR revisions and unique inside a meeting.
    pub external_key: String,
    /// Topic for a new segment; revisions retain their stored topic.
    pub topic_id: String,
    pub original_text: String,
    pub translated_text: Option<String>,
    pub source_language: String,
    pub target_language: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub source: SegmentSource,
    /// Required because raw diarizer labels (for example `speaker-01`) reset
    /// whenever capture resumes or imported audio is reprocessed.
    pub recognition_run_id: String,
    pub speaker_token: Option<String>,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    pub id: String,
    pub meeting_id: String,
    pub name: String,
    pub is_provisional: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMarker {
    pub id: String,
    pub segment_id: String,
    pub kind: MarkerKind,
    pub text: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingMinutes {
    pub meeting_id: String,
    pub markdown: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRun {
    pub recognition_run_id: String,
    /// Meeting time corresponding to the beginning of this run's audio.
    pub timeline_offset_ms: i64,
    /// Original file for imports and reprocessing; live tracks use the run directory.
    pub audio_source_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingBundle {
    pub meeting: Meeting,
    pub audio_runs: Vec<AudioRun>,
    pub topics: Vec<Topic>,
    pub segments: Vec<Segment>,
    pub speakers: Vec<Speaker>,
    pub markers: Vec<SegmentMarker>,
    pub minutes: Option<MeetingMinutes>,
}
