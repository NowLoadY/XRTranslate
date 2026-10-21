//! Bounded views over the durable transcript; no second in-memory task backlog.
use super::{MeetingStore, Result, Segment, repository::map_segment};
use rusqlite::{OptionalExtension, params};

const COLUMNS: &str = "id, external_key, meeting_id, topic_id, sequence, original_text,
    translated_text, start_ms, end_ms, source, recognition_run_id, speaker_token,
    canonical_speaker_id, is_final, created_at_ms, updated_at_ms, source_language, target_language";

pub(in crate::plugins::meeting) struct TranslationWindow {
    pub current: Segment,
    pub before: Vec<Segment>,
    pub after: Vec<Segment>,
}

impl MeetingStore {
    pub(in crate::plugins::meeting) fn translation_window(
        &self,
        meeting: &str,
        run: &str,
        imported: bool,
    ) -> Result<Option<TranslationWindow>> {
        let connection = self.connection()?;
        let current = connection
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM meeting_segments
                WHERE meeting_id = ?1 AND recognition_run_id = ?2 AND is_final = 1
                  AND length(trim(original_text)) > 0
                  AND length(trim(COALESCE(translated_text, ''))) = 0
                ORDER BY start_ms, end_ms, id LIMIT 1"
                ),
                params![meeting, run],
                map_segment,
            )
            .optional()?;
        let Some(current) = current else {
            return Ok(None);
        };
        let mut before = connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM meeting_segments
             WHERE meeting_id = ?1 AND (?2 = 0 OR recognition_run_id = ?3)
               AND is_final = 1 AND length(trim(original_text)) > 0
               AND (start_ms, end_ms, id) < (?4, ?5, ?6)
             ORDER BY start_ms DESC, end_ms DESC, id DESC LIMIT 8"
            ))?
            .query_map(
                params![
                    meeting,
                    imported,
                    run,
                    current.start_ms,
                    current.end_ms,
                    current.id
                ],
                map_segment,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        before.reverse();
        let after = connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM meeting_segments
             WHERE meeting_id = ?1 AND recognition_run_id = ?2
               AND is_final = 1 AND length(trim(original_text)) > 0
               AND (start_ms, end_ms, id) > (?3, ?4, ?5)
             ORDER BY start_ms, end_ms, id LIMIT 2"
            ))?
            .query_map(
                params![meeting, run, current.start_ms, current.end_ms, current.id],
                map_segment,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(TranslationWindow {
            current,
            before,
            after,
        }))
    }

    /// Only fill the same finalized source revision; identities and annotations stay intact.
    pub(in crate::plugins::meeting) fn complete_translation(
        &self,
        source: &Segment,
        translated: &str,
        source_language: &str,
        target_language: &str,
    ) -> Result<bool> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE meeting_segments SET translated_text = ?1, source_language = ?2,
                 target_language = ?3, updated_at_ms = ?4
             WHERE id = ?5 AND recognition_run_id = ?6 AND original_text = ?7
               AND source_language = ?8 AND is_final = 1
               AND length(trim(COALESCE(translated_text, ''))) = 0",
            params![
                translated,
                source_language,
                target_language,
                now,
                source.id,
                source.recognition_run_id,
                source.original_text,
                source.source_language
            ],
        )?;
        if changed > 0 {
            transaction.execute(
                "UPDATE meetings SET updated_at_ms = ?1, last_activity_at_ms = ?1 WHERE id = ?2",
                params![now, source.meeting_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed > 0)
    }
}
