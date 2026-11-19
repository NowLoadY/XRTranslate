//! Publish a prepared package without moving directories that Explorer may have open.
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(source)?.file_type();
    if kind.is_file() {
        fs::copy(source, destination)?;
    } else if kind.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        return Err(io::Error::other(
            "Plugin resources cannot be symbolic links",
        ));
    }
    Ok(())
}
pub struct Prepared {
    pub manifest: crate::Manifest,
    temporary: tempfile::TempDir,
    destination: PathBuf,
}

impl Prepared {
    pub fn install(self) -> io::Result<crate::Manifest> {
        if self.destination.exists() {
            let old = crate::directory::read(&self.destination, None).map_err(io::Error::other)?;
            if old.id != self.manifest.id || fs::symlink_metadata(&self.destination)?.is_symlink() {
                return Err(io::Error::other(
                    "Destination belongs to a different package",
                ));
            }
        }
        publish(self.temporary, &self.destination)?;
        Ok(self.manifest)
    }
}

fn unpack(archive: &Path, output: &Path) -> io::Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    fs::create_dir(output)?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let relative = file
            .enclosed_name()
            .ok_or_else(|| io::Error::other("Invalid path in plugin ZIP"))?;
        // Reject links and special files. Only ordinary package files are installed.
        if file.is_symlink()
            || !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other("Invalid entry in plugin ZIP"));
        }
        #[cfg(windows)]
        if relative.to_string_lossy().contains(':') {
            return Err(io::Error::other("Invalid Windows path in plugin ZIP"));
        }
        let path = output.join(relative);
        if file.is_dir() {
            fs::create_dir_all(path)?;
        } else {
            fs::create_dir_all(path.parent().unwrap())?;
            io::copy(&mut file, &mut fs::File::create_new(&path)?)?;
            #[cfg(unix)]
            if let Some(mode) = file.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
    }
    Ok(())
}

fn package_directory(root: &Path) -> io::Result<PathBuf> {
    if root.join("plugin.json").is_file() {
        return Ok(root.to_owned());
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("plugin.json").is_file() {
            candidates.push(entry.path());
        }
    }
    if candidates.len() != 1 {
        return Err(io::Error::other(
            "Choose a package containing one plugin.json and its executable",
        ));
    }
    Ok(candidates.pop().unwrap())
}

/// Prepare a built plugin in the chosen installation root before changing a running package.
pub fn prepare(source: &Path, root: &Path, version: &str) -> io::Result<Prepared> {
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let source = source.canonicalize()?;
    let temporary = tempfile::Builder::new()
        .prefix(".plugin-")
        .tempdir_in(&root)?;
    let staged = temporary.path().join("new");
    if source
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
        && source.is_file()
    {
        let unpacked = temporary.path().join("archive");
        unpack(&source, &unpacked)?;
        fs::rename(package_directory(&unpacked)?, &staged)?;
    } else {
        let source = if source.is_dir() {
            source.as_path()
        } else if source
            .file_name()
            .is_some_and(|s| s.eq_ignore_ascii_case("plugin.json"))
        {
            source.parent().unwrap()
        } else {
            return Err(io::Error::other("Select a plugin ZIP or plugin.json"));
        };
        let package = package_directory(source)?;
        if root.starts_with(&package) {
            return Err(io::Error::other("Choose a single built plugin directory"));
        }
        crate::directory::read(&package, None).map_err(io::Error::other)?;
        copy_tree(&package, &staged)?;
    }
    let manifest = crate::directory::read(&staged, None).map_err(io::Error::other)?;
    crate::directory::require_host(&manifest, version).map_err(io::Error::other)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = staged.join(&manifest.executable);
        let mode = fs::metadata(&executable)?.permissions().mode();
        fs::set_permissions(executable, fs::Permissions::from_mode(mode | 0o100))?;
    }
    let destination = root.join(&manifest.id);
    Ok(Prepared {
        manifest,
        temporary,
        destination,
    })
}

fn move_path(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "Cannot move {} to {}: {error}",
                source.display(),
                destination.display()
            ),
        )
    })
}

fn kind(path: &Path) -> io::Result<Option<fs::FileType>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata.file_type())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn same_file(source: &Path, destination: &Path) -> io::Result<bool> {
    let (source_metadata, destination_metadata) =
        (fs::metadata(source)?, fs::metadata(destination)?);
    if source_metadata.len() != destination_metadata.len()
        || source_metadata.permissions() != destination_metadata.permissions()
    {
        return Ok(false);
    }
    let mut source = fs::File::open(source)?;
    let mut destination = fs::File::open(destination)?;
    let (mut left, mut right) = ([0; 32768], [0; 32768]);
    loop {
        let a = source.read(&mut left)?;
        let b = destination.read(&mut right)?;
        if a != b || left[..a] != right[..b] {
            return Ok(false);
        }
        if a == 0 {
            return Ok(true);
        }
    }
}

fn replace_contents(
    source: &Path,
    destination: &Path,
    backup: &Path,
    package_manifest: &Path,
    moves: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    let mut names = BTreeSet::new();
    for directory in [source, destination] {
        for entry in fs::read_dir(directory)? {
            names.insert(entry?.file_name());
        }
    }
    let mut names: Vec<_> = names.into_iter().collect();
    // Publish the manifest after the executable and resources are in place.
    names.sort_by_key(|name| name == "plugin.json");
    for name in names {
        let new = source.join(&name);
        let old = destination.join(&name);
        let saved = backup.join(&name);
        let (new_kind, old_kind) = (kind(&new)?, kind(&old)?);
        if let (Some(new_kind), Some(old_kind)) = (new_kind, old_kind) {
            if new_kind.is_dir() && old_kind.is_dir() {
                replace_contents(&new, &old, &saved, package_manifest, moves)?;
                continue;
            }
            // Re-publish the root manifest last on an asset-only update. This
            // changes the package directory timestamp observed by a running host.
            let publish_manifest = old == package_manifest && !moves.is_empty();
            if new_kind.is_file()
                && old_kind.is_file()
                && !publish_manifest
                && same_file(&new, &old)?
            {
                continue;
            }
        }
        if old_kind.is_some() {
            fs::create_dir_all(backup)?;
            move_path(&old, &saved)?;
            moves.push((old.clone(), saved));
        }
        if new_kind.is_some() {
            move_path(&new, &old)?;
            moves.push((new, old));
        }
    }
    Ok(())
}

pub fn publish(temporary: tempfile::TempDir, destination: &Path) -> io::Result<()> {
    let new = temporary.path().join("new");
    if !destination.exists() {
        return move_path(&new, destination);
    }
    let mut moves = Vec::new();
    if let Err(error) = replace_contents(
        &new,
        destination,
        &temporary.path().join("previous"),
        &destination.join("plugin.json"),
        &mut moves,
    ) {
        for (source, target) in moves.iter().rev() {
            if let Err(rollback) = move_path(target, source) {
                let recovery = temporary.keep();
                return Err(io::Error::new(
                    rollback.kind(),
                    format!(
                        "{error}; rollback failed: {rollback}. Recovery files kept at {}",
                        recovery.display()
                    ),
                ));
            }
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(root: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(root).unwrap();
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    #[test]
    fn import_zip_and_manifest_use_the_package_id_and_keep_the_previous_install_on_error() {
        use std::io::Write;
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("plugins");
        let zip_path = scratch.path().join("sample.zip");
        let manifest = serde_json::json!({"id":"example_plus","pro_for":"example",
            "min_host_version":"0.4.8","title":{"en":"Example"},"executable":"program"});
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (path, bytes) in [
            ("package/plugin.json", manifest.to_string()),
            ("package/program", "first".into()),
        ] {
            zip.start_file(path, options).unwrap();
            zip.write_all(bytes.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let prepared = prepare(&zip_path, &root, "0.4.8").unwrap();
        assert!(!root.join("example_plus").exists());
        prepared.install().unwrap();
        let imported = root.join("example_plus");
        assert_eq!(fs::read(imported.join("program")).unwrap(), b"first");
        let folder = scratch.path().join("update");
        package(
            &folder,
            &[
                ("plugin.json", &manifest.to_string()),
                ("program", "second"),
            ],
        );
        assert!(prepare(&folder, &root, "0.4.7").is_err());
        assert_eq!(fs::read(imported.join("program")).unwrap(), b"first");
        prepare(&folder.join("plugin.json"), &root, "0.4.8")
            .unwrap()
            .install()
            .unwrap();
        assert_eq!(fs::read(imported.join("program")).unwrap(), b"second");
        assert!(!root.join("example").exists());
    }

    #[test]
    fn nested_resource_updates_publish_once_even_with_the_same_executable_and_manifest() {
        use std::time::{Duration, UNIX_EPOCH};
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("sample");
        let manifest = installed.join("plugin.json");
        let stage = |text, day: u64| {
            let temporary = tempfile::tempdir_in(root.path()).unwrap();
            let new = temporary.path().join("new");
            package(
                &new,
                &[
                    ("plugin.json", "{}"),
                    ("program", "binary"),
                    ("assets/library/title", text),
                ],
            );
            let stamp = UNIX_EPOCH + Duration::from_secs(946684800 + day * 86400);
            fs::File::options()
                .write(true)
                .open(new.join("plugin.json"))
                .unwrap()
                .set_modified(stamp)
                .unwrap();
            publish(temporary, &installed).unwrap();
            stamp
        };
        stage("first", 0);
        let updated = stage("second", 1);
        assert_eq!(
            fs::metadata(&manifest).unwrap().modified().unwrap(),
            updated
        );
        let directory_stamp = fs::metadata(&installed).unwrap().modified().unwrap();
        stage("second", 2);
        assert_eq!(
            fs::metadata(&manifest).unwrap().modified().unwrap(),
            updated
        );
        assert_eq!(
            fs::metadata(&installed).unwrap().modified().unwrap(),
            directory_stamp
        );
    }

    #[test]
    fn installs_and_updates_changed_files_and_removes_obsolete_resources() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("installed");
        let initial = tempfile::tempdir_in(root.path()).unwrap();
        package(
            &initial.path().join("new"),
            &[
                ("plugin.json", "old manifest"),
                ("plugin.exe", "old code"),
                ("assets/unchanged", "same"),
                ("assets/obsolete", "old resource"),
            ],
        );
        publish(initial, &destination).unwrap();
        let update = tempfile::tempdir_in(root.path()).unwrap();
        package(
            &update.path().join("new"),
            &[
                ("plugin.json", "new manifest"),
                ("plugin.exe", "new code"),
                ("assets/unchanged", "same"),
                ("assets/added", "new resource"),
            ],
        );
        publish(update, &destination).unwrap();
        for (path, expected) in [
            ("plugin.json", "new manifest"),
            ("plugin.exe", "new code"),
            ("assets/unchanged", "same"),
            ("assets/added", "new resource"),
        ] {
            assert_eq!(
                fs::read_to_string(destination.join(path)).unwrap(),
                expected
            );
        }
        assert!(!destination.join("assets/obsolete").exists());
    }

    #[cfg(windows)]
    #[test]
    fn open_resource_directories_and_unchanged_files_do_not_block_updates() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("installed");
        package(
            &destination,
            &[("plugin.exe", "old"), ("assets/audio/clip", "same")],
        );
        let _directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .open(destination.join("assets/audio"))
            .unwrap();
        let _clip = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(destination.join("assets/audio/clip"))
            .unwrap();
        // This is the former whole-directory replacement, with Explorer-like readers.
        assert!(fs::rename(&destination, root.path().join("previous")).is_err());
        let update = tempfile::tempdir_in(root.path()).unwrap();
        package(
            &update.path().join("new"),
            &[("plugin.exe", "new"), ("assets/audio/clip", "same")],
        );
        publish(update, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("plugin.exe")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(destination.join("assets/audio/clip")).unwrap(),
            "same"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locked_changed_file_rolls_back_earlier_changes() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("installed");
        package(
            &destination,
            &[("a", "old a"), ("z", "old z"), ("plugin.json", "old")],
        );
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(destination.join("z"))
            .unwrap();
        let update = tempfile::tempdir_in(root.path()).unwrap();
        package(
            &update.path().join("new"),
            &[("a", "new a"), ("z", "new z"), ("plugin.json", "new")],
        );
        let error = publish(update, &destination).unwrap_err().to_string();
        assert!(
            error.contains(destination.join("z").to_str().unwrap()),
            "{error}"
        );
        for (path, expected) in [("a", "old a"), ("z", "old z"), ("plugin.json", "old")] {
            assert_eq!(
                fs::read_to_string(destination.join(path)).unwrap(),
                expected
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn failed_new_file_move_restores_the_file_already_backed_up() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("installed");
        package(&destination, &[("a", "old a"), ("z", "old z")]);
        let update = tempfile::tempdir_in(root.path()).unwrap();
        package(
            &update.path().join("new"),
            &[("a", "new a"), ("z", "new z")],
        );
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(update.path().join("new/z"))
            .unwrap();
        assert!(publish(update, &destination).is_err());
        assert_eq!(fs::read_to_string(destination.join("a")).unwrap(), "old a");
        assert_eq!(fs::read_to_string(destination.join("z")).unwrap(), "old z");
    }
}
