//! A plugin is present exactly while its manifest and executable are in the directory.
use crate::Manifest;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn read(directory: &Path) -> Result<Manifest, String> {
    let path = directory.join("plugin.json");
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("Plugin manifest is too large".into());
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    manifest.validate()?;
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let executable = root
        .join(&manifest.executable)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !executable.starts_with(&root) || !executable.is_file() {
        return Err("Plugin executable is outside its directory".into());
    }
    Ok(manifest)
}

pub fn discover(root: &Path) -> Result<Vec<(PathBuf, Result<Manifest, String>)>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut directories = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| entry.path())
        .filter(|path| path.join("plugin.json").is_file())
        .collect::<Vec<_>>();
    directories.sort();
    Ok(directories
        .into_iter()
        .map(|path| {
            let manifest = read(&path);
            (path, manifest)
        })
        .collect())
}

pub fn require_host(manifest: &Manifest, version: &str) -> Result<(), String> {
    let required = semver::Version::parse(&manifest.min_host_version).map_err(|e| e.to_string())?;
    let current = semver::Version::parse(version).map_err(|e| e.to_string())?;
    if current < required {
        Err(format!(
            "Requires XRTranslate {required} or later (installed: {current})"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_plugins_remain_compatible_with_host_updates() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id":"example", "version":"0.1.0", "min_host_version":"0.4.0",
            "title":{"en":"Example"}, "executable":"example.exe"
        }))
        .unwrap();
        for host in ["0.4.0", "0.4.1", "0.5.0"] {
            assert!(require_host(&manifest, host).is_ok());
        }
        assert!(require_host(&manifest, "0.3.9").is_err());
    }
}
