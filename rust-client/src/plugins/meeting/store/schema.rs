use rusqlite::Connection;

use super::model::{MeetingStoreError, Result};

const SCHEMA_VERSION: i64 = 2;

pub(super) fn migrate(connection: &Connection) -> Result<()> {
    let current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(MeetingStoreError::InvalidData(format!(
            "meeting database schema {current} is newer than supported schema {SCHEMA_VERSION}"
        )));
    }
    if current == 0 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE meetings (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL CHECK(length(trim(name)) > 0),
                 status TEXT NOT NULL CHECK(status IN
                    ('draft','live','paused','ended','interrupted','imported','processing','failed')),
                 source_kind TEXT NOT NULL CHECK(source_kind IN ('live_capture','imported_audio')),
                 input_source TEXT,
                 source_language TEXT NOT NULL,
                 target_language TEXT NOT NULL,
                 audio_source_path TEXT,
                 recording_path TEXT,
                 can_reprocess INTEGER NOT NULL DEFAULT 0 CHECK(can_reprocess IN (0,1)),
                 failure_message TEXT,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 started_at_ms INTEGER,
                 ended_at_ms INTEGER,
                 last_activity_at_ms INTEGER NOT NULL
             );
             CREATE INDEX meetings_recent_idx ON meetings(last_activity_at_ms DESC);

             CREATE TABLE meeting_topics (
                 id TEXT PRIMARY KEY,
                 meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                 sequence INTEGER NOT NULL,
                 title TEXT,
                 created_at_ms INTEGER NOT NULL,
                 UNIQUE(meeting_id, sequence)
             );
             CREATE INDEX meeting_topics_order_idx ON meeting_topics(meeting_id, sequence);

             CREATE TABLE canonical_speakers (
                 id TEXT PRIMARY KEY,
                 meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                 name TEXT NOT NULL CHECK(length(trim(name)) > 0),
                 is_provisional INTEGER NOT NULL DEFAULT 1 CHECK(is_provisional IN (0,1)),
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX canonical_speakers_meeting_idx ON canonical_speakers(meeting_id);

             CREATE TABLE speaker_aliases (
                 meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                 recognition_run_id TEXT NOT NULL,
                 speaker_token TEXT NOT NULL,
                 canonical_speaker_id TEXT NOT NULL REFERENCES canonical_speakers(id) ON DELETE CASCADE,
                 PRIMARY KEY(meeting_id, recognition_run_id, speaker_token)
             );
             CREATE INDEX speaker_aliases_canonical_idx ON speaker_aliases(canonical_speaker_id);

             CREATE TABLE meeting_segments (
                 id TEXT PRIMARY KEY,
                 external_key TEXT NOT NULL,
                 meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                 topic_id TEXT NOT NULL REFERENCES meeting_topics(id) ON DELETE CASCADE,
                 sequence INTEGER NOT NULL,
                 original_text TEXT NOT NULL,
                 translated_text TEXT,
                 start_ms INTEGER NOT NULL CHECK(start_ms >= 0),
                 end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
                 source TEXT NOT NULL CHECK(source IN ('microphone','system_audio','imported_audio')),
                 recognition_run_id TEXT NOT NULL,
                 speaker_token TEXT,
                 canonical_speaker_id TEXT REFERENCES canonical_speakers(id) ON DELETE SET NULL,
                 is_final INTEGER NOT NULL DEFAULT 1 CHECK(is_final IN (0,1)),
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 UNIQUE(topic_id, sequence),
                 UNIQUE(meeting_id, external_key)
             );
             CREATE INDEX meeting_segments_timeline_idx
                 ON meeting_segments(meeting_id, start_ms, created_at_ms);
             CREATE INDEX meeting_segments_topic_idx ON meeting_segments(topic_id, sequence);
             CREATE INDEX meeting_segments_token_idx ON meeting_segments(meeting_id, speaker_token);

             CREATE TABLE segment_markers (
                 id TEXT PRIMARY KEY,
                 segment_id TEXT NOT NULL REFERENCES meeting_segments(id) ON DELETE CASCADE,
                 kind TEXT NOT NULL CHECK(kind IN ('key_decision','action_item','note')),
                 text TEXT NOT NULL DEFAULT '',
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX segment_markers_segment_idx ON segment_markers(segment_id, created_at_ms);

             CREATE TABLE meeting_minutes (
                 meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
                 markdown TEXT NOT NULL DEFAULT '',
                 updated_at_ms INTEGER NOT NULL
             );
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    if current < 2 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE meeting_audio_runs (
                 meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                 recognition_run_id TEXT NOT NULL,
                 timeline_offset_ms INTEGER NOT NULL CHECK(timeline_offset_ms >= 0),
                 audio_source_path TEXT,
                 PRIMARY KEY(meeting_id, recognition_run_id)
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    Ok(())
}
