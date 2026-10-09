use std::{fmt, fs, io, path::PathBuf};

use crate::{ModelAssetId, RequiredModelFile, ResolvedModelAsset, ResolvedModelAssets};

impl ResolvedModelAssets {
    /// Complete, readable catalog packages at their configured install paths.
    /// Checks required files and sizes without reading model weights.
    pub fn installed_assets(&self) -> impl Iterator<Item = &ResolvedModelAsset> {
        self.catalog_assets().filter(|asset| {
            asset
                .manifest()
                .required_files
                .iter()
                .all(|file| asset.check_file(*file).is_none())
        })
    }

    /// Checks every active runtime file for presence, readability, and size.
    #[must_use]
    pub fn check(&self) -> ModelAssetsPreflight {
        let diagnostics = self
            .active_assets()
            .flat_map(ResolvedModelAsset::check)
            .collect();
        ModelAssetsPreflight { diagnostics }
    }
}

impl ResolvedModelAsset {
    #[must_use]
    pub fn check(&self) -> Vec<ModelAssetDiagnostic> {
        self.manifest
            .required_files
            .iter()
            .filter_map(|required_file| self.check_file(*required_file))
            .collect()
    }

    fn check_file(&self, required_file: RequiredModelFile) -> Option<ModelAssetDiagnostic> {
        let path = self.directory.join(required_file.relative_path);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Some(self.diagnostic(required_file, path, ModelAssetProblem::Missing));
            }
            Err(error) => {
                return Some(self.diagnostic(
                    required_file,
                    path,
                    ModelAssetProblem::MetadataUnavailable {
                        kind: error.kind(),
                        message: error.to_string(),
                    },
                ));
            }
        };

        if !metadata.is_file() {
            return Some(self.diagnostic(required_file, path, ModelAssetProblem::NotAFile));
        }

        if metadata.len() != required_file.bytes {
            return Some(self.diagnostic(
                required_file,
                path,
                ModelAssetProblem::SizeMismatch {
                    expected: required_file.bytes,
                    actual: metadata.len(),
                },
            ));
        }

        if let Err(error) = fs::File::open(&path) {
            return Some(self.diagnostic(
                required_file,
                path,
                ModelAssetProblem::Unreadable {
                    kind: error.kind(),
                    message: error.to_string(),
                },
            ));
        }

        None
    }

    fn diagnostic(
        &self,
        required_file: RequiredModelFile,
        path: PathBuf,
        problem: ModelAssetProblem,
    ) -> ModelAssetDiagnostic {
        ModelAssetDiagnostic {
            asset_id: self.manifest.id,
            asset_label: self.manifest.label,
            required_file,
            path,
            problem,
        }
    }
}

/// Result of checking all declared assets before launching llama.cpp.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelAssetsPreflight {
    pub(crate) diagnostics: Vec<ModelAssetDiagnostic>,
}

impl ModelAssetsPreflight {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.diagnostics.is_empty()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[ModelAssetDiagnostic] {
        &self.diagnostics
    }

    /// Turns a failed preflight into an error suitable for the backend's
    /// startup path while retaining every actionable problem.
    pub fn into_result(self) -> Result<(), ModelAssetsPreflightError> {
        if self.is_ready() {
            Ok(())
        } else {
            Err(ModelAssetsPreflightError {
                diagnostics: self.diagnostics,
            })
        }
    }
}

/// An individual problem found while validating a required asset file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelAssetDiagnostic {
    pub asset_id: ModelAssetId,
    pub asset_label: &'static str,
    pub required_file: RequiredModelFile,
    pub path: PathBuf,
    pub problem: ModelAssetProblem,
}

impl fmt::Display for ModelAssetDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}) requires {} at {}: {}",
            self.asset_label,
            self.asset_id,
            self.required_file.purpose,
            self.path.display(),
            self.problem
        )
    }
}

/// Why an expected model file cannot be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelAssetProblem {
    Missing,
    NotAFile,
    MetadataUnavailable {
        kind: io::ErrorKind,
        message: String,
    },
    Unreadable {
        kind: io::ErrorKind,
        message: String,
    },
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
}

impl fmt::Display for ModelAssetProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("file is missing"),
            Self::NotAFile => formatter.write_str("path exists but is not a regular file"),
            Self::MetadataUnavailable { message, .. } => {
                write!(formatter, "could not inspect path ({message})")
            }
            Self::Unreadable { message, .. } => {
                write!(formatter, "file cannot be opened ({message})")
            }
            Self::SizeMismatch { expected, actual } => {
                write!(
                    formatter,
                    "file size is {actual} bytes; expected {expected} bytes"
                )
            }
        }
    }
}

/// Failed [`ModelAssetsPreflight`] with all missing or unusable files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelAssetsPreflightError {
    diagnostics: Vec<ModelAssetDiagnostic>,
}

impl ModelAssetsPreflightError {
    #[must_use]
    pub fn diagnostics(&self) -> &[ModelAssetDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for ModelAssetsPreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("default GGUF assets are not ready:")?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n- {diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ModelAssetsPreflightError {}
