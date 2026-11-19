//! A plugin is present exactly while its manifest and executable are in the directory.
use crate::Manifest;
use std::{
    collections::BTreeMap,
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

#[derive(Clone)]
pub struct Package {
    pub directory: PathBuf,
    pub manifest: Manifest,
    pub modified: Option<std::time::SystemTime>,
}

#[derive(Default)]
pub struct Catalog {
    pub providers: BTreeMap<String, Vec<Package>>,
    pub issues: Vec<(PathBuf, String)>,
}

impl Catalog {
    /// Use the saved edition when installed, otherwise the standard package or first available edition.
    pub fn selected(&self, provider: &str, preferred: Option<&str>) -> Option<&Package> {
        let packages = self.providers.get(provider)?;
        packages
            .iter()
            .find(|p| Some(p.manifest.id.as_str()) == preferred)
            .or_else(|| packages.first())
    }
}

pub fn discover_packages(
    user: &Path,
    bundled: Option<&Path>,
    executables: Option<&Path>,
) -> Result<Catalog, String> {
    let mut catalog = Catalog::default();
    let mut seen = BTreeMap::new();
    for (root, binaries, is_user) in [(Some(user), executables, true), (bundled, None, false)] {
        let Some(root) = root else { continue };
        for (directory, entry) in discover(root, binaries)? {
            let manifest = match entry {
                Ok(manifest) => manifest,
                Err(error) => {
                    catalog.issues.push((directory, error));
                    continue;
                }
            };
            if let Some(previous) = seen.get(&manifest.id) {
                if *previous == is_user {
                    catalog.issues.push((
                        directory,
                        format!("Duplicate plugin package {}", manifest.id),
                    ));
                }
                continue;
            }
            seen.insert(manifest.id.clone(), is_user);
            let modified = fs::metadata(binaries.unwrap_or(&directory).join(&manifest.executable))
                .and_then(|m| m.modified())
                .ok();
            catalog
                .providers
                .entry(manifest.provider_id().to_owned())
                .or_default()
                .push(Package {
                    directory,
                    manifest,
                    modified,
                });
        }
    }
    for packages in catalog.providers.values_mut() {
        packages.sort_by(|a, b| {
            (a.manifest.pro_for.is_some(), &a.manifest.id)
                .cmp(&(b.manifest.pro_for.is_some(), &b.manifest.id))
        });
    }
    Ok(catalog)
}

/// Discover the default package for each provider when no edition preference is supplied.
pub fn discover_preferred(
    user: &Path,
    bundled: Option<&Path>,
    executables: Option<&Path>,
) -> Result<Vec<(PathBuf, Result<Manifest, String>)>, String> {
    let catalog = discover_packages(user, bundled, executables)?;
    Ok(catalog
        .issues
        .into_iter()
        .map(|(path, error)| (path, Err(error)))
        .chain(
            catalog
                .providers
                .into_values()
                .filter_map(|packages| packages.into_iter().next())
                .map(|p| (p.directory, Ok(p.manifest))),
        )
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
    fn edition_selection_respects_user_choice_and_deduplicates_package_copies() {
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
        let scan = || discover_packages(&user, Some(&bundled), None).unwrap();
        install(&bundled, "sample", None, "0.4.0");
        install(&user, "sample", None, "0.4.0");
        install(&user, "other", None, "0.4.0");
        install(&bundled, "sample_plus", Some("sample"), "0.4.8");
        install(&user, "sample_plus", Some("sample"), "0.4.8");
        let both = scan();
        assert!(both.issues.is_empty());
        assert_eq!(both.providers.len(), 2);
        assert_eq!(both.providers["sample"].len(), 2);
        assert_eq!(
            both.selected("sample", None).unwrap().directory,
            user.join("sample")
        );
        assert_eq!(
            both.selected("sample", Some("sample_plus"))
                .unwrap()
                .directory,
            user.join("sample_plus")
        );
        assert_eq!(
            both.selected("sample", Some("other")).unwrap().manifest.id,
            "sample"
        );
        fs::remove_dir_all(user.join("sample_plus")).unwrap();
        assert_eq!(
            scan()
                .selected("sample", Some("sample_plus"))
                .unwrap()
                .directory,
            bundled.join("sample_plus")
        );
        fs::remove_dir_all(bundled.join("sample_plus")).unwrap();
        assert_eq!(
            scan()
                .selected("sample", Some("sample_plus"))
                .unwrap()
                .manifest
                .id,
            "sample"
        );
        fs::remove_dir_all(user.join("sample")).unwrap();
        fs::remove_dir_all(bundled.join("sample")).unwrap();
        install(&user, "sample_plus", Some("sample"), "0.4.8");
        assert_eq!(
            scan().selected("sample", None).unwrap().manifest.id,
            "sample_plus"
        );
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
