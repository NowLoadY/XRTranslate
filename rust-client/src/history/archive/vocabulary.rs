//! Explicit bookmarks share the history database and its single worker.
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) const DEFAULT_FOLDER: i64 = 1;
pub(crate) type Reply = crossbeam_channel::Receiver<Result<Output, String>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub source: String,
    pub translated: String,
    pub source_lang: String,
    pub target_lang: String,
}

#[derive(Clone)]
pub(crate) struct Record {
    pub id: i64,
    pub entry: Entry,
    pub languages: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Example {
    pub sentence: String,
    pub back_translation: String,
}

#[derive(Clone)]
pub(crate) struct Translation {
    pub id: i64,
    pub entry: Entry,
    pub example: Example,
}

impl Entry {
    pub fn canonical(mut self) -> Result<Self, String> {
        for tag in [&mut self.source_lang, &mut self.target_lang] {
            *tag = xrtranslate_engine::language::SupportedLanguage::parse(tag)
                .ok_or_else(|| format!("Unknown vocabulary language: {tag}"))?
                .code()
                .to_owned();
        }
        self.source = self.source.trim().to_owned();
        self.translated = self.translated.trim().to_owned();
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub count: usize,
    pub saved: bool,
}
impl Folder {
    pub fn label(&self, language: crate::i18n::UiLanguage) -> &str {
        if self.name.is_empty() {
            crate::i18n::tr(language, "Default wordbook")
        } else {
            &self.name
        }
    }
}

pub(crate) enum Action {
    Folders(Entry),
    Page {
        folder: i64,
        query: String,
        offset: usize,
    },
    Add {
        folder: i64,
        entry: Entry,
    },
    Detail {
        folder: i64,
        source: String,
        source_lang: String,
    },
    SaveExample {
        entry: Entry,
        example: Example,
    },
    RemoveWord {
        folder: i64,
        source: String,
        source_lang: String,
    },
    Update {
        id: i64,
        folder: i64,
        entry: Entry,
    },
    Remove(i64),
    CreateFolder(String),
    RenameFolder {
        id: i64,
        name: String,
    },
    DeleteFolder(i64),
}
impl Action {
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            Self::Folders(_) | Self::Page { .. } | Self::Detail { .. }
        )
    }
}

pub(crate) enum Output {
    Folders(Vec<Folder>),
    Page {
        folders: Vec<Folder>,
        records: Vec<Record>,
    },
    CreatedFolder(i64),
    Updated {
        folder: i64,
        entry: Entry,
    },
    Detail(Vec<Translation>),
    Changed,
}

pub(crate) fn can_save(source: &str) -> bool {
    let source = source.trim();
    !source.is_empty()
        && !source.contains(['\n', '\r'])
        && source.chars().count() <= 80
        && source.unicode_words().count() <= 12
        && xrtranslate_engine::sentence_end_offsets(source).is_empty()
}

fn folders(db: &Connection) -> rusqlite::Result<Vec<Folder>> {
    db.prepare_cached("SELECT f.id,f.name,COUNT(v.id) FROM vocabulary_folders f
        LEFT JOIN vocabulary v ON f.id=v.folder_id GROUP BY f.id ORDER BY f.id=1 DESC,f.name COLLATE NOCASE")?
        .query_map([], |row| Ok(Folder { id: row.get(0)?, name: row.get(1)?, count: row.get(2)?, saved: false }))?.collect()
}

pub(super) fn execute(db: &mut Connection, action: &Action) -> rusqlite::Result<Output> {
    match action {
        Action::Folders(entry) => {
            let mut rows = folders(db)?;
            for folder in &mut rows {
                folder.saved = db.query_row("SELECT EXISTS(SELECT 1 FROM vocabulary
                    WHERE folder_id=?1 AND source=?2 AND translated=?3 AND source_lang=?4 AND target_lang=?5)",
                    params![folder.id, entry.source, entry.translated, entry.source_lang, entry.target_lang], |row| row.get(0))?;
            }
            return Ok(Output::Folders(rows));
        }
        Action::Page {
            folder,
            query,
            offset,
        } => {
            let records = db.prepare_cached("SELECT v.id,v.source,v.translated,v.source_lang,v.target_lang,g.languages FROM vocabulary v JOIN
                (SELECT MAX(id) AS id,MAX(saved_at) AS saved_at,COUNT(DISTINCT target_lang) AS languages FROM vocabulary
                WHERE folder_id=?1 AND instr(search,?2)>0 GROUP BY source,source_lang) g ON v.id=g.id
                ORDER BY g.saved_at DESC,v.id DESC LIMIT ?3 OFFSET ?4")?
                .query_map(params![folder, query.trim().to_lowercase(), super::PAGE_SIZE + 1, offset], |row| Ok(Record {
                    id: row.get(0)?, entry: Entry { source: row.get(1)?, translated: row.get(2)?,
                        source_lang: row.get(3)?, target_lang: row.get(4)? },
                    languages: row.get(5)?,
                }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(Output::Page {
                folders: folders(db)?,
                records,
            });
        }
        Action::Detail {
            folder,
            source,
            source_lang,
        } => {
            let rows = db.prepare_cached("SELECT v.id,v.source,v.translated,v.source_lang,v.target_lang,
                COALESCE(e.sentence,''),COALESCE(e.back_translation,'') FROM vocabulary v
                LEFT JOIN vocabulary_examples e USING(source,source_lang,target_lang,translated)
                WHERE folder_id=?1 AND v.source=?2 AND v.source_lang=?3 ORDER BY v.target_lang,v.id")?
                .query_map(params![folder, source, source_lang], |row| Ok(Translation {
                    id: row.get(0)?, entry: Entry { source: row.get(1)?, translated: row.get(2)?, source_lang: row.get(3)?, target_lang: row.get(4)? },
                    example: Example { sentence: row.get(5)?, back_translation: row.get(6)? },
                }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(Output::Detail(rows));
        }
        Action::SaveExample { entry, example } => {
            // A late result cannot reattach itself after its word or translation was edited/deleted.
            db.execute("INSERT INTO vocabulary_examples(source,source_lang,target_lang,translated,sentence,back_translation)
                SELECT ?1,?2,?3,?4,?5,?6 WHERE EXISTS(SELECT 1 FROM vocabulary
                    WHERE source=?1 AND source_lang=?2 AND target_lang=?3 AND translated=?4)
                ON CONFLICT(source,source_lang,target_lang,translated) DO UPDATE SET
                    sentence=excluded.sentence,
                    back_translation=excluded.back_translation",
                params![entry.source, entry.source_lang, entry.target_lang, entry.translated, example.sentence, example.back_translation])?;
        }
        Action::RemoveWord {
            folder,
            source,
            source_lang,
        } => {
            db.execute(
                "DELETE FROM vocabulary WHERE folder_id=?1 AND source=?2 AND source_lang=?3",
                params![folder, source, source_lang],
            )?;
        }
        Action::Add { folder, entry } => {
            db.execute("INSERT INTO vocabulary(folder_id,source,translated,source_lang,target_lang,search,saved_at)
                VALUES (?1,?2,?3,?4,?5,?6,unixepoch()) ON CONFLICT(folder_id,source,translated,source_lang,target_lang) DO NOTHING",
                params![folder, entry.source, entry.translated, entry.source_lang, entry.target_lang,
                    format!("{}\n{}", entry.source, entry.translated).to_lowercase()])?;
            return execute(db, &Action::Folders(entry.clone()));
        }
        Action::Update { id, folder, entry } => {
            db.execute(
                "UPDATE vocabulary SET folder_id=?1,source=?2,translated=?3,search=?4,source_lang=?5,target_lang=?6 WHERE id=?7",
                params![
                    folder,
                    entry.source,
                    entry.translated,
                    format!("{}\n{}", entry.source, entry.translated).to_lowercase(),
                    entry.source_lang,
                    entry.target_lang,
                    id
                ],
            )?;
        }
        Action::Remove(id) => {
            db.execute("DELETE FROM vocabulary WHERE id=?1", [id])?;
        }
        Action::CreateFolder(name) => {
            db.execute(
                "INSERT INTO vocabulary_folders(name) VALUES (?1)",
                [name.trim()],
            )?;
            return Ok(Output::CreatedFolder(db.last_insert_rowid()));
        }
        Action::RenameFolder { id, name } => {
            db.execute(
                "UPDATE vocabulary_folders SET name=?1 WHERE id=?2",
                params![name.trim(), id],
            )?;
        }
        Action::DeleteFolder(id) if *id != DEFAULT_FOLDER => {
            // Keep words when deleting a folder; identical snapshots merge into the default folder.
            let tx = db.transaction()?;
            tx.execute("INSERT OR IGNORE INTO vocabulary(folder_id,source,translated,source_lang,target_lang,search,saved_at)
                SELECT 1,source,translated,source_lang,target_lang,search,saved_at FROM vocabulary WHERE folder_id=?1", [id])?;
            tx.execute("DELETE FROM vocabulary WHERE folder_id=?1", [id])?;
            tx.execute("DELETE FROM vocabulary_folders WHERE id=?1", [id])?;
            tx.commit()?;
        }
        Action::DeleteFolder(_) => {}
    }
    if matches!(
        action,
        Action::Update { .. } | Action::Remove(_) | Action::RemoveWord { .. }
    ) {
        db.execute("DELETE FROM vocabulary_examples AS e WHERE NOT EXISTS(SELECT 1 FROM vocabulary v
            WHERE v.source=e.source AND v.source_lang=e.source_lang AND v.target_lang=e.target_lang AND v.translated=e.translated)", [])?;
    }
    Ok(match action {
        Action::Update { folder, entry, .. } => Output::Updated {
            folder: *folder,
            entry: entry.clone(),
        },
        _ => Output::Changed,
    })
}
