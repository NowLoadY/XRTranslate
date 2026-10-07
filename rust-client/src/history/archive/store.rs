//! SQLite operations, stable record identities and local-calendar deletion ranges.
use super::PAGE_SIZE;
use crate::session_coordinator::TranslationSegment;
use rusqlite::{Connection, params};
use std::{collections::HashSet, path::Path, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RecordId {
    session: String,
    stream: u64,
    turn: String,
    segment: u32,
}
impl RecordId {
    pub(super) fn belongs_to(&self, session: &str) -> bool {
        self.session == session
    }
    pub(super) fn for_segment(session: &str, segment: &TranslationSegment) -> Self {
        Self {
            session: session.into(),
            stream: segment.stream_id,
            turn: segment.turn_id.clone(),
            segment: segment.segment_index,
        }
    }
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            session: row.get(0)?,
            stream: row.get(1)?,
            turn: row.get(2)?,
            segment: row.get(3)?,
        })
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub id: RecordId,
    pub source: String,
    pub translated: String,
    pub timestamp: String,
}
pub(crate) enum DeleteSelection {
    Record(RecordId),
    Dates { from: String, through: String },
}

pub(super) fn open_database(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(500))?;
    connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS translations (
        session TEXT NOT NULL, stream INTEGER NOT NULL, turn TEXT NOT NULL, segment INTEGER NOT NULL,
        source TEXT NOT NULL, translated TEXT NOT NULL, search TEXT NOT NULL, saved_at INTEGER NOT NULL,
        PRIMARY KEY(session, stream, turn, segment));
        CREATE INDEX IF NOT EXISTS translations_recent ON translations(saved_at DESC);")?;
    Ok(connection)
}
pub(super) fn save(
    database: &mut Connection,
    session: &str,
    segments: &[TranslationSegment],
    replace: bool,
) -> rusqlite::Result<()> {
    let transaction = database.transaction()?;
    if replace && let Some(first) = segments.first() {
        transaction.execute(
            "DELETE FROM translations WHERE session=?1 AND stream=?2 AND turn=?3",
            params![session, first.stream_id, first.turn_id],
        )?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    for segment in segments {
        let mut translated = segment.translated.clone().unwrap_or_default();
        for extra in &segment.additional_translations {
            translated.push('\n');
            translated.push_str(&extra.translated_text);
        }
        let searchable = format!("{}\n{}", segment.source, translated).to_lowercase();
        transaction.execute("INSERT INTO translations VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
            ON CONFLICT(session,stream,turn,segment) DO UPDATE SET source=excluded.source,translated=excluded.translated,search=excluded.search",
            params![session, segment.stream_id, segment.turn_id, segment.segment_index, segment.source, translated, searchable, now])?;
    }
    transaction.commit()
}
pub(super) fn search(
    database: &Connection,
    query: &str,
    offset: usize,
) -> rusqlite::Result<Vec<Record>> {
    let mut statement = database.prepare_cached("SELECT session,stream,turn,segment,source,translated,strftime('%Y-%m-%d %H:%M',saved_at/1000,'unixepoch','localtime') FROM translations WHERE instr(search,?1)>0 ORDER BY saved_at DESC,rowid DESC LIMIT ?2 OFFSET ?3")?;
    statement
        .query_map(
            params![query.trim().to_lowercase(), PAGE_SIZE + 1, offset],
            |row| {
                Ok(Record {
                    id: RecordId::read(row)?,
                    source: row.get(4)?,
                    translated: row.get(5)?,
                    timestamp: row.get(6)?,
                })
            },
        )?
        .collect()
}

/// Snapshot identities before confirmation so later arrivals are never swept in.
pub(super) fn prepare_deletion(
    recent: &Connection,
    saved: Option<&Connection>,
    selection: &DeleteSelection,
) -> Result<Vec<RecordId>, String> {
    let mut ids = HashSet::new();
    let bounds = match selection {
        DeleteSelection::Dates { from, through } => Some(date_bounds(recent, from, through)?),
        DeleteSelection::Record(_) => None,
    };
    for db in std::iter::once(recent).chain(saved) {
        match selection {
            DeleteSelection::Record(id) => {
                let found: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM translations WHERE session=?1 AND stream=?2 AND turn=?3 AND segment=?4)",
                    params![id.session, id.stream, id.turn, id.segment], |row| row.get(0),
                ).map_err(|e| e.to_string())?;
                if found {
                    ids.insert(id.clone());
                }
            }
            DeleteSelection::Dates { .. } => {
                let (start, end) = bounds.unwrap();
                let mut statement = db.prepare_cached(
                    "SELECT session,stream,turn,segment FROM translations WHERE saved_at>=?1 AND saved_at<?2",
                ).map_err(|e| e.to_string())?;
                let rows = statement
                    .query_map(params![start, end], RecordId::read)
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    ids.insert(row.map_err(|e| e.to_string())?);
                }
            }
        }
    }
    Ok(ids.into_iter().collect())
}

fn date_bounds(db: &Connection, from: &str, through: &str) -> Result<(i64, i64), String> {
    let date_shape = |s: &str| {
        s.len() == 10
            && s.bytes().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            })
    };
    let invalid =
        || "Enter valid dates as YYYY-MM-DD, with the start no later than the end.".to_owned();
    if !date_shape(from) || !date_shape(through) || from > through {
        return Err(invalid());
    }
    let (valid, start, end): (bool, Option<i64>, Option<i64>) = db
        .query_row(
            "SELECT coalesce(date(?1,'+0 days')=?1 AND date(?2,'+0 days')=?2,0),
            unixepoch(?1,'utc')*1000, unixepoch(?2,'+1 day','utc')*1000",
            params![from, through],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    match (valid, start, end) {
        (true, Some(start), Some(end)) if start < end => Ok((start, end)),
        _ => Err(invalid()),
    }
}

pub(super) fn remove(database: &mut Connection, ids: &[RecordId]) -> rusqlite::Result<()> {
    let transaction = database.transaction()?;
    {
        let mut statement = transaction.prepare_cached(
            "DELETE FROM translations WHERE session=?1 AND stream=?2 AND turn=?3 AND segment=?4",
        )?;
        for id in ids {
            statement.execute(params![id.session, id.stream, id.turn, id.segment])?;
        }
    }
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_deletion_uses_whole_local_days_and_only_the_previewed_records() {
        let mut recent = open_database(Path::new(":memory:")).unwrap();
        let mut saved = open_database(Path::new(":memory:")).unwrap();
        let (start, end) = date_bounds(&recent, "2026-10-07", "2026-10-07").unwrap();
        for date in [
            "2026-02-29",
            "2026-10-32",
            "2026-1-07",
            "2026-10-07' OR 1=1",
        ] {
            assert!(date_bounds(&recent, date, date).is_err());
        }
        assert!(date_bounds(&recent, "2026-10-08", "2026-10-07").is_err());
        assert!(date_bounds(&recent, "2024-02-29", "2024-02-29").is_ok());
        let add = |db: &mut Connection, name: &str, timestamp| {
            let mut segment = crate::session_coordinator::test_segment(name, "译文");
            segment.turn_id = name.into();
            save(db, "session", &[segment], false).unwrap();
            db.execute(
                "UPDATE translations SET saved_at=?1 WHERE turn=?2",
                params![timestamp, name],
            )
            .unwrap();
        };
        add(&mut saved, "before", start - 1);
        add(&mut saved, "start", start);
        add(&mut recent, "start", start);
        add(&mut recent, "last millisecond", end - 1);
        add(&mut saved, "after", end);
        let selection = DeleteSelection::Dates {
            from: "2026-10-07".into(),
            through: "2026-10-07".into(),
        };
        let ids = prepare_deletion(&recent, Some(&saved), &selection).unwrap();
        assert_eq!(ids.len(), 2, "the same record in both lists counts once");
        assert!(
            search(&recent, "last millisecond", 0).unwrap()[0]
                .timestamp
                .starts_with("2026-10-07")
        );
        assert_eq!(
            search(&saved, "", 0).unwrap().len(),
            3,
            "preview does not delete"
        );
        add(&mut saved, "arrived after preview", start + 100);
        remove(&mut saved, &ids).unwrap();
        remove(&mut recent, &ids).unwrap();
        assert!(search(&recent, "", 0).unwrap().is_empty());
        let remaining: HashSet<_> = search(&saved, "", 0)
            .unwrap()
            .into_iter()
            .map(|row| row.source)
            .collect();
        assert_eq!(
            remaining,
            HashSet::from([
                "before".into(),
                "after".into(),
                "arrived after preview".into()
            ])
        );
    }
}
