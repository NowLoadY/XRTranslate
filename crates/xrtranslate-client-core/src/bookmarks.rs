//! Shared bookmark records and operations. Storage belongs to the host archive.
use serde::{Deserialize, Serialize};
pub const PAGE_SIZE: usize = 100;
pub const DEFAULT_FOLDER: i64 = 1;
pub type Reply = crossbeam_channel::Receiver<Result<Output, String>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub source: String,
    pub translated: String,
    pub source_lang: String,
    pub target_lang: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: i64,
    pub entry: Entry,
    pub languages: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Example {
    pub sentence: String,
    pub back_translation: String,
    /// Empty in older records, whose examples used the entry's target language.
    pub sentence_lang: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Translation {
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

#[derive(Serialize, Deserialize)]
pub enum Action {
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

#[derive(Serialize, Deserialize)]
pub enum Output {
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

pub use crate::can_lookup as can_save;
