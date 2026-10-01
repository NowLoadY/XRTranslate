use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    path::Path,
    str::FromStr,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[cfg(test)]
use super::export::render_markdown;
use super::{model::*, schema::migrate};

/// A single shareable store. The mutex serializes writes on one SQLite connection;
/// SQLite WAL still allows other processes/connections to read concurrently.
pub struct MeetingStore {
    connection: Mutex<Connection>,
}

// The repository intentionally exposes the complete meeting persistence API.
// Some operations are exercised only by tests or optional UI workflows.
#[allow(dead_code)]
impl MeetingStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;
        migrate(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| MeetingStoreError::LockPoisoned)
    }

    pub fn create_meeting(&self, new_meeting: NewMeeting) -> Result<MeetingBundle> {
        validate_new_meeting(&new_meeting)?;
        let now = now_ms();
        let meeting_id = new_id();
        let topic_id = new_id();
        let initial_status = match new_meeting.source_kind {
            MeetingSourceKind::LiveCapture => MeetingStatus::Draft,
            MeetingSourceKind::ImportedAudio => MeetingStatus::Imported,
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO meetings (
                 id, name, status, source_kind, input_source, source_language,
                 target_language, audio_source_path, recording_path, can_reprocess,
                 created_at_ms, updated_at_ms, last_activity_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?11)",
            params![
                meeting_id,
                new_meeting.name.trim(),
                initial_status.as_str(),
                new_meeting.source_kind.as_str(),
                new_meeting.input_source,
                new_meeting.source_language,
                new_meeting.target_language,
                new_meeting.audio_source_path,
                new_meeting.recording_path,
                new_meeting.can_reprocess,
                now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO meeting_topics (id, meeting_id, sequence, title, created_at_ms)
             VALUES (?1, ?2, 0, NULL, ?3)",
            params![topic_id, meeting_id, now],
        )?;
        transaction.commit()?;
        drop(connection);
        self.open_meeting(&meeting_id)
    }

    pub fn get_meeting(&self, meeting_id: &str) -> Result<Meeting> {
        let connection = self.connection()?;
        query_meeting(&connection, meeting_id)
    }

    pub fn open_meeting(&self, meeting_id: &str) -> Result<MeetingBundle> {
        let connection = self.connection()?;
        let meeting = query_meeting(&connection, meeting_id)?;
        let topics = query_topics(&connection, meeting_id)?;
        let segments = query_segments(&connection, meeting_id, None)?;
        let speakers = query_speakers(&connection, meeting_id)?;
        let markers = query_markers(&connection, meeting_id)?;
        let minutes = query_minutes(&connection, meeting_id)?;
        Ok(MeetingBundle {
            meeting,
            topics,
            segments,
            speakers,
            markers,
            minutes,
        })
    }

    pub fn list_meetings(&self, limit: usize) -> Result<Vec<Meeting>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, name, status, source_kind, input_source, source_language,
                    target_language, audio_source_path, recording_path, can_reprocess,
                    failure_message, created_at_ms, updated_at_ms, started_at_ms,
                    ended_at_ms, last_activity_at_ms
             FROM meetings
             ORDER BY last_activity_at_ms DESC, created_at_ms DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map(params![usize_to_i64(limit)], map_meeting_row)?;
        collect_rows(rows)
    }

    pub fn rename_meeting(&self, meeting_id: &str, name: &str) -> Result<()> {
        require_non_empty("meeting name", name)?;
        let now = now_ms();
        let changed = self.connection()?.execute(
            "UPDATE meetings SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![name.trim(), now, meeting_id],
        )?;
        require_changed(changed, "meeting", meeting_id)
    }

    pub fn delete_meeting(&self, meeting_id: &str) -> Result<()> {
        let changed = self
            .connection()?
            .execute("DELETE FROM meetings WHERE id = ?1", params![meeting_id])?;
        require_changed(changed, "meeting", meeting_id)
    }

    pub fn start_meeting(&self, meeting_id: &str) -> Result<Meeting> {
        let meeting = self.get_meeting(meeting_id)?;
        let target = match meeting.source_kind {
            MeetingSourceKind::LiveCapture => MeetingStatus::Live,
            MeetingSourceKind::ImportedAudio => MeetingStatus::Processing,
        };
        self.transition_status(meeting_id, target, None)
    }

    pub fn pause_meeting(&self, meeting_id: &str) -> Result<Meeting> {
        self.transition_status(meeting_id, MeetingStatus::Paused, None)
    }

    pub fn resume_meeting(&self, meeting_id: &str) -> Result<Meeting> {
        self.start_meeting(meeting_id)
    }

    pub fn end_meeting(&self, meeting_id: &str) -> Result<Meeting> {
        self.transition_status(meeting_id, MeetingStatus::Ended, None)
    }

    pub fn fail_meeting(&self, meeting_id: &str, message: impl Into<String>) -> Result<Meeting> {
        self.transition_status(meeting_id, MeetingStatus::Failed, Some(message.into()))
    }

    /// Marks sessions that could have been actively writing when the process died.
    /// Paused sessions remain paused and are already safe to reopen/resume.
    pub fn recover_interrupted_meetings(&self) -> Result<Vec<String>> {
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT id FROM meetings WHERE status IN ('live', 'processing') ORDER BY id",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        transaction.execute(
            "UPDATE meetings
             SET status = 'interrupted', updated_at_ms = ?1, last_activity_at_ms = ?1
             WHERE status IN ('live', 'processing')",
            params![now],
        )?;
        transaction.commit()?;
        Ok(ids)
    }

    fn transition_status(
        &self,
        meeting_id: &str,
        target: MeetingStatus,
        failure_message: Option<String>,
    ) -> Result<Meeting> {
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = query_meeting(&transaction, meeting_id)?;
        if current.status != target && !is_valid_transition(current.status, target) {
            return Err(MeetingStoreError::InvalidTransition {
                from: current.status,
                to: target,
            });
        }
        let started_at = if matches!(target, MeetingStatus::Live | MeetingStatus::Processing)
            && current.started_at_ms.is_none()
        {
            Some(now)
        } else {
            current.started_at_ms
        };
        let ended_at = if target == MeetingStatus::Ended {
            Some(now)
        } else if matches!(target, MeetingStatus::Live | MeetingStatus::Processing) {
            None
        } else {
            current.ended_at_ms
        };
        transaction.execute(
            "UPDATE meetings
             SET status = ?1, failure_message = ?2, started_at_ms = ?3, ended_at_ms = ?4,
                 updated_at_ms = ?5, last_activity_at_ms = ?5
             WHERE id = ?6",
            params![
                target.as_str(),
                failure_message,
                started_at,
                ended_at,
                now,
                meeting_id
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        self.get_meeting(meeting_id)
    }

    pub fn create_topic(&self, meeting_id: &str, title: Option<&str>) -> Result<Topic> {
        let now = now_ms();
        let topic_id = new_id();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_meeting_exists(&transaction, meeting_id)?;
        let sequence: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM meeting_topics WHERE meeting_id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO meeting_topics (id, meeting_id, sequence, title, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                topic_id,
                meeting_id,
                sequence,
                normalize_optional(title),
                now
            ],
        )?;
        touch_meeting(&transaction, meeting_id, now)?;
        transaction.commit()?;
        Ok(Topic {
            id: topic_id,
            meeting_id: meeting_id.to_owned(),
            sequence,
            title: normalize_optional(title).map(ToOwned::to_owned),
            created_at_ms: now,
        })
    }

    pub fn rename_topic(&self, topic_id: &str, title: Option<&str>) -> Result<()> {
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let meeting_id: Option<String> = transaction
            .query_row(
                "SELECT meeting_id FROM meeting_topics WHERE id = ?1",
                params![topic_id],
                |row| row.get(0),
            )
            .optional()?;
        let meeting_id = meeting_id.ok_or_else(|| not_found("topic", topic_id))?;
        transaction.execute(
            "UPDATE meeting_topics SET title = ?1 WHERE id = ?2",
            params![normalize_optional(title), topic_id],
        )?;
        touch_meeting(&transaction, &meeting_id, now)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_topics(&self, meeting_id: &str) -> Result<Vec<Topic>> {
        let connection = self.connection()?;
        query_topics(&connection, meeting_id)
    }

    /// Inserts a new ASR segment, or revises the existing segment with the same
    /// `(meeting_id, external_key)`. Revisions preserve database id, topic and order.
    pub fn upsert_segment(&self, new_segment: NewSegment) -> Result<Segment> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let segment = upsert_segment_in(&transaction, new_segment, now_ms())?;
        transaction.commit()?;
        Ok(segment)
    }

    /// Atomically replaces only keys owned by the previous live batch. Annotated
    /// rows remain available as evidence even when recognition removes them.
    pub fn replace_segments(
        &self,
        segments: Vec<NewSegment>,
        previous_keys: &[String],
    ) -> Result<()> {
        let Some(first) = segments.first() else {
            return Ok(());
        };
        let meeting = first.meeting_id.clone();
        let topic = first.topic_id.clone();
        let run = first.recognition_run_id.clone();
        if segments.iter().any(|segment| {
            segment.meeting_id != meeting
                || segment.topic_id != topic
                || segment.recognition_run_id != run
        }) {
            return Err(MeetingStoreError::InvalidData(
                "A segment batch must belong to one recording and topic".into(),
            ));
        }
        let retained: std::collections::HashSet<_> = segments
            .iter()
            .map(|segment| segment.external_key.clone())
            .collect();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = now_ms();
        for segment in segments {
            upsert_segment_in(&transaction, segment, now)?;
        }
        for key in previous_keys.iter().filter(|key| !retained.contains(*key)) {
            transaction.execute(
                "DELETE FROM meeting_segments WHERE meeting_id = ?1 AND topic_id = ?2
                 AND recognition_run_id = ?3 AND external_key = ?4
                 AND NOT EXISTS (SELECT 1 FROM segment_markers WHERE segment_id = meeting_segments.id)",
                params![meeting, topic, run, key],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Compatibility alias with upsert semantics; callers should prefer
    /// `upsert_segment` to make streaming revision behavior explicit.
    pub fn append_segment(&self, new_segment: NewSegment) -> Result<Segment> {
        self.upsert_segment(new_segment)
    }

    pub fn update_segment_text(
        &self,
        segment_id: &str,
        original_text: &str,
        translated_text: Option<&str>,
        is_final: bool,
    ) -> Result<()> {
        require_non_empty("segment original text", original_text)?;
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let meeting_id: Option<String> = transaction
            .query_row(
                "SELECT meeting_id FROM meeting_segments WHERE id = ?1",
                params![segment_id],
                |row| row.get(0),
            )
            .optional()?;
        let meeting_id = meeting_id.ok_or_else(|| not_found("segment", segment_id))?;
        transaction.execute(
            "UPDATE meeting_segments
             SET original_text = ?1, translated_text = ?2, is_final = ?3, updated_at_ms = ?4
             WHERE id = ?5",
            params![original_text, translated_text, is_final, now, segment_id],
        )?;
        touch_meeting(&transaction, &meeting_id, now)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_segments(&self, meeting_id: &str) -> Result<Vec<Segment>> {
        let connection = self.connection()?;
        query_segments(&connection, meeting_id, None)
    }

    pub fn list_topic_segments(&self, meeting_id: &str, topic_id: &str) -> Result<Vec<Segment>> {
        let connection = self.connection()?;
        query_segments(&connection, meeting_id, Some(topic_id))
    }

    /// Creates (or reuses) a provisional speaker and binds the diarization token.
    /// Existing segments carrying that immutable token are reconciled atomically.
    pub fn assign_speaker_token(
        &self,
        meeting_id: &str,
        recognition_run_id: &str,
        speaker_token: &str,
        suggested_name: &str,
    ) -> Result<Speaker> {
        require_non_empty("speaker token", speaker_token)?;
        require_non_empty("recognition run id", recognition_run_id)?;
        require_non_empty("speaker name", suggested_name)?;
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_meeting_exists(&transaction, meeting_id)?;
        let existing_id: Option<String> = transaction
            .query_row(
                "SELECT canonical_speaker_id FROM speaker_aliases
                 WHERE meeting_id = ?1 AND recognition_run_id = ?2 AND speaker_token = ?3",
                params![meeting_id, recognition_run_id, speaker_token],
                |row| row.get(0),
            )
            .optional()?;
        let speaker_id = existing_id.unwrap_or_else(new_id);
        transaction.execute(
            "INSERT OR IGNORE INTO canonical_speakers
                 (id, meeting_id, name, is_provisional, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)",
            params![speaker_id, meeting_id, suggested_name.trim(), now],
        )?;
        transaction.execute(
            "INSERT INTO speaker_aliases
                 (meeting_id, recognition_run_id, speaker_token, canonical_speaker_id)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(meeting_id, recognition_run_id, speaker_token)
             DO UPDATE SET canonical_speaker_id = excluded.canonical_speaker_id",
            params![meeting_id, recognition_run_id, speaker_token, speaker_id],
        )?;
        transaction.execute(
            "UPDATE meeting_segments SET canonical_speaker_id = ?1, updated_at_ms = ?2
             WHERE meeting_id = ?3 AND recognition_run_id = ?4 AND speaker_token = ?5",
            params![
                speaker_id,
                now,
                meeting_id,
                recognition_run_id,
                speaker_token
            ],
        )?;
        touch_meeting(&transaction, meeting_id, now)?;
        let speaker = query_speaker(&transaction, &speaker_id)?;
        transaction.commit()?;
        Ok(speaker)
    }

    pub fn rename_speaker(&self, speaker_id: &str, name: &str) -> Result<Speaker> {
        require_non_empty("speaker name", name)?;
        let now = now_ms();
        let changed = self.connection()?.execute(
            "UPDATE canonical_speakers
             SET name = ?1, is_provisional = 0, updated_at_ms = ?2
             WHERE id = ?3",
            params![name.trim(), now, speaker_id],
        )?;
        require_changed(changed, "speaker", speaker_id)?;
        let connection = self.connection()?;
        query_speaker(&connection, speaker_id)
    }

    /// Moves every token and segment from `source_speaker_id` into the target,
    /// then removes the now-unreferenced source identity in one transaction.
    pub fn merge_speakers(
        &self,
        source_speaker_id: &str,
        target_speaker_id: &str,
    ) -> Result<Speaker> {
        if source_speaker_id == target_speaker_id {
            let connection = self.connection()?;
            return query_speaker(&connection, target_speaker_id);
        }
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source = query_speaker(&transaction, source_speaker_id)?;
        let target = query_speaker(&transaction, target_speaker_id)?;
        if source.meeting_id != target.meeting_id {
            return Err(MeetingStoreError::InvalidData(
                "cannot merge speakers from different meetings".to_owned(),
            ));
        }
        transaction.execute(
            "UPDATE speaker_aliases SET canonical_speaker_id = ?1 WHERE canonical_speaker_id = ?2",
            params![target_speaker_id, source_speaker_id],
        )?;
        transaction.execute(
            "UPDATE meeting_segments SET canonical_speaker_id = ?1, updated_at_ms = ?2
             WHERE canonical_speaker_id = ?3",
            params![target_speaker_id, now, source_speaker_id],
        )?;
        transaction.execute(
            "DELETE FROM canonical_speakers WHERE id = ?1",
            params![source_speaker_id],
        )?;
        touch_meeting(&transaction, &source.meeting_id, now)?;
        let merged = query_speaker(&transaction, target_speaker_id)?;
        transaction.commit()?;
        Ok(merged)
    }

    pub fn list_speakers(&self, meeting_id: &str) -> Result<Vec<Speaker>> {
        let connection = self.connection()?;
        query_speakers(&connection, meeting_id)
    }

    pub fn add_marker(
        &self,
        segment_id: &str,
        kind: MarkerKind,
        text: &str,
    ) -> Result<SegmentMarker> {
        let now = now_ms();
        let marker_id = new_id();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let meeting_id: Option<String> = transaction
            .query_row(
                "SELECT meeting_id FROM meeting_segments WHERE id = ?1",
                params![segment_id],
                |row| row.get(0),
            )
            .optional()?;
        let meeting_id = meeting_id.ok_or_else(|| not_found("segment", segment_id))?;
        transaction.execute(
            "INSERT INTO segment_markers
                 (id, segment_id, kind, text, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![marker_id, segment_id, kind.as_str(), text, now],
        )?;
        touch_meeting(&transaction, &meeting_id, now)?;
        transaction.commit()?;
        Ok(SegmentMarker {
            id: marker_id,
            segment_id: segment_id.to_owned(),
            kind,
            text: text.to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    pub fn update_marker(&self, marker_id: &str, kind: MarkerKind, text: &str) -> Result<()> {
        let now = now_ms();
        let changed = self.connection()?.execute(
            "UPDATE segment_markers SET kind = ?1, text = ?2, updated_at_ms = ?3 WHERE id = ?4",
            params![kind.as_str(), text, now, marker_id],
        )?;
        require_changed(changed, "marker", marker_id)
    }

    pub fn delete_marker(&self, marker_id: &str) -> Result<()> {
        let changed = self.connection()?.execute(
            "DELETE FROM segment_markers WHERE id = ?1",
            params![marker_id],
        )?;
        require_changed(changed, "marker", marker_id)
    }

    pub fn save_minutes(&self, meeting_id: &str, markdown: &str) -> Result<MeetingMinutes> {
        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_meeting_exists(&transaction, meeting_id)?;
        transaction.execute(
            "INSERT INTO meeting_minutes (meeting_id, markdown, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(meeting_id)
             DO UPDATE SET markdown = excluded.markdown, updated_at_ms = excluded.updated_at_ms",
            params![meeting_id, markdown, now],
        )?;
        touch_meeting(&transaction, meeting_id, now)?;
        transaction.commit()?;
        Ok(MeetingMinutes {
            meeting_id: meeting_id.to_owned(),
            markdown: markdown.to_owned(),
            updated_at_ms: now,
        })
    }

    pub fn get_minutes(&self, meeting_id: &str) -> Result<Option<MeetingMinutes>> {
        let connection = self.connection()?;
        ensure_meeting_exists(&connection, meeting_id)?;
        query_minutes(&connection, meeting_id)
    }
}

fn upsert_segment_in(
    transaction: &Transaction<'_>,
    mut new_segment: NewSegment,
    now: i64,
) -> Result<Segment> {
    validate_segment(&new_segment)?;
    ensure_meeting_exists(&transaction, &new_segment.meeting_id)?;
    ensure_topic_belongs_to(&transaction, &new_segment.topic_id, &new_segment.meeting_id)?;
    let existing: Option<(String, String, i64, i64, String, Option<String>)> = transaction
        .query_row(
            "SELECT id, topic_id, sequence, created_at_ms, original_text, translated_text FROM meeting_segments
             WHERE meeting_id = ?1 AND external_key = ?2",
            params![new_segment.meeting_id, new_segment.external_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()?;
    if let Some((_, existing_topic_id, ..)) = &existing {
        if existing_topic_id != &new_segment.topic_id {
            return Err(MeetingStoreError::InvalidData(format!(
                "segment {} already belongs to topic {existing_topic_id}",
                new_segment.external_key
            )));
        }
    }
    if new_segment.translated_text.is_none()
        && let Some((_, _, _, _, source, translated)) = &existing
        && *source == new_segment.original_text
    {
        new_segment.translated_text = translated.clone();
    }
    let segment_id = existing
        .as_ref()
        .map(|item| item.0.clone())
        .unwrap_or_else(new_id);
    let sequence: i64 = match existing.as_ref() {
        Some(item) => item.2,
        None => transaction.query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM meeting_segments WHERE topic_id = ?1",
            params![new_segment.topic_id],
            |row| row.get(0),
        )?,
    };
    let created_at_ms = existing.as_ref().map(|item| item.3).unwrap_or(now);
    let canonical_speaker_id = match new_segment.speaker_token.as_deref() {
        Some(token) => transaction
            .query_row(
                "SELECT canonical_speaker_id FROM speaker_aliases
                 WHERE meeting_id = ?1 AND recognition_run_id = ?2 AND speaker_token = ?3",
                params![
                    new_segment.meeting_id,
                    new_segment.recognition_run_id,
                    token
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        None => None,
    };
    if existing.is_some() {
        transaction.execute(
            "UPDATE meeting_segments
             SET original_text = ?1, translated_text = ?2, start_ms = ?3, end_ms = ?4,
                 source = ?5, recognition_run_id = ?6, speaker_token = ?7,
                 canonical_speaker_id = ?8, is_final = ?9, updated_at_ms = ?10
             WHERE id = ?11",
            params![
                new_segment.original_text,
                new_segment.translated_text,
                new_segment.start_ms,
                new_segment.end_ms,
                new_segment.source.as_str(),
                new_segment.recognition_run_id,
                new_segment.speaker_token,
                canonical_speaker_id,
                new_segment.is_final,
                now,
                segment_id,
            ],
        )?;
    } else {
        transaction.execute(
            "INSERT INTO meeting_segments (
                 id, external_key, meeting_id, topic_id, sequence, original_text, translated_text,
                 start_ms, end_ms, source, recognition_run_id, speaker_token, canonical_speaker_id,
                 is_final, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            params![
                segment_id,
                new_segment.external_key,
                new_segment.meeting_id,
                new_segment.topic_id,
                sequence,
                new_segment.original_text,
                new_segment.translated_text,
                new_segment.start_ms,
                new_segment.end_ms,
                new_segment.source.as_str(),
                new_segment.recognition_run_id,
                new_segment.speaker_token,
                canonical_speaker_id,
                new_segment.is_final,
                now,
            ],
        )?;
    }
    touch_meeting(&transaction, &new_segment.meeting_id, now)?;
    Ok(Segment {
        id: segment_id,
        external_key: new_segment.external_key,
        meeting_id: new_segment.meeting_id,
        topic_id: new_segment.topic_id,
        sequence,
        original_text: new_segment.original_text,
        translated_text: new_segment.translated_text,
        start_ms: new_segment.start_ms,
        end_ms: new_segment.end_ms,
        source: new_segment.source,
        recognition_run_id: new_segment.recognition_run_id,
        speaker_token: new_segment.speaker_token,
        canonical_speaker_id,
        is_final: new_segment.is_final,
        created_at_ms,
        updated_at_ms: now,
    })
}

fn query_meeting(connection: &Connection, meeting_id: &str) -> Result<Meeting> {
    connection
        .query_row(
            "SELECT id, name, status, source_kind, input_source, source_language,
                    target_language, audio_source_path, recording_path, can_reprocess,
                    failure_message, created_at_ms, updated_at_ms, started_at_ms,
                    ended_at_ms, last_activity_at_ms
             FROM meetings WHERE id = ?1",
            params![meeting_id],
            map_meeting_row,
        )
        .optional()?
        .ok_or_else(|| not_found("meeting", meeting_id))
}

fn map_meeting_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: row.get(0)?,
        name: row.get(1)?,
        status: parse_db_enum(row.get::<_, String>(2)?, 2)?,
        source_kind: parse_db_enum(row.get::<_, String>(3)?, 3)?,
        input_source: row.get(4)?,
        source_language: row.get(5)?,
        target_language: row.get(6)?,
        audio_source_path: row.get(7)?,
        recording_path: row.get(8)?,
        can_reprocess: row.get(9)?,
        failure_message: row.get(10)?,
        created_at_ms: row.get(11)?,
        updated_at_ms: row.get(12)?,
        started_at_ms: row.get(13)?,
        ended_at_ms: row.get(14)?,
        last_activity_at_ms: row.get(15)?,
    })
}

fn query_topics(connection: &Connection, meeting_id: &str) -> Result<Vec<Topic>> {
    ensure_meeting_exists(connection, meeting_id)?;
    let mut statement = connection.prepare(
        "SELECT id, meeting_id, sequence, title, created_at_ms
         FROM meeting_topics WHERE meeting_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement.query_map(params![meeting_id], |row| {
        Ok(Topic {
            id: row.get(0)?,
            meeting_id: row.get(1)?,
            sequence: row.get(2)?,
            title: row.get(3)?,
            created_at_ms: row.get(4)?,
        })
    })?;
    collect_rows(rows)
}

fn query_segments(
    connection: &Connection,
    meeting_id: &str,
    topic_id: Option<&str>,
) -> Result<Vec<Segment>> {
    ensure_meeting_exists(connection, meeting_id)?;
    if let Some(topic_id) = topic_id {
        ensure_topic_belongs_to(connection, topic_id, meeting_id)?;
    }
    let sql = if topic_id.is_some() {
        "SELECT s.id, s.external_key, s.meeting_id, s.topic_id, s.sequence, s.original_text,
                s.translated_text, s.start_ms, s.end_ms, s.source, s.recognition_run_id, s.speaker_token,
                s.canonical_speaker_id, s.is_final, s.created_at_ms, s.updated_at_ms
         FROM meeting_segments s
         WHERE s.meeting_id = ?1 AND s.topic_id = ?2
         ORDER BY s.sequence"
    } else {
        "SELECT s.id, s.external_key, s.meeting_id, s.topic_id, s.sequence, s.original_text,
                s.translated_text, s.start_ms, s.end_ms, s.source, s.recognition_run_id, s.speaker_token,
                s.canonical_speaker_id, s.is_final, s.created_at_ms, s.updated_at_ms
         FROM meeting_segments s
         JOIN meeting_topics t ON t.id = s.topic_id
         WHERE s.meeting_id = ?1
         ORDER BY t.sequence, s.sequence"
    };
    let mut statement = connection.prepare(sql)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(Segment {
            id: row.get(0)?,
            external_key: row.get(1)?,
            meeting_id: row.get(2)?,
            topic_id: row.get(3)?,
            sequence: row.get(4)?,
            original_text: row.get(5)?,
            translated_text: row.get(6)?,
            start_ms: row.get(7)?,
            end_ms: row.get(8)?,
            source: parse_db_enum(row.get::<_, String>(9)?, 9)?,
            recognition_run_id: row.get(10)?,
            speaker_token: row.get(11)?,
            canonical_speaker_id: row.get(12)?,
            is_final: row.get(13)?,
            created_at_ms: row.get(14)?,
            updated_at_ms: row.get(15)?,
        })
    };
    let rows = if let Some(topic_id) = topic_id {
        statement.query_map(params![meeting_id, topic_id], map)?
    } else {
        statement.query_map(params![meeting_id], map)?
    };
    collect_rows(rows)
}

fn query_speaker(connection: &Connection, speaker_id: &str) -> Result<Speaker> {
    connection
        .query_row(
            "SELECT id, meeting_id, name, is_provisional, created_at_ms, updated_at_ms
             FROM canonical_speakers WHERE id = ?1",
            params![speaker_id],
            |row| {
                Ok(Speaker {
                    id: row.get(0)?,
                    meeting_id: row.get(1)?,
                    name: row.get(2)?,
                    is_provisional: row.get(3)?,
                    created_at_ms: row.get(4)?,
                    updated_at_ms: row.get(5)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| not_found("speaker", speaker_id))
}

fn query_speakers(connection: &Connection, meeting_id: &str) -> Result<Vec<Speaker>> {
    let mut statement = connection.prepare(
        "SELECT id, meeting_id, name, is_provisional, created_at_ms, updated_at_ms
         FROM canonical_speakers WHERE meeting_id = ?1 ORDER BY created_at_ms, id",
    )?;
    let rows = statement.query_map(params![meeting_id], |row| {
        Ok(Speaker {
            id: row.get(0)?,
            meeting_id: row.get(1)?,
            name: row.get(2)?,
            is_provisional: row.get(3)?,
            created_at_ms: row.get(4)?,
            updated_at_ms: row.get(5)?,
        })
    })?;
    collect_rows(rows)
}

fn query_markers(connection: &Connection, meeting_id: &str) -> Result<Vec<SegmentMarker>> {
    let mut statement = connection.prepare(
        "SELECT m.id, m.segment_id, m.kind, m.text, m.created_at_ms, m.updated_at_ms
         FROM segment_markers m
         JOIN meeting_segments s ON s.id = m.segment_id
         JOIN meeting_topics t ON t.id = s.topic_id
         WHERE s.meeting_id = ?1
         ORDER BY t.sequence, s.sequence, m.created_at_ms",
    )?;
    let rows = statement.query_map(params![meeting_id], |row| {
        Ok(SegmentMarker {
            id: row.get(0)?,
            segment_id: row.get(1)?,
            kind: parse_db_enum(row.get::<_, String>(2)?, 2)?,
            text: row.get(3)?,
            created_at_ms: row.get(4)?,
            updated_at_ms: row.get(5)?,
        })
    })?;
    collect_rows(rows)
}

fn query_minutes(connection: &Connection, meeting_id: &str) -> Result<Option<MeetingMinutes>> {
    Ok(connection
        .query_row(
            "SELECT meeting_id, markdown, updated_at_ms FROM meeting_minutes WHERE meeting_id = ?1",
            params![meeting_id],
            |row| {
                Ok(MeetingMinutes {
                    meeting_id: row.get(0)?,
                    markdown: row.get(1)?,
                    updated_at_ms: row.get(2)?,
                })
            },
        )
        .optional()?)
}

fn parse_db_enum<T: FromStr<Err = MeetingStoreError>>(
    value: String,
    column: usize,
) -> rusqlite::Result<T> {
    value.parse().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn validate_new_meeting(new_meeting: &NewMeeting) -> Result<()> {
    require_non_empty("meeting name", &new_meeting.name)?;
    require_non_empty("source language", &new_meeting.source_language)?;
    require_non_empty("target language", &new_meeting.target_language)?;
    if new_meeting.source_kind == MeetingSourceKind::ImportedAudio
        && normalize_optional(new_meeting.audio_source_path.as_deref()).is_none()
    {
        return Err(MeetingStoreError::InvalidData(
            "imported meetings require an audio source path".to_owned(),
        ));
    }
    if new_meeting.can_reprocess
        && new_meeting.audio_source_path.is_none()
        && new_meeting.recording_path.is_none()
    {
        return Err(MeetingStoreError::InvalidData(
            "reprocessing requires an imported audio or retained recording path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_segment(segment: &NewSegment) -> Result<()> {
    require_non_empty("segment original text", &segment.original_text)?;
    require_non_empty("segment external key", &segment.external_key)?;
    require_non_empty("recognition run id", &segment.recognition_run_id)?;
    if segment.start_ms < 0 || segment.end_ms < segment.start_ms {
        return Err(MeetingStoreError::InvalidData(
            "segment timestamps must satisfy 0 <= start_ms <= end_ms".to_owned(),
        ));
    }
    Ok(())
}

fn is_valid_transition(from: MeetingStatus, to: MeetingStatus) -> bool {
    use MeetingStatus::*;
    matches!(
        (from, to),
        (Draft, Live)
            | (Draft, Ended)
            | (Live, Paused)
            | (Live, Ended)
            | (Live, Interrupted)
            | (Paused, Live)
            | (Paused, Ended)
            | (Paused, Interrupted)
            | (Ended, Live)
            | (Ended, Processing)
            | (Imported, Processing)
            | (Imported, Ended)
            | (Processing, Ended)
            | (Processing, Failed)
            | (Processing, Interrupted)
            | (Interrupted, Live)
            | (Interrupted, Processing)
            | (Interrupted, Ended)
            | (Failed, Processing)
            | (Failed, Ended)
    )
}

fn ensure_meeting_exists(connection: &Connection, meeting_id: &str) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?1)",
        params![meeting_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(not_found("meeting", meeting_id))
    }
}

fn ensure_topic_belongs_to(
    connection: &Connection,
    topic_id: &str,
    meeting_id: &str,
) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM meeting_topics WHERE id = ?1 AND meeting_id = ?2
         )",
        params![topic_id, meeting_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(not_found("topic in meeting", topic_id))
    }
}

fn touch_meeting(transaction: &Transaction<'_>, meeting_id: &str, now: i64) -> Result<()> {
    transaction.execute(
        "UPDATE meetings SET updated_at_ms = ?1, last_activity_at_ms = ?1 WHERE id = ?2",
        params![now, meeting_id],
    )?;
    Ok(())
}

fn require_changed(changed: usize, entity: &'static str, id: &str) -> Result<()> {
    if changed == 0 {
        Err(not_found(entity, id))
    } else {
        Ok(())
    }
}

fn require_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(MeetingStoreError::InvalidData(format!(
            "{field} cannot be empty"
        )))
    } else {
        Ok(())
    }
}

fn normalize_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn not_found(entity: &'static str, id: &str) -> MeetingStoreError {
    MeetingStoreError::NotFound {
        entity,
        id: id.to_owned(),
    }
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_meeting() -> NewMeeting {
        NewMeeting::live("Weekly sync", Some("default".to_owned()), "en", "zh-CN")
    }

    fn append(store: &MeetingStore, meeting_id: &str, token: Option<&str>) -> Segment {
        let topic_id = store
            .list_topics(meeting_id)
            .unwrap()
            .last()
            .unwrap()
            .id
            .clone();
        store
            .upsert_segment(NewSegment {
                meeting_id: meeting_id.to_owned(),
                external_key: new_id(),
                topic_id,
                original_text: "Ship it".to_owned(),
                translated_text: Some("发布吧".to_owned()),
                start_ms: 100,
                end_ms: 900,
                source: SegmentSource::Microphone,
                recognition_run_id: "capture-run-1".to_owned(),
                speaker_token: token.map(ToOwned::to_owned),
                is_final: true,
            })
            .unwrap()
    }

    #[test]
    fn creates_default_topic_and_supports_lifecycle() {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store.create_meeting(live_meeting()).unwrap();
        assert_eq!(bundle.meeting.status, MeetingStatus::Draft);
        assert_eq!(bundle.topics.len(), 1);
        assert_eq!(bundle.topics[0].sequence, 0);

        assert_eq!(
            store.start_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Live
        );
        assert_eq!(
            store.pause_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Paused
        );
        assert_eq!(
            store.resume_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Live
        );
        assert_eq!(
            store.end_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Ended
        );
        // Explicitly reopening an ended card can continue capturing.
        assert_eq!(
            store.resume_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Live
        );
    }

    #[test]
    fn markdown_export_does_not_disclose_absolute_local_paths() {
        let store = MeetingStore::open_in_memory().unwrap();
        let mut meeting = NewMeeting::imported_audio(
            "Private path",
            r"C:\Users\alice\Recordings\planning.wav",
            "en",
            "zh",
        );
        meeting.recording_path = Some(r"C:\Users\alice\AppData\meeting-recordings".into());
        meeting.can_reprocess = true;
        let bundle = store.create_meeting(meeting).unwrap();

        let markdown = render_markdown(&bundle);
        assert!(markdown.contains("planning.wav"));
        assert!(!markdown.contains(r"C:\Users\alice"));
        assert!(markdown.contains("retained in XRTranslate local storage"));
    }

    #[test]
    fn topics_segments_markers_and_minutes_round_trip() {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store.create_meeting(live_meeting()).unwrap();
        let meeting_id = &bundle.meeting.id;
        let first = append(&store, meeting_id, None);
        let topic = store
            .create_topic(meeting_id, Some("Launch risks"))
            .unwrap();
        let second = append(&store, meeting_id, None);
        assert_eq!(second.topic_id, topic.id);

        let marker = store
            .add_marker(&second.id, MarkerKind::ActionItem, "Alice owns rollout")
            .unwrap();
        store
            .update_marker(&marker.id, MarkerKind::KeyDecision, "Roll out Friday")
            .unwrap();
        store
            .save_minutes(meeting_id, "# Edited meeting notes")
            .unwrap();

        let reopened = store.open_meeting(meeting_id).unwrap();
        assert_eq!(reopened.topics.len(), 2);
        assert_eq!(reopened.segments.len(), 2);
        assert_eq!(reopened.segments[0].id, first.id);
        assert_eq!(reopened.markers[0].kind, MarkerKind::KeyDecision);
        assert_eq!(reopened.markers[0].text, "Roll out Friday");
        assert_eq!(reopened.minutes.unwrap().markdown, "# Edited meeting notes");
    }

    #[test]
    fn streaming_upsert_preserves_segment_identity_and_order() {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store.create_meeting(live_meeting()).unwrap();
        let topic_id = bundle.topics[0].id.clone();
        let mut partial = NewSegment {
            meeting_id: bundle.meeting.id.clone(),
            external_key: "asr-generation-7:utterance-2".to_owned(),
            topic_id,
            original_text: "Ship".to_owned(),
            translated_text: None,
            start_ms: 100,
            end_ms: 500,
            source: SegmentSource::SystemAudio,
            recognition_run_id: "capture-run-7".to_owned(),
            speaker_token: Some("cluster-a".to_owned()),
            is_final: false,
        };
        let first = store.upsert_segment(partial.clone()).unwrap();
        partial.original_text = "Ship it".to_owned();
        partial.translated_text = Some("发布吧".to_owned());
        partial.end_ms = 800;
        partial.is_final = true;
        let revised = store.upsert_segment(partial).unwrap();
        assert_eq!(revised.id, first.id);
        assert_eq!(revised.sequence, first.sequence);
        assert_eq!(revised.created_at_ms, first.created_at_ms);
        assert_eq!(store.list_segments(&bundle.meeting.id).unwrap().len(), 1);
        assert_eq!(revised.original_text, "Ship it");
        assert!(revised.is_final);
    }

    #[test]
    fn speaker_assignment_rename_and_merge_preserve_raw_tokens() {
        let store = MeetingStore::open_in_memory().unwrap();
        let meeting = store.create_meeting(live_meeting()).unwrap().meeting;
        let first = append(&store, &meeting.id, Some("voice-cluster-1"));
        assert!(first.canonical_speaker_id.is_none());

        let one = store
            .assign_speaker_token(&meeting.id, "capture-run-1", "voice-cluster-1", "Speaker 1")
            .unwrap();
        let one = store.rename_speaker(&one.id, "Alice").unwrap();
        assert!(!one.is_provisional);
        let second_segment = append(&store, &meeting.id, Some("voice-cluster-2"));
        let two = store
            .assign_speaker_token(&meeting.id, "capture-run-1", "voice-cluster-2", "Speaker 2")
            .unwrap();
        store.merge_speakers(&two.id, &one.id).unwrap();

        let reopened = store.open_meeting(&meeting.id).unwrap();
        assert_eq!(reopened.speakers.len(), 1);
        assert_eq!(reopened.speakers[0].name, "Alice");
        let first = reopened
            .segments
            .iter()
            .find(|item| item.id == first.id)
            .unwrap();
        let second = reopened
            .segments
            .iter()
            .find(|item| item.id == second_segment.id)
            .unwrap();
        assert_eq!(first.speaker_token.as_deref(), Some("voice-cluster-1"));
        assert_eq!(second.speaker_token.as_deref(), Some("voice-cluster-2"));
        assert_eq!(first.canonical_speaker_id, second.canonical_speaker_id);
    }

    #[test]
    fn crash_recovery_is_durable_and_cascade_delete_is_complete() {
        let path = std::env::temp_dir().join(format!("meeting-store-{}.sqlite3", new_id()));
        let meeting_id = {
            let store = MeetingStore::open(&path).unwrap();
            let meeting = store.create_meeting(live_meeting()).unwrap().meeting;
            store.start_meeting(&meeting.id).unwrap();
            let segment = append(&store, &meeting.id, Some("raw-token"));
            store
                .add_marker(&segment.id, MarkerKind::Note, "Remember")
                .unwrap();
            meeting.id
        };
        {
            let store = MeetingStore::open(&path).unwrap();
            assert_eq!(
                store.recover_interrupted_meetings().unwrap(),
                vec![meeting_id.clone()]
            );
            let reopened = store.open_meeting(&meeting_id).unwrap();
            assert_eq!(reopened.meeting.status, MeetingStatus::Interrupted);
            assert_eq!(reopened.segments.len(), 1);
            assert_eq!(
                store.resume_meeting(&meeting_id).unwrap().status,
                MeetingStatus::Live
            );
            store.delete_meeting(&meeting_id).unwrap();
            assert!(matches!(
                store.open_meeting(&meeting_id),
                Err(MeetingStoreError::NotFound { .. })
            ));
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn imported_audio_is_reprocessable_and_uses_processing_state() {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store
            .create_meeting(NewMeeting::imported_audio(
                "Interview",
                "interview.wav",
                "ja",
                "zh-CN",
            ))
            .unwrap();
        assert_eq!(bundle.meeting.status, MeetingStatus::Imported);
        assert!(bundle.meeting.can_reprocess);
        assert_eq!(
            store.start_meeting(&bundle.meeting.id).unwrap().status,
            MeetingStatus::Processing
        );
    }

    #[test]
    fn markdown_export_contains_only_stored_content_and_evidence_links() {
        let store = MeetingStore::open_in_memory().unwrap();
        let bundle = store.create_meeting(live_meeting()).unwrap();
        let segment = append(&store, &bundle.meeting.id, Some("cluster-7"));
        let speaker = store
            .assign_speaker_token(
                &bundle.meeting.id,
                "capture-run-1",
                "cluster-7",
                "Speaker 7",
            )
            .unwrap();
        store.rename_speaker(&speaker.id, "Mina").unwrap();
        store
            .add_marker(
                &segment.id,
                MarkerKind::ActionItem,
                "Mina will publish the build",
            )
            .unwrap();
        store
            .save_minutes(&bundle.meeting.id, "Edited by the user.")
            .unwrap();

        let markdown = render_markdown(&store.open_meeting(&bundle.meeting.id).unwrap());
        assert!(markdown.contains("# Weekly sync"));
        assert!(markdown.contains("Languages: `en` → `zh-CN`"));
        assert!(markdown.contains("## Meeting notes\n\nEdited by the user."));
        assert!(markdown.contains("### Topic 1"));
        assert!(markdown.contains("· Mina"));
        assert!(markdown.contains("> Ship it"));
        assert!(markdown.contains("> 发布吧"));
        assert!(markdown.contains("**Action item**"));
        assert!(markdown.contains("Mina will publish the build"));
        assert!(markdown.contains(&format!("[00:00](#segment-{})", segment.id)));
        assert!(markdown.contains(&format!("<a id=\"segment-{}\"></a>", segment.id)));
    }
}
