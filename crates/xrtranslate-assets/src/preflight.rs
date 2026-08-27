use std::{
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{ModelAssetId, RequiredModelFile, ResolvedModelAsset, ResolvedModelAssets};

impl ResolvedModelAssets {
    /// Checks every active runtime file without modifying its contents.
    #[must_use]
    pub fn check(&self) -> ModelAssetsPreflight {
        let diagnostics = self
            .active_assets()
            .flat_map(ResolvedModelAsset::check)
            .collect();
        ModelAssetsPreflight { diagnostics }
    }

    /// Performs the expensive cryptographic verification explicitly requested
    /// by an installer, update flow, or user-facing “Verify models” command.
    ///
    /// Normal backend startup deliberately calls [`Self::check`] instead so it
    /// does not re-read more than 3 GB of model files on every launch.
    #[must_use]
    pub fn verify_integrity(&self) -> ModelAssetsPreflight {
        let diagnostics = self
            .active_assets()
            .flat_map(ResolvedModelAsset::verify_integrity)
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

    #[must_use]
    pub fn verify_integrity(&self) -> Vec<ModelAssetDiagnostic> {
        self.manifest
            .required_files
            .iter()
            .filter_map(|required_file| self.verify_file(*required_file))
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

    fn verify_file(&self, required_file: RequiredModelFile) -> Option<ModelAssetDiagnostic> {
        let path = self.directory.join(required_file.relative_path);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                let problem = if error.kind() == io::ErrorKind::NotFound {
                    ModelAssetProblem::Missing
                } else {
                    ModelAssetProblem::MetadataUnavailable {
                        kind: error.kind(),
                        message: error.to_string(),
                    }
                };
                return Some(self.diagnostic(required_file, path, problem));
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
        let actual = match sha256_file(&path) {
            Ok(actual) => actual,
            Err(error) => {
                return Some(self.diagnostic(
                    required_file,
                    path,
                    ModelAssetProblem::Unreadable {
                        kind: error.kind(),
                        message: error.to_string(),
                    },
                ));
            }
        };
        if !actual.eq_ignore_ascii_case(required_file.sha256) {
            return Some(self.diagnostic(
                required_file,
                path,
                ModelAssetProblem::HashMismatch {
                    expected: required_file.sha256.to_owned(),
                    actual,
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
    HashMismatch {
        expected: String,
        actual: String,
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
            Self::HashMismatch { expected, actual } => {
                write!(formatter, "SHA-256 is {actual}; expected {expected}")
            }
        }
    }
}

pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    // Windows' default thread stack is commonly 1 MiB, so this transfer
    // buffer must live on the heap rather than making `verify` overflow before
    // it can hash the first model byte.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
