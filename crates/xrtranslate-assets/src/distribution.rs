//! Distribution ownership and writable storage, shared by desktop services.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const DISTRIBUTION_FILE: &str = "distribution.json";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    #[default]
    Portable,
    Installer,
    Store,
    Steam,
}

impl Distribution {
    pub fn read(root: &Path) -> io::Result<Self> {
        match fs::read(root.join(DISTRIBUTION_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::Portable),
            Err(error) => Err(error),
        }
    }
}

/// Installed channels use per-user storage; portable/source trees stay self-contained.
pub fn data_root(root: &Path) -> PathBuf {
    if cfg!(windows)
        && matches!(
            Distribution::read(root),
            Ok(Distribution::Installer | Distribution::Store | Distribution::Steam)
        )
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .expect("Windows user profile must provide LOCALAPPDATA")
            .join("XRTranslate");
    }
    root.to_path_buf()
}
