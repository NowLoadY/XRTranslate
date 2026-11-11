//! Move writable storage while the application and its services are stopped.
//! Both the desktop UI and installer use this operation. Plugin documents stay
//! opaque: only absolute file references in JSON/SQLite are relocated.
use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::{Connection, functions::FunctionFlags};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const DIRECTORIES: &[&str] = &["runtime", "models", "plugins"];

#[derive(Clone, Debug)]
pub struct DataMove {
    source: PathBuf,
    target: PathBuf,
    location_file: PathBuf,
    retained: Vec<PathBuf>,
}

impl DataMove {
    pub fn new(root: &Path, target: &Path) -> Result<Self> {
        Self::with_paths(
            root,
            &crate::data_root(root),
            target,
            &crate::data_location_file(root),
        )
    }

    /// Explicit paths let Setup preserve an old portable installation's data.
    pub fn with_paths(
        root: &Path,
        source: &Path,
        target: &Path,
        location_file: &Path,
    ) -> Result<Self> {
        let source = absolute(source)?;
        let target = absolute(target)?;
        let root = absolute(root)?;
        let mut retained = Vec::new();
        if source == root {
            // These files belong to the application, not its writable storage.
            retained.extend([
                PathBuf::from("runtime/onnxruntime/cpu"),
                PathBuf::from("models/silero-vad"),
                PathBuf::from("models/3D-Speaker-ERes2NetV2"),
                PathBuf::from("models/gtcrn"),
            ]);
            if let Ok(entries) = fs::read_dir(root.join("plugins")) {
                for entry in entries {
                    let entry = entry?;
                    if fs::read(entry.path().join("plugin.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .is_some_and(|manifest| manifest["bundle_with_host"] == true)
                    {
                        retained.push(Path::new("plugins").join(entry.file_name()));
                    }
                }
            }
        }
        if target == root || target.starts_with(&root) {
            return Err(invalid("Choose a data folder outside the installation folder.").into());
        }
        let plan = Self {
            source,
            target,
            location_file: absolute(location_file)?,
            retained,
        };
        plan.validate()?;
        Ok(plan)
    }

    fn validate(&self) -> Result<()> {
        if self.source.parent().is_none()
            || self.target.parent().is_none()
            || self.source.starts_with(&self.target)
            || self.target.starts_with(&self.source)
        {
            return Err(invalid("Choose a separate, empty data folder.").into());
        }
        if self.target.exists() {
            for entry in fs::read_dir(&self.target)? {
                if entry?.path() != self.location_file {
                    return Err(invalid("Choose an empty data folder.").into());
                }
            }
        }
        Ok(())
    }

    /// Copy and repair references before switching the location preference.
    /// Nothing in the original data is deleted until the new copy is committed.
    pub fn execute(&self, mut progress: impl FnMut(f32)) -> Result<()> {
        use io::{Read, Write};
        self.validate()?;
        progress(0.0);
        let parent = self
            .target
            .parent()
            .ok_or_else(|| invalid("Invalid data folder."))?;
        fs::create_dir_all(parent)?;
        fs::create_dir_all(self.location_file.parent().unwrap())?;
        // Verify that the location can be saved before copying large models.
        drop(tempfile::NamedTempFile::new_in(
            self.location_file.parent().unwrap(),
        )?);
        let staging = tempfile::Builder::new()
            .prefix(".xrtranslate-move-")
            .tempdir_in(parent)?;
        let mut files = Vec::new();
        for name in DIRECTORIES {
            self.collect(Path::new(name), &mut files)?;
        }
        let total: u64 = files.iter().map(|(_, size)| size).sum();
        let mut copied = 0;
        let mut buffer = vec![0; 1024 * 1024];
        let mut last_progress = std::time::Instant::now();
        for (relative, size) in &files {
            let destination = staging.path().join(relative);
            fs::create_dir_all(destination.parent().unwrap())?;
            let source = self.source.join(relative);
            #[cfg(unix)]
            if fs::symlink_metadata(&source)?.file_type().is_symlink() {
                let link = fs::read_link(&source)?;
                let resolved = absolute(&source.parent().unwrap().join(&link))?;
                let link = match self.rebase_path(&resolved.to_string_lossy()) {
                    Some(_) if link.is_relative() => link,
                    Some(moved) => PathBuf::from(moved),
                    None => resolved,
                };
                std::os::unix::fs::symlink(link, destination)?;
                continue;
            }
            let mut input = fs::File::open(&source)?;
            let mut output = fs::File::create(&destination)?;
            let mut bytes = 0;
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
                bytes += count as u64;
                copied += count as u64;
                if last_progress.elapsed().as_millis() >= 100 {
                    progress(0.85 * copied as f32 / total.max(1) as f32);
                    last_progress = std::time::Instant::now();
                }
            }
            fs::set_permissions(&destination, input.metadata()?.permissions())?;
            if bytes != *size {
                return Err(invalid(
                    "User data changed during the move. Exit XRTranslate and try again.",
                )
                .into());
            }
            progress(0.85 * copied as f32 / total.max(1) as f32);
        }
        for (relative, _) in &files {
            let path = staging.path().join(relative);
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                continue;
            }
            match path.extension().and_then(|s| s.to_str()) {
                Some("json") => {
                    let bytes = fs::read(&path)?;
                    if let Ok(mut document) = serde_json::from_slice::<Value>(&bytes)
                        && self.rebase_json(&mut document)
                    {
                        fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
                    }
                }
                Some("sqlite" | "sqlite3" | "db") => self.rebase_database(&path)?,
                _ => {}
            }
        }
        progress(0.95);
        // Recheck before committing, in case another process used the destination.
        self.validate()?;
        fs::create_dir_all(&self.target)?;
        let mut committed = Vec::new();
        let commit = (|| -> Result<()> {
            let mut location =
                tempfile::NamedTempFile::new_in(self.location_file.parent().unwrap())?;
            write!(location, "{}", self.target.display())?;
            location.as_file().sync_all()?;
            for entry in fs::read_dir(staging.path())? {
                let entry = entry?;
                let destination = self.target.join(entry.file_name());
                fs::rename(entry.path(), &destination)?;
                committed.push(destination);
            }
            location.persist(&self.location_file)?;
            Ok(())
        })();
        if let Err(error) = commit {
            // Only the entries just created by this operation can be removed.
            for path in committed {
                let _ = fs::remove_dir_all(path);
            }
            return Err(error);
        }
        for (relative, _) in &files {
            let old_file = self.source.join(relative);
            fs::remove_file(&old_file).map_err(|error| io::Error::new(
                error.kind(), format!("Data moved successfully, but an old file could not be removed: {} ({error})", old_file.display())
            ))?;
            let mut parent = old_file.parent();
            while let Some(directory) = parent {
                if directory == self.source || fs::remove_dir(directory).is_err() {
                    break;
                }
                parent = directory.parent();
            }
        }
        progress(1.0);
        Ok(())
    }

    fn managed(&self, relative: &Path) -> bool {
        DIRECTORIES.iter().any(|name| relative.starts_with(name))
            && !self.retained.iter().any(|path| relative.starts_with(path))
    }

    fn collect(&self, relative: &Path, files: &mut Vec<(PathBuf, u64)>) -> Result<()> {
        if !self.managed(relative) {
            return Ok(());
        }
        let path = self.source.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let linked = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let linked = linked || {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        if linked {
            #[cfg(unix)]
            {
                files.push((relative.to_path_buf(), 0));
                return Ok(());
            }
            #[cfg(not(unix))]
            return Err(invalid(format!(
                "Move linked folders separately: {}",
                path.display()
            ))
            .into());
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                self.collect(&relative.join(entry?.file_name()), files)?;
            }
        } else if metadata.is_file() {
            files.push((relative.to_path_buf(), metadata.len()));
        }
        Ok(())
    }

    fn rebase_path(&self, value: &str) -> Option<String> {
        let source = self.source.to_string_lossy().replace('\\', "/");
        let path = value.replace('\\', "/");
        let prefix = path.get(..source.len())?;
        if !(prefix == source || cfg!(windows) && prefix.eq_ignore_ascii_case(&source)) {
            return None;
        }
        let relative = Path::new(path.get(source.len()..)?.strip_prefix('/')?);
        if relative
            .components()
            .any(|part| part == std::path::Component::ParentDir)
        {
            return None;
        }
        self.managed(relative)
            .then(|| self.target.join(relative).to_string_lossy().into_owned())
    }

    fn rebase_json(&self, value: &mut Value) -> bool {
        match value {
            Value::String(text) => {
                if let Some(path) = self.rebase_path(text) {
                    *text = path;
                    true
                } else {
                    false
                }
            }
            Value::Array(values) => values
                .iter_mut()
                .fold(false, |changed, value| self.rebase_json(value) | changed),
            Value::Object(values) => values
                .values_mut()
                .fold(false, |changed, value| self.rebase_json(value) | changed),
            _ => false,
        }
    }

    fn rebase_text(&self, text: &str) -> String {
        if let Some(path) = self.rebase_path(text) {
            return path;
        }
        if let Ok(mut value) = serde_json::from_str::<Value>(text)
            && self.rebase_json(&mut value)
        {
            return value.to_string();
        }
        text.into()
    }

    fn rebase_database(&self, path: &Path) -> Result<()> {
        use io::Read;
        let mut header = [0; 16];
        if fs::File::open(path)?.read(&mut header)? != 16 || &header != b"SQLite format 3\0" {
            return Ok(());
        }
        let mut connection = Connection::open(path)?;
        let plan = Arc::new(self.clone());
        connection.create_scalar_function(
            "relocate_path",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx| Ok(plan.rebase_text(ctx.get_raw(0).as_str()?)),
        )?;
        let transaction = connection.transaction()?;
        // Ordinary tables only; virtual tables and their indexes manage themselves.
        let tables = transaction.prepare("SELECT name FROM pragma_table_list WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
            .query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        for table in tables {
            let columns = transaction
                .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden = 0")?
                .query_map([&table], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for column in columns {
                let column = quote_identifier(&column);
                transaction.execute(&format!(
                    "UPDATE {} SET {column} = relocate_path({column}) WHERE typeof({column}) = 'text' AND (instr(lower({column}), lower(?1)) > 0 OR instr(lower({column}), lower(?2)) > 0 OR instr(lower({column}), lower(?3)) > 0) AND {column} != relocate_path({column})", quote_identifier(&table)
                ), [self.source.to_string_lossy().as_ref(),
                    &self.source.to_string_lossy().replace('\\', "/"),
                    &self.source.to_string_lossy().replace('\\', "\\\\")])?;
            }
        }
        transaction.commit()?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

// Resolve existing ancestors to catch aliases and nested destinations as well.
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    if path.exists() {
        let path = fs::canonicalize(path)?;
        #[cfg(windows)]
        if let Some(path) = path
            .to_str()
            .and_then(|path| path.strip_prefix(r"\\?\UNC\"))
        {
            return Ok(PathBuf::from(format!(r"\\{path}")));
        }
        #[cfg(windows)]
        if let Some(path) = path.to_str().and_then(|path| path.strip_prefix(r"\\?\")) {
            return Ok(PathBuf::from(path));
        }
        Ok(path)
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Invalid data folder."))?;
        Ok(absolute(parent)?.join(
            path.file_name()
                .ok_or_else(|| invalid("Invalid data folder."))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn move_keeps_subtitles_recordings_prompts_and_independent_plugins() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("portable");
        let target = directory.path().join("用户数据");
        write(&root, "runtime/media-audio/speech.wav", b"speech");
        write(
            &root,
            "runtime/prompt-studio.json",
            br#"{"prompt":"Keep this prompt unchanged."}"#,
        );
        write(&root, "models/custom/model.gguf", b"model");
        write(&root, "models/gtcrn/gtcrn_simple.onnx", b"bundled model");
        write(
            &root,
            "runtime/onnxruntime/cpu/onnxruntime.dll",
            b"bundled library",
        );
        write(
            &root,
            "plugins/bundled/plugin.json",
            br#"{"bundle_with_host":true}"#,
        );
        write(
            &root,
            "plugins/independent/plugin.json",
            br#"{"bundle_with_host":false}"#,
        );
        write(&root, "XRTranslate.exe", b"application");
        let old_audio = root.join("runtime/media-audio/speech.wav");
        let external = directory.path().join("original.mp4");
        let document = serde_json::json!({
            "source": external, "speech": old_audio,
            "cues": [{"start": 1000, "end": 2000, "text": "保留字幕", "speaker": "speaker-1"}],
            "model": root.join("models/gtcrn/gtcrn_simple.onnx")
        });
        write(&root, "runtime/video_tasks.json", document.to_string());
        let db = Connection::open(root.join("runtime/meetings.sqlite3")).unwrap();
        db.execute_batch("CREATE TABLE meetings(id TEXT PRIMARY KEY, recording_path TEXT, note TEXT, audio BLOB) WITHOUT ROWID;").unwrap();
        db.execute(
            "INSERT INTO meetings VALUES ('meeting-1', ?1, 'keep notes', X'0001')",
            [old_audio.to_str().unwrap()],
        )
        .unwrap();
        drop(db);

        DataMove::new(&root, &target)
            .unwrap()
            .execute(|_| {})
            .unwrap();
        assert_eq!(crate::data_root(&root), target);
        assert!(root.join("XRTranslate.exe").is_file());
        assert!(root.join("models/gtcrn/gtcrn_simple.onnx").is_file());
        assert!(
            root.join("runtime/onnxruntime/cpu/onnxruntime.dll")
                .is_file()
        );
        assert!(root.join("plugins/bundled/plugin.json").is_file());
        assert!(target.join("plugins/independent/plugin.json").is_file());
        assert!(!old_audio.exists());
        assert_eq!(
            fs::read(target.join("models/custom/model.gguf")).unwrap(),
            b"model"
        );
        assert_eq!(
            fs::read(target.join("runtime/prompt-studio.json")).unwrap(),
            br#"{"prompt":"Keep this prompt unchanged."}"#
        );
        let moved: Value =
            serde_json::from_slice(&fs::read(target.join("runtime/video_tasks.json")).unwrap())
                .unwrap();
        assert_eq!(moved["cues"], document["cues"]);
        assert_eq!(moved["source"], document["source"]);
        assert_eq!(moved["model"], document["model"]);
        assert_eq!(
            moved["speech"],
            target
                .join("runtime/media-audio/speech.wav")
                .to_str()
                .unwrap()
        );
        let db = Connection::open(target.join("runtime/meetings.sqlite3")).unwrap();
        let recording: String = db
            .query_row("SELECT recording_path FROM meetings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            recording,
            target
                .join("runtime/media-audio/speech.wav")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            db.query_row("SELECT note FROM meetings", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep notes"
        );
    }

    #[test]
    fn failed_preparation_leaves_original_data_and_location_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        let target = directory.path().join("data");
        write(&root, "runtime/user-config.json", b"original settings");
        assert!(DataMove::new(&root, &root.join("nested")).is_err());
        write(&target, "unrelated.txt", b"keep");
        assert!(DataMove::new(&root, &target).is_err());
        assert_eq!(fs::read(target.join("unrelated.txt")).unwrap(), b"keep");
        fs::remove_file(target.join("unrelated.txt")).unwrap();
        let plan = DataMove::new(&root, &target).unwrap();
        // A directory in place of the preference file makes the final switch fail.
        fs::create_dir(root.join("data-directory.txt")).unwrap();
        assert!(plan.execute(|_| {}).is_err());
        assert_eq!(
            fs::read(root.join("runtime/user-config.json")).unwrap(),
            b"original settings"
        );
        assert!(!target.join("runtime").exists());
    }

    #[test]
    fn shared_data_can_move_back_to_its_default_folder() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        let shared = directory.path().join("shared");
        let target = directory.path().join("other-drive");
        let location = shared.join("data-directory.txt");
        write(&shared, "runtime/user-config.json", b"{}");
        DataMove::with_paths(&root, &shared, &target, &location)
            .unwrap()
            .execute(|_| {})
            .unwrap();
        DataMove::with_paths(&root, &target, &shared, &location)
            .unwrap()
            .execute(|_| {})
            .unwrap();
        assert_eq!(
            fs::read_to_string(location).unwrap(),
            shared.to_str().unwrap()
        );
        assert_eq!(
            fs::read(shared.join("runtime/user-config.json")).unwrap(),
            b"{}"
        );
        assert!(!target.join("runtime/user-config.json").exists());
    }

    #[test]
    #[cfg(unix)]
    fn move_preserves_links_without_following_them() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        let target = directory.path().join("data");
        write(&root, "runtime/lib/service", b"service");
        fs::set_permissions(
            root.join("runtime/lib/service"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("service", root.join("runtime/lib/relative")).unwrap();
        symlink(
            root.join("runtime/lib/service"),
            root.join("runtime/lib/absolute"),
        )
        .unwrap();
        let external = directory.path().join("external.json");
        let content = serde_json::json!({"path": root.join("runtime/lib/service")}).to_string();
        fs::write(&external, &content).unwrap();
        symlink(&external, root.join("runtime/external.json")).unwrap();
        symlink(
            "../../external.json",
            root.join("runtime/relative-external.json"),
        )
        .unwrap();
        DataMove::new(&root, &target)
            .unwrap()
            .execute(|_| {})
            .unwrap();
        assert_eq!(
            fs::read_link(target.join("runtime/lib/relative")).unwrap(),
            Path::new("service")
        );
        assert_eq!(
            fs::read_link(target.join("runtime/lib/absolute")).unwrap(),
            target.join("runtime/lib/service")
        );
        assert_eq!(fs::read_to_string(&external).unwrap(), content);
        assert_eq!(
            fs::read_to_string(target.join("runtime/relative-external.json")).unwrap(),
            content
        );
        assert_eq!(
            fs::metadata(target.join("runtime/lib/service"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(!root.join("runtime/lib/relative").exists());
    }
}
