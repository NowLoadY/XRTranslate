//! A plugin is present exactly while its manifest and executable are in the directory.
use crate::Manifest;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn read(directory: &Path, executables: Option<&Path>) -> Result<Manifest, String> {
    let path = directory.join("plugin.json");
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("Plugin manifest is too large".into());
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    manifest.validate()?;
    executable(directory, &manifest, executables)?;
    Ok(manifest)
}

/// Resources may live separately from OS-installed executable code (e.g. Android).
pub fn executable(
    directory: &Path,
    manifest: &Manifest,
    executables: Option<&Path>,
) -> Result<PathBuf, String> {
    let root = executables
        .unwrap_or(directory)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let path = root
        .join(&manifest.executable)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("Plugin executable is outside its installation directory".into());
    }
    Ok(path)
}

pub fn discover(
    root: &Path,
    executables: Option<&Path>,
) -> Result<Vec<(PathBuf, Result<Manifest, String>)>, String> {
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
            let manifest = read(&path, executables);
            (path, manifest)
        })
        .collect())
}

/// Discover providers from the user and application plugin directories.
pub fn discover_preferred(
    user: &Path,
    bundled: Option<&Path>,
    executables: Option<&Path>,
) -> Result<Vec<(PathBuf, Result<Manifest, String>)>, String> {
    let mut selected = std::collections::BTreeMap::new();
    let mut issues = Vec::new();
    for (root, binaries, user_package) in [(Some(user), executables, true), (bundled, None, false)]
    {
        let Some(root) = root else { continue };
        for (path, entry) in discover(root, binaries)? {
            let manifest = match entry {
                Ok(manifest) => manifest,
                Err(error) => {
                    issues.push((path, Err(error)));
                    continue;
                }
            };
            let rank = (manifest.pro_for.is_some(), user_package);
            let key = manifest.provider_id().to_owned();
            if let Some((current, _, _)) = selected.get(&key) {
                if *current == rank {
                    issues.push((path, Err(format!("Duplicate plugin provider for {key}"))));
                    continue;
                }
                if *current > rank {
                    continue;
                }
            }
            selected.insert(key, (rank, path, manifest));
        }
    }
    issues.extend(
        selected
            .into_values()
            .map(|(_, path, manifest)| (path, Ok(manifest))),
    );
    Ok(issues)
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
    fn pro_owns_the_family_until_its_package_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let user = temp.path().join("user");
        let bundled = temp.path().join("bundled");
        let install = |root: &Path, id: &str, pro_for: Option<&str>, minimum: &str| {
            let path = root.join(id);
            fs::create_dir_all(&path).unwrap();
            let provider = pro_for.unwrap_or(id);
            let manifest = serde_json::json!({
                "id":id, "pro_for":pro_for, "min_host_version":minimum,
                "title":{"en":id}, "executable":"program", "tools":[{
                    "name":format!("{provider}.read"), "description":"Read",
                    "inputSchema":{"type":"object","properties":{}}
                }]
            });
            fs::write(path.join("plugin.json"), manifest.to_string()).unwrap();
            fs::write(path.join("program"), []).unwrap();
        };
        let chosen = || {
            discover_preferred(&user, Some(&bundled), None)
                .unwrap()
                .into_iter()
                .map(|(path, manifest)| (path, manifest.unwrap().id))
                .collect::<Vec<_>>()
        };
        install(&bundled, "sample", None, "0.4.0");
        install(&user, "sample", None, "0.4.0");
        install(&user, "other", None, "0.4.0");
        assert!(chosen().contains(&(user.join("sample"), "sample".into())));
        install(&bundled, "sample_plus", Some("sample"), "0.4.8");
        let pro = chosen();
        assert_eq!(pro.len(), 2);
        assert!(pro.contains(&(bundled.join("sample_plus"), "sample_plus".into())));
        // A present Pro that needs a newer host still reserves its provider.
        install(&user, "sample_plus", Some("sample"), "9.0.0");
        assert!(chosen().contains(&(user.join("sample_plus"), "sample_plus".into())));
        fs::remove_dir_all(user.join("sample_plus")).unwrap();
        fs::remove_dir_all(bundled.join("sample_plus")).unwrap();
        assert!(chosen().contains(&(user.join("sample"), "sample".into())));
    }

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

    #[test]
    fn plugin_release_version_is_optional_metadata() {
        let mut manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id":"example", "min_host_version":"0.4.0",
            "title":{"en":"Example"}, "executable":"example.exe"
        }))
        .unwrap();
        manifest.validate().unwrap();
        assert!(
            serde_json::to_value(&manifest)
                .unwrap()
                .get("version")
                .is_none()
        );
        for version in ["", "0.1.0", "9.0.0"] {
            manifest.version = version.into();
            assert!(require_host(&manifest, "0.4.3").is_ok());
            assert!(require_host(&manifest, "0.3.9").is_err());
        }
    }
}
