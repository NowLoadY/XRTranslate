use std::{
    collections::HashSet,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use xrtranslate_download::{DownloadClient, DownloadSource, DownloadSpec};

use crate::{
    ModelAssetId, ModelAssetsPreflight, ModelAssetsPreflightError, ModelSource, RequiredModelFile,
    ResolvedModelAsset, ResolvedModelAssets,
};

impl ResolvedModelAssets {
    /// Atomically enables one fully downloaded package from a staging
    /// directory. The method intentionally never overwrites an existing
    /// installation.
    pub fn install_from_staging(
        &self,
        id: ModelAssetId,
        staging_directory: impl AsRef<Path>,
    ) -> Result<PathBuf, AtomicInstallError> {
        install_verified_directory(self.asset(id), staging_directory.as_ref())
    }
}

/// Failure while atomically promoting a verified model package.
#[derive(Debug)]
pub enum AtomicInstallError {
    StagingInvalid {
        directory: PathBuf,
        source: ModelAssetsPreflightError,
    },
    DestinationExists(PathBuf),
    CreateParent {
        path: PathBuf,
        source: io::Error,
    },
    Rename {
        staging: PathBuf,
        destination: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for AtomicInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StagingInvalid { directory, source } => {
                write!(
                    formatter,
                    "staged model package at {} is invalid: {source}",
                    directory.display()
                )
            }
            Self::DestinationExists(path) => write!(
                formatter,
                "refusing to overwrite existing model package at {}",
                path.display()
            ),
            Self::CreateParent { path, source } => {
                write!(
                    formatter,
                    "cannot create model parent {}: {source}",
                    path.display()
                )
            }
            Self::Rename {
                staging,
                destination,
                source,
            } => write!(
                formatter,
                "cannot atomically activate staged package {} at {}: {source}",
                staging.display(),
                destination.display()
            ),
        }
    }
}

impl Error for AtomicInstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::StagingInvalid { source, .. } => Some(source),
            Self::CreateParent { source, .. } | Self::Rename { source, .. } => Some(source),
            Self::DestinationExists(_) => None,
        }
    }
}

pub(crate) fn install_verified_directory(
    target: &ResolvedModelAsset,
    staging_directory: &Path,
) -> Result<PathBuf, AtomicInstallError> {
    let staged = ResolvedModelAsset::new(target.manifest, staging_directory.to_path_buf());
    ModelAssetsPreflight {
        diagnostics: staged.verify_integrity(),
    }
    .into_result()
    .map_err(|source| AtomicInstallError::StagingInvalid {
        directory: staging_directory.to_path_buf(),
        source,
    })?;

    let destination = target.directory.clone();
    if destination.exists() {
        return Err(AtomicInstallError::DestinationExists(destination));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| AtomicInstallError::CreateParent {
            path: destination.clone(),
            source: io::Error::other("model package destination has no parent"),
        })?;
    fs::create_dir_all(parent).map_err(|source| AtomicInstallError::CreateParent {
        path: parent.to_path_buf(),
        source,
    })?;
    fs::rename(staging_directory, &destination).map_err(|source| AtomicInstallError::Rename {
        staging: staging_directory.to_path_buf(),
        destination: destination.clone(),
        source,
    })?;
    Ok(destination)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub asset_id: ModelAssetId,
    pub relative_path: &'static str,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// Download and installation failures that preserve the staging directory for
/// a safe retry. The installer never deletes an active model package.
#[derive(Debug)]
pub enum ModelDownloadError {
    Download(xrtranslate_download::DownloadError),
    StagingDirectory { path: PathBuf, source: io::Error },
    Removal { path: PathBuf, source: io::Error },
    Locked(PathBuf),
    Lock { path: PathBuf, source: io::Error },
    AtomicInstall(AtomicInstallError),
    Archive { path: PathBuf, message: String },
}

impl ModelDownloadError {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Download(error) if error.is_cancelled())
    }
}

impl fmt::Display for ModelDownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download(error) => error.fmt(formatter),
            Self::StagingDirectory { path, source } => {
                write!(
                    formatter,
                    "cannot create model staging directory {}: {source}",
                    path.display()
                )
            }
            Self::Removal { path, source } => {
                write!(
                    formatter,
                    "cannot remove model resource {}: {source}",
                    path.display()
                )
            }
            Self::Locked(path) => write!(
                formatter,
                "another model installer already owns the package lock at {}",
                path.display()
            ),
            Self::Lock { path, source } => {
                write!(
                    formatter,
                    "cannot acquire package lock {}: {source}",
                    path.display()
                )
            }
            Self::AtomicInstall(error) => error.fmt(formatter),
            Self::Archive { path, message } => {
                write!(
                    formatter,
                    "cannot extract model archive {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl Error for ModelDownloadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Download(error) => Some(error),
            Self::StagingDirectory { source, .. }
            | Self::Removal { source, .. }
            | Self::Lock { source, .. } => Some(source),
            Self::AtomicInstall(error) => Some(error),
            Self::Locked(_) | Self::Archive { .. } => None,
        }
    }
}

/// Native downloader for the compiled, immutable GGUF manifest.
#[derive(Clone, Debug)]
pub struct NativeModelInstaller {
    assets: ResolvedModelAssets,
    client: DownloadClient,
}

impl NativeModelInstaller {
    pub fn new(assets: ResolvedModelAssets) -> Result<Self, ModelDownloadError> {
        Self::with_proxy(assets, None)
    }

    pub fn with_proxy(
        assets: ResolvedModelAssets,
        proxy_url: Option<&str>,
    ) -> Result<Self, ModelDownloadError> {
        Self::with_download_source(assets, proxy_url, DownloadSource::Official)
    }

    pub fn with_download_source(
        assets: ResolvedModelAssets,
        proxy_url: Option<&str>,
        source: DownloadSource,
    ) -> Result<Self, ModelDownloadError> {
        let client = DownloadClient::with_proxy_and_source(
            concat!("xrtranslate-assets/", env!("CARGO_PKG_VERSION")),
            proxy_url,
            source,
        )
        .map_err(ModelDownloadError::Download)?;
        Ok(Self { assets, client })
    }

    pub fn with_download_source_and_cancellation(
        assets: ResolvedModelAssets,
        proxy_url: Option<&str>,
        source: DownloadSource,
        cancellation: xrtranslate_download::DownloadCancellation,
    ) -> Result<Self, ModelDownloadError> {
        let client = DownloadClient::with_proxy_source_and_cancellation(
            concat!("xrtranslate-assets/", env!("CARGO_PKG_VERSION")),
            proxy_url,
            source,
            cancellation,
        )
        .map_err(ModelDownloadError::Download)?;
        Ok(Self { assets, client })
    }

    /// Downloads exactly one catalog package. `on_progress` is called for
    /// resumed bytes too, so a UI can render a stable progress bar.
    pub async fn install(
        &self,
        id: ModelAssetId,
        mut on_progress: impl FnMut(DownloadProgress),
    ) -> Result<PathBuf, ModelDownloadError> {
        let target = self.asset(id);
        if target.directory().exists() {
            if target.verify_integrity().is_empty() {
                return Ok(target.directory().to_path_buf());
            }
            return Err(ModelDownloadError::AtomicInstall(
                AtomicInstallError::DestinationExists(target.directory().to_path_buf()),
            ));
        }
        let staging = staging_directory(target);
        let staging_parent = staging.parent().expect("staging directory has parent");
        fs::create_dir_all(staging_parent).map_err(|source| {
            ModelDownloadError::StagingDirectory {
                path: staging_parent.to_path_buf(),
                source,
            }
        })?;
        let _lock = InstallLock::acquire(staging_parent, id)?;
        prune_obsolete_model_staging(staging_parent, &staging, id)?;
        fs::create_dir_all(&staging).map_err(|source| ModelDownloadError::StagingDirectory {
            path: staging.clone(),
            source,
        })?;

        let archive = target.manifest().source.archive;
        let total_bytes = target.manifest().download_bytes();
        let mut completed_bytes = 0_u64;
        if let Some(archive) = archive {
            self.download_and_extract_archive(id, archive, &staging, total_bytes, &mut on_progress)
                .await?;
            completed_bytes = archive.bytes;
        }
        for file in target.manifest().required_files {
            if archive.is_some_and(|archive| {
                archive
                    .entries
                    .iter()
                    .any(|entry| entry.relative_path == file.relative_path)
            }) {
                continue;
            }
            self.download_file(
                *file,
                AssetDownloadContext {
                    id,
                    source: target.manifest().source,
                    staging: &staging,
                    completed_bytes,
                    total_bytes,
                },
                &mut on_progress,
            )
            .await?;
            completed_bytes = completed_bytes.saturating_add(file.bytes);
        }
        if let Some(archive) = archive {
            remove_downloaded_archive(&staging, archive.filename)?;
        }
        self.assets
            .install_from_staging(id, staging)
            .map_err(ModelDownloadError::AtomicInstall)
    }

    fn asset(&self, id: ModelAssetId) -> &ResolvedModelAsset {
        self.assets.asset(id)
    }

    async fn download_and_extract_archive(
        &self,
        id: ModelAssetId,
        archive: crate::ModelArchiveSource,
        staging: &Path,
        total_bytes: u64,
        on_progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), ModelDownloadError> {
        validate_archive_layout(
            archive.filename,
            archive.entries,
            self.asset(id).manifest().required_files,
        )
        .map_err(|message| ModelDownloadError::Archive {
            path: staging.to_path_buf(),
            message,
        })?;
        let downloads = staging.join(".downloads");
        fs::create_dir_all(&downloads).map_err(|source| ModelDownloadError::StagingDirectory {
            path: downloads.clone(),
            source,
        })?;
        let path = downloads.join(archive.filename);
        self.client
            .download_to(
                DownloadSpec::verified(
                    "model package archive",
                    archive.url,
                    archive.bytes,
                    archive.sha256,
                ),
                &path,
                |progress| {
                    on_progress(DownloadProgress {
                        asset_id: id,
                        relative_path: archive.filename,
                        downloaded_bytes: progress.downloaded_bytes,
                        total_bytes,
                    });
                },
            )
            .await
            .map_err(ModelDownloadError::Download)?;
        extract_archive_entries(&path, staging, archive.entries)?;
        Ok(())
    }

    async fn download_file(
        &self,
        file: RequiredModelFile,
        context: AssetDownloadContext<'_>,
        on_progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<(), ModelDownloadError> {
        let complete = context.staging.join(file.relative_path);
        let url = context.source.hugging_face_resolve_url(file.relative_path);
        self.client
            .download_to(
                DownloadSpec::verified(file.purpose, &url, file.bytes, file.sha256),
                &complete,
                |progress| {
                    on_progress(DownloadProgress {
                        asset_id: context.id,
                        relative_path: file.relative_path,
                        downloaded_bytes: context
                            .completed_bytes
                            .saturating_add(progress.downloaded_bytes),
                        total_bytes: context.total_bytes,
                    });
                },
            )
            .await
            .map_err(ModelDownloadError::Download)
    }
}

fn remove_downloaded_archive(staging: &Path, filename: &str) -> Result<(), ModelDownloadError> {
    let downloads = staging.join(".downloads");
    let path = safe_relative_file(&downloads, filename).map_err(|message| {
        ModelDownloadError::Archive {
            path: downloads.clone(),
            message,
        }
    })?;
    fs::remove_file(&path).map_err(|source| ModelDownloadError::Removal {
        path: path.clone(),
        source,
    })?;
    fs::remove_dir(&downloads).map_err(|source| ModelDownloadError::Removal {
        path: downloads,
        source,
    })
}

fn extract_archive_entries(
    archive_path: &Path,
    staging: &Path,
    entries: &[crate::ModelArchiveEntry],
) -> Result<(), ModelDownloadError> {
    let file = fs::File::open(archive_path).map_err(|error| ModelDownloadError::Archive {
        path: archive_path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| ModelDownloadError::Archive {
        path: archive_path.to_path_buf(),
        message: error.to_string(),
    })?;
    for entry in entries {
        let mut source =
            archive
                .by_name(entry.archive_path)
                .map_err(|error| ModelDownloadError::Archive {
                    path: archive_path.to_path_buf(),
                    message: format!("missing {}: {error}", entry.archive_path),
                })?;
        if !source.is_file() {
            return Err(ModelDownloadError::Archive {
                path: archive_path.to_path_buf(),
                message: format!("{} is not a regular file", entry.archive_path),
            });
        }
        let destination = safe_relative_file(staging, entry.relative_path).map_err(|message| {
            ModelDownloadError::Archive {
                path: archive_path.to_path_buf(),
                message,
            }
        })?;
        let parent = destination
            .parent()
            .expect("archive destination has a staging parent");
        fs::create_dir_all(parent).map_err(|error| ModelDownloadError::Archive {
            path: archive_path.to_path_buf(),
            message: format!("cannot create {}: {error}", parent.display()),
        })?;
        let mut output =
            fs::File::create(&destination).map_err(|error| ModelDownloadError::Archive {
                path: archive_path.to_path_buf(),
                message: format!("cannot create {}: {error}", destination.display()),
            })?;
        io::copy(&mut source, &mut output).map_err(|error| ModelDownloadError::Archive {
            path: archive_path.to_path_buf(),
            message: format!("cannot write {}: {error}", destination.display()),
        })?;
    }
    Ok(())
}

fn validate_archive_layout(
    filename: &str,
    entries: &[crate::ModelArchiveEntry],
    required_files: &[RequiredModelFile],
) -> Result<(), String> {
    if Path::new(filename).components().count() != 1 {
        return Err(format!(
            "archive filename {filename:?} is not a plain file name"
        ));
    }
    safe_relative_file(Path::new("."), filename)?;
    if entries.is_empty() {
        return Err("archive declares no model files".into());
    }
    let required = required_files
        .iter()
        .map(|file| file.relative_path)
        .collect::<HashSet<_>>();
    let mut destinations = HashSet::new();
    let mut sources = HashSet::new();
    for entry in entries {
        safe_relative_file(Path::new("."), entry.relative_path)?;
        safe_relative_file(Path::new("."), entry.archive_path)?;
        if !required.contains(entry.relative_path) {
            return Err(format!(
                "archive destination {:?} is not a required model file",
                entry.relative_path
            ));
        }
        if !destinations.insert(entry.relative_path) {
            return Err(format!(
                "archive destination {:?} is declared more than once",
                entry.relative_path
            ));
        }
        if !sources.insert(entry.archive_path) {
            return Err(format!(
                "archive source {:?} is declared more than once",
                entry.archive_path
            ));
        }
    }
    Ok(())
}

fn safe_relative_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe model archive path: {}", relative.display()));
    }
    Ok(root.join(relative))
}

/// Removes only the resumable staging owned by one immutable model package.
/// Callers must first stop the package's worker so no `.part` file is open.
pub fn clear_model_staging(
    assets: &ResolvedModelAssets,
    id: ModelAssetId,
) -> Result<(), ModelDownloadError> {
    let staging = staging_directory(assets.asset(id));
    match fs::remove_dir_all(&staging) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ModelDownloadError::StagingDirectory {
            path: staging,
            source,
        }),
    }
}

/// Deletes exactly one model package and its resumable staging. Manifest files
/// are removed individually so a custom package directory containing unrelated
/// user data is never recursively erased.
pub fn remove_model_asset(
    assets: &ResolvedModelAssets,
    id: ModelAssetId,
) -> Result<(), ModelDownloadError> {
    let target = assets.asset(id);
    let staging = staging_directory(target);
    let staging_parent = staging.parent().expect("staging directory has parent");
    fs::create_dir_all(staging_parent).map_err(|source| ModelDownloadError::StagingDirectory {
        path: staging_parent.to_path_buf(),
        source,
    })?;
    let _lock = InstallLock::acquire(staging_parent, id)?;

    clear_model_staging(assets, id)?;
    for file in target.manifest().required_files {
        let path = target.directory().join(file.relative_path);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(ModelDownloadError::Removal { path, source }),
        }
        remove_empty_parents(path.parent(), target.directory())?;
    }
    match fs::remove_dir(target.directory()) {
        Ok(()) => Ok(()),
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(ModelDownloadError::Removal {
            path: target.directory().to_path_buf(),
            source,
        }),
    }
}

fn remove_empty_parents(
    mut directory: Option<&Path>,
    boundary: &Path,
) -> Result<(), ModelDownloadError> {
    while let Some(path) = directory {
        if path == boundary {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => directory = path.parent(),
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                break;
            }
            Err(source) => {
                return Err(ModelDownloadError::Removal {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }
    Ok(())
}

fn staging_directory(target: &ResolvedModelAsset) -> PathBuf {
    let parent = target
        .directory()
        .parent()
        .unwrap_or_else(|| Path::new("."));
    parent.join(".xrtranslate-staging").join(format!(
        "{}-{}",
        target.manifest().id.as_str(),
        target.manifest().source.revision
    ))
}

#[derive(Clone, Copy)]
struct AssetDownloadContext<'a> {
    id: ModelAssetId,
    source: ModelSource,
    staging: &'a Path,
    completed_bytes: u64,
    total_bytes: u64,
}

fn prune_obsolete_model_staging(
    staging_parent: &Path,
    current: &Path,
    id: ModelAssetId,
) -> Result<(), ModelDownloadError> {
    let prefix = format!("{}-", id.as_str());
    let entries =
        fs::read_dir(staging_parent).map_err(|source| ModelDownloadError::StagingDirectory {
            path: staging_parent.to_path_buf(),
            source,
        })?;
    for entry in entries {
        let entry = entry.map_err(|source| ModelDownloadError::StagingDirectory {
            path: staging_parent.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path != current
            && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            && entry.file_name().to_string_lossy().starts_with(&prefix)
        {
            fs::remove_dir_all(&path)
                .map_err(|source| ModelDownloadError::StagingDirectory { path, source })?;
        }
    }
    Ok(())
}

struct InstallLock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl InstallLock {
    fn acquire(staging_parent: &Path, id: ModelAssetId) -> Result<Self, ModelDownloadError> {
        let path = staging_parent.join(format!("{}.lock", id.as_str()));
        match open_install_lock(&path) {
            Ok(mut file) => {
                use std::io::Write;
                file.set_len(0).map_err(|source| ModelDownloadError::Lock {
                    path: path.clone(),
                    source,
                })?;
                writeln!(
                    file,
                    "pid={} created_unix={}",
                    std::process::id(),
                    unix_seconds()
                )
                .map_err(|source| ModelDownloadError::Lock {
                    path: path.clone(),
                    source,
                })?;
                Ok(Self {
                    path,
                    file: Some(file),
                })
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::PermissionDenied
                ) =>
            {
                Err(ModelDownloadError::Locked(path))
            }
            Err(source) => Err(ModelDownloadError::Lock { path, source }),
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
fn open_install_lock(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).share_mode(0);
    options.open(path)
}

#[cfg(not(windows))]
fn open_install_lock(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest_for;

    #[test]
    fn staging_is_adjacent_to_the_destination_for_same_volume_activation() {
        let target = ResolvedModelAsset::new(
            manifest_for(ModelAssetId::Qwen3AsrGguf),
            PathBuf::from("X:/custom-models/Qwen3-ASR-1.7B-GGUF"),
        );

        assert_eq!(
            staging_directory(&target),
            PathBuf::from("X:/custom-models/.xrtranslate-staging").join(format!(
                "qwen3-asr-gguf-{}",
                target.manifest().source.revision
            ))
        );
    }

    #[test]
    fn archive_layout_rejects_escaping_and_undeclared_destinations() {
        let required = [RequiredModelFile {
            role: crate::ModelFileRole::Weights,
            relative_path: "models/model.onnx",
            purpose: "fixture",
            bytes: 1,
            sha256: "0",
        }];
        assert!(
            validate_archive_layout(
                "fixture.zip",
                &[crate::ModelArchiveEntry {
                    relative_path: "../escape.onnx",
                    archive_path: "payload/model.onnx",
                }],
                &required,
            )
            .unwrap_err()
            .contains("unsafe")
        );
        assert!(
            validate_archive_layout(
                "fixture.zip",
                &[crate::ModelArchiveEntry {
                    relative_path: "models/extra.onnx",
                    archive_path: "payload/model.onnx",
                }],
                &required,
            )
            .unwrap_err()
            .contains("not a required model file")
        );
    }

    #[test]
    fn archive_layout_requires_unique_source_and_destination_paths() {
        let required = [RequiredModelFile {
            role: crate::ModelFileRole::Weights,
            relative_path: "models/model.onnx",
            purpose: "fixture",
            bytes: 1,
            sha256: "0",
        }];
        let duplicate = crate::ModelArchiveEntry {
            relative_path: "models/model.onnx",
            archive_path: "payload/model.onnx",
        };
        assert!(
            validate_archive_layout("fixture.zip", &[duplicate, duplicate], &required)
                .unwrap_err()
                .contains("declared more than once")
        );
    }
}
