//! Bounded LRC files with a small, persistent least-recently-used index.
//! Numeric file IDs keep song metadata out of paths without introducing hashes.
use super::{player::Track, timeline};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const CAPACITY: u64 = 50_000_000;
const MAX_LYRIC_BYTES: u64 = 256 * 1024;

pub(super) struct Cache {
    directory: PathBuf,
    index: Mutex<Option<Index>>,
    generation: AtomicU64,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    // Oldest first. A hit moves its entry to the end, including across restarts.
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    id: u64,
    track: Track,
    duration_ms: u64,
    bytes: u64,
}

impl Entry {
    fn matches(&self, track: &Track, duration_ms: u64) -> bool {
        self.track == *track
            && (duration_ms == 0
                || self.duration_ms == 0
                || self.duration_ms.abs_diff(duration_ms) <= 2000)
    }
}

impl Cache {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            index: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Called before dispatching cleanup so an older download cannot refill it.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn load(&self, track: &Track, duration_ms: u64) -> io::Result<Option<Vec<timeline::Cue>>> {
        let mut state = self.index.lock();
        let index = self.open(&mut state)?;
        let key = normalized(track);
        let Some(position) = index
            .entries
            .iter()
            .position(|entry| entry.matches(&key, duration_ms))
        else {
            return Ok(None);
        };
        let entry = index.entries.remove(position);
        let path = self.lyric_path(entry.id);
        let cues = fs::metadata(&path)
            .ok()
            .filter(|info| info.len() <= MAX_LYRIC_BYTES)
            .and_then(|_| fs::read_to_string(&path).ok())
            .map(|lrc| timeline::parse(&lrc));
        let cues = cues.filter(|cues| cues.iter().any(|cue| !cue.text.is_empty()));
        if cues.is_some() {
            index.entries.push(entry);
        } else {
            remove_file(&path)?;
        }
        // A read remains useful even if the recency update cannot be written.
        if let Err(error) = self.save_index(index) {
            log::warn!("Lyrics cache: {error}");
        }
        Ok(cues)
    }

    pub fn store(
        &self,
        track: &Track,
        duration_ms: u64,
        lrc: &str,
        generation: u64,
    ) -> io::Result<()> {
        if lrc.len() as u64 > MAX_LYRIC_BYTES {
            return Ok(());
        }
        let mut state = self.index.lock();
        if generation != self.generation() {
            return Ok(());
        }
        let index = self.open(&mut state)?;
        let track = normalized(track);
        let id = index
            .entries
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        if let Some(position) = index
            .entries
            .iter()
            .position(|entry| entry.matches(&track, duration_ms))
        {
            remove_file(&self.lyric_path(index.entries[position].id))?;
            index.entries.remove(position);
        }
        index.entries.push(Entry {
            id,
            track,
            duration_ms,
            bytes: lrc.len() as u64,
        });
        let result = (|| {
            // Reserve space for the published index and its atomic replacement.
            self.trim(index)?;
            if index.entries.last().is_none_or(|entry| entry.id != id) {
                return self.save_index(index);
            }
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            file.write_all(lrc.as_bytes())?;
            file.persist(self.lyric_path(id))
                .map_err(|error| error.error)?;
            self.save_index(index)
        })();
        if result.is_err() {
            // The next access reconciles any incomplete write with the index.
            *state = None;
        }
        result
    }

    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.index.lock();
        *state = None;
        match fs::remove_dir_all(&self.directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn lyric_path(&self, id: u64) -> PathBuf {
        self.directory.join(format!("{id}.lrc"))
    }

    fn open<'a>(&self, state: &'a mut Option<Index>) -> io::Result<&'a mut Index> {
        if state.is_none() {
            fs::create_dir_all(&self.directory)?;
            let path = self.directory.join("index.json");
            let mut index: Index = fs::metadata(&path)
                .ok()
                .filter(|info| info.len() <= CAPACITY)
                .and_then(|_| fs::read(&path).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            index.entries.retain_mut(|entry| {
                let Ok(metadata) = fs::metadata(self.lyric_path(entry.id)) else {
                    return false;
                };
                entry.bytes = metadata.len();
                metadata.is_file() && entry.bytes <= MAX_LYRIC_BYTES
            });
            // Recover from interrupted writes; unindexed lyric and staging files
            // belong to this cache and must not escape its budget.
            let ids = index
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<std::collections::BTreeSet<_>>();
            for file in fs::read_dir(&self.directory)? {
                let file = file?;
                if file.file_name() == "index.json" {
                    continue;
                }
                let path = file.path();
                let id = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u64>().ok());
                if path.extension().is_none_or(|ext| ext != "lrc")
                    || id.is_none_or(|id| !ids.contains(&id))
                {
                    if file.file_type()?.is_file() {
                        remove_file(&path)?;
                    }
                }
            }
            self.trim(&mut index)?;
            self.save_index(&index)?;
            *state = Some(index);
        }
        Ok(state.as_mut().unwrap())
    }

    fn trim(&self, index: &mut Index) -> io::Result<()> {
        let mut bytes = index.entries.iter().map(|entry| entry.bytes).sum::<u64>();
        let index_bytes = serde_json::to_vec(index)?.len() as u64;
        // Each removed entry also shrinks the index; the initial estimate keeps
        // eviction linear and leaves a little headroom for its next update.
        let mut removed = 0;
        for entry in &index.entries {
            if bytes + 2 * index_bytes <= CAPACITY {
                break;
            }
            if let Err(error) = remove_file(&self.lyric_path(entry.id)) {
                index.entries.drain(..removed);
                return Err(error);
            }
            bytes = bytes.saturating_sub(entry.bytes);
            removed += 1;
        }
        index.entries.drain(..removed);
        Ok(())
    }

    fn save_index(&self, index: &Index) -> io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut file, index)?;
        file.persist(self.directory.join("index.json"))
            .map_err(|error| error.error)?;
        Ok(())
    }
}

fn normalized(track: &Track) -> Track {
    let clean = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let mut artists = track
        .artists
        .iter()
        .map(|artist| clean(artist))
        .collect::<Vec<_>>();
    artists.sort();
    artists.dedup();
    Track {
        title: clean(&track.title),
        artists,
        album: clean(&track.album),
    }
}

fn remove_file(path: &std::path::Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
