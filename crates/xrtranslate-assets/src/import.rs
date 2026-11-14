//! Local packages use the same manifest, lock and publication as downloads.
use crate::{ModelDownloadError, ModelPackage, install::InstallLock};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use xrtranslate_download::{DownloadCancellation, DownloadError};

impl ModelPackage {
    /// Copies a model folder or ZIP into its resolved storage directory.
    /// Only declared model files are imported; the source is never modified.
    pub fn import(
        &self,
        source: &Path,
        destination: &Path,
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(&str, u64, u64),
    ) -> Result<PathBuf, ModelDownloadError> {
        self.validate()?;
        ensure_running(cancellation)?;
        let mut source = Source::open(self, source)?;
        let parent = destination
            .parent()
            .ok_or_else(|| ModelDownloadError::Manifest("Model directory has no parent".into()))?;
        let staging_parent = parent.join(".xrtranslate-staging");
        fs::create_dir_all(&staging_parent).map_err(|e| file_error(&staging_parent, e))?;
        let _lock = InstallLock::acquire_named(&staging_parent, &self.id)?;
        if self.check(destination)? {
            return Ok(destination.into());
        }
        let staging = tempfile::Builder::new()
            .prefix(&format!("{}-import-", self.id))
            .tempdir_in(&staging_parent)
            .map_err(|e| file_error(&staging_parent, e))?;
        let missing: HashSet<_> = self
            .files
            .iter()
            .filter(|f| !destination.join(&f.path).is_file())
            .map(|f| f.path.as_str())
            .collect();
        let total = self.files.iter().map(|file| file.bytes).sum();
        let mut completed = 0;
        let mut buffer = vec![0_u8; 1024 * 1024];
        for file in &self.files {
            ensure_running(cancellation)?;
            if missing.contains(file.path.as_str()) {
                let output = staging.path().join(&file.path);
                fs::create_dir_all(output.parent().unwrap()).map_err(|e| file_error(&output, e))?;
                let mut input = source.read(&file.path)?;
                let mut writer = File::create(&output).map_err(|e| file_error(&output, e))?;
                let mut copied = 0;
                loop {
                    ensure_running(cancellation)?;
                    let count = input
                        .read(&mut buffer)
                        .map_err(|e| file_error(&output, e))?;
                    if count == 0 {
                        break;
                    }
                    copied += count as u64;
                    if copied > file.bytes {
                        return Err(ModelDownloadError::Manifest(format!(
                            "{}: unexpected model file size",
                            file.path
                        )));
                    }
                    writer
                        .write_all(&buffer[..count])
                        .map_err(|e| file_error(&output, e))?;
                    progress(&file.path, completed + copied, total);
                }
                if copied != file.bytes {
                    return Err(ModelDownloadError::Manifest(format!(
                        "{}: incomplete model file",
                        file.path
                    )));
                }
                writer.sync_all().map_err(|e| file_error(&output, e))?;
            }
            completed += file.bytes;
            progress(&file.path, completed, total);
        }
        ensure_running(cancellation)?;
        self.publish(staging.path(), destination, &missing)
    }
}

enum Source {
    Directory(PathBuf),
    Zip {
        archive: zip::ZipArchive<File>,
        entries: HashMap<String, usize>,
        prefix: String,
    },
}

impl Source {
    fn open(package: &ModelPackage, path: &Path) -> Result<Self, ModelDownloadError> {
        if path.is_dir() {
            let root = path.canonicalize().map_err(|e| file_error(path, e))?;
            let mut candidates = vec![root.clone()];
            // Accept the model itself, its containing folder, or models/<model>.
            for depth in 0..=2 {
                let matching: Vec<_> = candidates
                    .iter()
                    .filter(|candidate| {
                        package.check(candidate).unwrap_or(false)
                            && package.files.iter().all(|file| {
                                candidate
                                    .join(&file.path)
                                    .canonicalize()
                                    .is_ok_and(|p| p.starts_with(candidate))
                            })
                    })
                    .cloned()
                    .collect();
                if !matching.is_empty() {
                    return unique(matching, &package.id).map(Self::Directory);
                }
                if depth == 2 {
                    break;
                }
                let mut children = Vec::new();
                for candidate in candidates {
                    for entry in fs::read_dir(&candidate).map_err(|e| file_error(&candidate, e))? {
                        let entry = entry.map_err(|e| file_error(&candidate, e))?;
                        let kind = entry
                            .file_type()
                            .map_err(|e| file_error(&entry.path(), e))?;
                        if kind.is_dir()
                            && !kind.is_symlink()
                            && !entry.file_name().to_string_lossy().starts_with('.')
                        {
                            let child = entry
                                .path()
                                .canonicalize()
                                .map_err(|e| file_error(&entry.path(), e))?;
                            if child.starts_with(&root) && children.len() < 256 {
                                children.push(child);
                            }
                        }
                    }
                }
                candidates = children;
            }
            return Err(missing(&package.id));
        }
        let file = File::open(path).map_err(|e| file_error(path, e))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| archive_error(path, e))?;
        let mut files = HashMap::new();
        for index in 0..archive.len() {
            let entry = archive
                .by_index(index)
                .map_err(|e| archive_error(path, e))?;
            let name = entry.name().replace('\\', "/");
            crate::install::safe_relative_file(Path::new(""), &name)
                .map_err(ModelDownloadError::Manifest)?;
            if entry.is_dir() {
                continue;
            }
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(ModelDownloadError::Manifest(
                    "Model archives cannot contain symbolic links".into(),
                ));
            }
            if files.insert(name, (index, entry.size())).is_some() {
                return Err(ModelDownloadError::Manifest(
                    "Repeated file in model archive".into(),
                ));
            }
        }
        let first = &package.files[0].path;
        let prefixes: Vec<_> = files
            .keys()
            .filter_map(|name| {
                if name == first {
                    Some(String::new())
                } else {
                    name.strip_suffix(&format!("/{first}"))
                        .map(|p| format!("{p}/"))
                }
            })
            .filter(|prefix| {
                package.files.iter().all(|file| {
                    files
                        .get(&format!("{prefix}{}", file.path))
                        .is_some_and(|(_, bytes)| *bytes == file.bytes)
                })
            })
            .collect();
        let prefix = unique(prefixes, &package.id)?;
        Ok(Self::Zip {
            archive,
            entries: files
                .into_iter()
                .map(|(p, (index, _))| (p, index))
                .collect(),
            prefix,
        })
    }

    fn read(&mut self, relative: &str) -> Result<Box<dyn Read + '_>, ModelDownloadError> {
        match self {
            Self::Directory(root) => {
                let path = root.join(relative);
                Ok(Box::new(
                    File::open(&path).map_err(|e| file_error(&path, e))?,
                ))
            }
            Self::Zip {
                archive,
                entries,
                prefix,
            } => {
                let name = format!("{prefix}{relative}");
                Ok(Box::new(
                    archive
                        .by_index(entries[&name])
                        .map_err(|e| archive_error(Path::new(&name), e))?,
                ))
            }
        }
    }
}

fn unique<T>(mut values: Vec<T>, id: &str) -> Result<T, ModelDownloadError> {
    match values.len() {
        1 => Ok(values.pop().unwrap()),
        0 => Err(missing(id)),
        _ => Err(ModelDownloadError::Manifest(format!(
            "Multiple copies of {id} found. Select one model folder."
        ))),
    }
}

fn missing(id: &str) -> ModelDownloadError {
    ModelDownloadError::Manifest(format!(
        "The selected folder or ZIP does not contain the complete {id} model."
    ))
}

fn file_error(path: &Path, source: io::Error) -> ModelDownloadError {
    ModelDownloadError::Download(DownloadError::FileIo {
        path: path.into(),
        source,
    })
}

fn archive_error(path: &Path, error: zip::result::ZipError) -> ModelDownloadError {
    ModelDownloadError::Archive {
        path: path.into(),
        message: error.to_string(),
    }
}

fn ensure_running(cancellation: &DownloadCancellation) -> Result<(), ModelDownloadError> {
    if cancellation.is_cancelled() {
        Err(ModelDownloadError::Download(DownloadError::Cancelled {
            label: "model import".into(),
        }))
    } else {
        Ok(())
    }
}
