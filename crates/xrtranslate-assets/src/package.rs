//! One installation transaction for built-in and plugin-provided model packages.
use crate::install::{InstallLock, safe_relative_file};
use crate::{ModelAssetManifest, ModelDownloadError};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use xrtranslate_download::{DownloadClient, DownloadSpec};

#[derive(Clone, Debug)]
pub struct PackageFile {
    pub path: String,
    pub url: String,
    pub bytes: u64,
}
#[derive(Clone, Debug)]
pub struct PackageArchive {
    pub filename: String,
    pub url: String,
    pub bytes: u64,
    pub entries: Vec<(String, String)>,
}
#[derive(Clone, Debug)]
pub struct ModelPackage {
    pub id: String,
    pub revision: String,
    pub files: Vec<PackageFile>,
    pub archive: Option<PackageArchive>,
}
impl From<&ModelAssetManifest> for ModelPackage {
    fn from(manifest: &ModelAssetManifest) -> Self {
        Self {
            id: manifest.id.as_str().into(),
            revision: manifest.source.revision.into(),
            files: manifest
                .required_files
                .iter()
                .map(|f| PackageFile {
                    path: f.relative_path.into(),
                    url: manifest.source.file_url(f.relative_path),
                    bytes: f.bytes,
                })
                .collect(),
            archive: manifest.source.archive.map(|a| PackageArchive {
                filename: a.filename.into(),
                url: a.url.into(),
                bytes: a.bytes,
                entries: a
                    .entries
                    .iter()
                    .map(|e| (e.relative_path.into(), e.archive_path.into()))
                    .collect(),
            }),
        }
    }
}
impl ModelPackage {
    pub fn download_bytes(&self) -> u64 {
        self.archive.as_ref().map_or(0, |a| a.bytes)
            + self
                .files
                .iter()
                .filter(|f| !self.archived(&f.path))
                .map(|f| f.bytes)
                .sum::<u64>()
    }
    fn archived(&self, path: &str) -> bool {
        self.archive
            .as_ref()
            .is_some_and(|a| a.entries.iter().any(|(p, _)| p == path))
    }
    pub fn check(&self, directory: &Path) -> Result<bool, ModelDownloadError> {
        let mut complete = true;
        for file in &self.files {
            let path =
                safe_relative_file(directory, &file.path).map_err(ModelDownloadError::Manifest)?;
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() && meta.len() == file.bytes => {}
                Ok(_) => {
                    return Err(ModelDownloadError::Manifest(format!(
                        "{}: expected {} bytes in a regular file",
                        path.display(),
                        file.bytes
                    )));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => complete = false,
                Err(source) => return Err(ModelDownloadError::StagingDirectory { path, source }),
            }
        }
        Ok(complete)
    }
    pub async fn install(
        &self,
        client: &DownloadClient,
        destination: &Path,
        mut progress: impl FnMut(&str, u64, u64),
    ) -> Result<PathBuf, ModelDownloadError> {
        self.validate()?;
        if self.check(destination)? {
            return Ok(destination.into());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| ModelDownloadError::Manifest("Model directory has no parent".into()))?;
        let staging_parent = parent.join(".xrtranslate-staging");
        fs::create_dir_all(&staging_parent).map_err(|source| {
            ModelDownloadError::StagingDirectory {
                path: staging_parent.clone(),
                source,
            }
        })?;
        let _lock = InstallLock::acquire_named(&staging_parent, &self.id)?;
        let staging = staging_parent.join(format!("{}-{}", self.id, self.revision));
        crate::install::prune_obsolete_model_staging_named(&staging_parent, &staging, &self.id)?;
        fs::create_dir_all(&staging).map_err(|source| ModelDownloadError::StagingDirectory {
            path: staging.clone(),
            source,
        })?;
        let missing: HashSet<_> = self
            .files
            .iter()
            .filter(|f| !destination.join(&f.path).is_file())
            .map(|f| f.path.as_str())
            .collect();
        let total = self.download_bytes();
        let mut completed = 0;
        if let Some(archive) = &self.archive {
            if archive
                .entries
                .iter()
                .any(|(path, _)| missing.contains(path.as_str()))
            {
                let downloads = staging.join(".downloads");
                fs::create_dir_all(&downloads).map_err(|source| {
                    ModelDownloadError::StagingDirectory {
                        path: downloads.clone(),
                        source,
                    }
                })?;
                let path = downloads.join(&archive.filename);
                client
                    .download_to(
                        DownloadSpec::new("model archive", &archive.url, archive.bytes),
                        &path,
                        |p| progress(&archive.filename, p.downloaded_bytes, total),
                    )
                    .await
                    .map_err(ModelDownloadError::Download)?;
                crate::install::extract_archive_entries(
                    &path,
                    &staging,
                    archive
                        .entries
                        .iter()
                        .map(|(a, b)| (a.as_str(), b.as_str())),
                )?;
                crate::install::remove_downloaded_archive(&staging, &archive.filename)?;
            }
            completed = archive.bytes;
        }
        for file in self.files.iter().filter(|f| !self.archived(&f.path)) {
            if missing.contains(file.path.as_str()) {
                client
                    .download_to(
                        DownloadSpec::new(&file.path, &file.url, file.bytes),
                        &staging.join(&file.path),
                        |p| progress(&file.path, completed + p.downloaded_bytes, total),
                    )
                    .await
                    .map_err(ModelDownloadError::Download)?;
            } else {
                progress(&file.path, completed + file.bytes, total);
            }
            completed += file.bytes;
        }
        client
            .ensure_not_cancelled("model package")
            .map_err(ModelDownloadError::Download)?;
        // Validate every new file before publishing any of them.
        for file in self
            .files
            .iter()
            .filter(|f| missing.contains(f.path.as_str()))
        {
            let path = staging.join(&file.path);
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() == file.bytes) {
                return Err(ModelDownloadError::Manifest(format!(
                    "{}: expected {} bytes",
                    path.display(),
                    file.bytes
                )));
            }
        }
        if destination.exists() {
            for file in self
                .files
                .iter()
                .filter(|f| missing.contains(f.path.as_str()))
            {
                let target = destination.join(&file.path);
                fs::create_dir_all(target.parent().unwrap()).map_err(|source| {
                    ModelDownloadError::StagingDirectory {
                        path: target.clone(),
                        source,
                    }
                })?;
                fs::hard_link(staging.join(&file.path), &target).map_err(|source| {
                    ModelDownloadError::StagingDirectory {
                        path: target,
                        source,
                    }
                })?;
            }
            fs::remove_dir_all(&staging).map_err(|source| ModelDownloadError::Removal {
                path: staging,
                source,
            })?;
        } else {
            fs::rename(&staging, destination).map_err(|source| {
                ModelDownloadError::StagingDirectory {
                    path: destination.into(),
                    source,
                }
            })?;
        }
        Ok(destination.into())
    }
    fn validate(&self) -> Result<(), ModelDownloadError> {
        for name in [&self.id, &self.revision] {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                || name == "."
                || name == ".."
            {
                return Err(ModelDownloadError::Manifest(
                    "Invalid package identity or revision".into(),
                ));
            }
        }
        if self.files.is_empty() {
            return Err(ModelDownloadError::Manifest("Empty model package".into()));
        }
        let mut names = HashSet::new();
        for file in &self.files {
            safe_relative_file(Path::new("."), &file.path).map_err(ModelDownloadError::Manifest)?;
            if !names.insert(&file.path) {
                return Err(ModelDownloadError::Manifest("Repeated model file".into()));
            }
        }
        if let Some(archive) = &self.archive {
            safe_relative_file(Path::new("."), &archive.filename)
                .map_err(ModelDownloadError::Manifest)?;
            if Path::new(&archive.filename).components().count() != 1 {
                return Err(ModelDownloadError::Manifest(
                    "Archive filename must be a single name".into(),
                ));
            }
            let mut destinations = HashSet::new();
            for (path, source) in &archive.entries {
                safe_relative_file(Path::new("."), source).map_err(ModelDownloadError::Manifest)?;
                if !names.contains(path) || !destinations.insert(path) {
                    return Err(ModelDownloadError::Manifest("Invalid archive entry".into()));
                }
            }
        }
        Ok(())
    }
}
