use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use xrtranslate_download::{DownloadClient, DownloadSource};

use crate::{
    ModelAssetId, ModelAssetsPreflight, ModelAssetsPreflightError, ResolvedModelAsset,
    ResolvedModelAssets,
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
        diagnostics: staged.check(),
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
    Manifest(String),
    Download(xrtranslate_download::DownloadError),
    StagingDirectory { path: PathBuf, source: io::Error },
    Removal { path: PathBuf, source: io::Error },
    Locked(PathBuf),
    Lock { path: PathBuf, source: io::Error },
    AtomicInstall(AtomicInstallError),
    ExistingInvalid(ModelAssetsPreflightError),
    Archive { path: PathBuf, message: String },
}

impl ModelDownloadError {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Download(error) if error.is_cancelled())
    }

    #[must_use]
    pub fn allows_source_fallback(&self) -> bool {
        matches!(self, Self::Download(error) if error.allows_source_fallback())
    }
}

impl fmt::Display for ModelDownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(message) => formatter.write_str(message),
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
            Self::ExistingInvalid(error) => {
                formatter.write_str("existing model files cannot be reused:")?;
                for diagnostic in error.diagnostics() {
                    write!(formatter, "\n- {diagnostic}")?;
                }
                Ok(())
            }
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
            Self::ExistingInvalid(error) => Some(error),
            Self::Locked(_) | Self::Archive { .. } | Self::Manifest(_) => None,
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
        let target = self.assets.asset(id);
        let manifest = target.manifest();
        crate::ModelPackage::from(manifest)
            .install(
                &self.client,
                target.directory(),
                |path, downloaded_bytes, total_bytes| {
                    let relative_path = manifest
                        .required_files
                        .iter()
                        .find(|f| f.relative_path == path)
                        .map(|f| f.relative_path)
                        .or_else(|| {
                            manifest
                                .source
                                .archive
                                .filter(|a| a.filename == path)
                                .map(|a| a.filename)
                        })
                        .expect("package progress belongs to its manifest");
                    on_progress(DownloadProgress {
                        asset_id: id,
                        relative_path,
                        downloaded_bytes,
                        total_bytes,
                    });
                },
            )
            .await
    }
}

pub(crate) fn remove_downloaded_archive(
    staging: &Path,
    filename: &str,
) -> Result<(), ModelDownloadError> {
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

pub(crate) fn extract_archive_entries<'a>(
    archive_path: &Path,
    staging: &Path,
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), ModelDownloadError> {
    // Finish extraction and release the archive handle before evicting a bad
    // completed download. Partial transfers and filesystem failures stay retryable.
    let result = (|| -> io::Result<()> {
        let file = fs::File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        for (relative_path, archive_path) in entries {
            let mut source = archive.by_name(archive_path).map_err(|error| {
                if matches!(error, zip::result::ZipError::FileNotFound) {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("missing {}: {error}", archive_path),
                    )
                } else {
                    io::Error::from(error)
                }
            })?;
            if !source.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a regular file", archive_path),
                ));
            }
            let destination = safe_relative_file(staging, relative_path)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
            let parent = destination
                .parent()
                .expect("archive destination has a staging parent");
            fs::create_dir_all(parent)?;
            let mut output = fs::File::create(&destination)?;
            io::copy(&mut source, &mut output)?;
        }
        Ok(())
    })();
    result.map_err(|error| {
        if matches!(
            error.kind(),
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
        ) && let Err(source) = fs::remove_file(archive_path)
        {
            return ModelDownloadError::Removal {
                path: archive_path.to_path_buf(),
                source,
            };
        }
        ModelDownloadError::Archive {
            path: archive_path.to_path_buf(),
            message: error.to_string(),
        }
    })
}

pub(crate) fn safe_relative_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
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
    crate::ModelPackage::from(target.manifest()).remove(target.directory())
}

pub(crate) fn remove_empty_parents(
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

pub(crate) fn prune_obsolete_model_staging_named(
    staging_parent: &Path,
    current: &Path,
    id: &str,
) -> Result<(), ModelDownloadError> {
    let prefix = format!("{id}-");
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

pub(crate) struct InstallLock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl InstallLock {
    pub(crate) fn acquire_named(
        staging_parent: &Path,
        id: &str,
    ) -> Result<Self, ModelDownloadError> {
        let path = staging_parent.join(format!("{id}.lock"));
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
