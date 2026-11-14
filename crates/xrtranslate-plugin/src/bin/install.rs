//! Publish a prepared package without moving directories that Explorer may have open.
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

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
                replace_contents(&new, &old, &saved, moves)?;
                continue;
            }
            if new_kind.is_file() && old_kind.is_file() && same_file(&new, &old)? {
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

pub(super) fn publish(temporary: tempfile::TempDir, destination: &Path) -> io::Result<()> {
    let new = temporary.path().join("new");
    if !destination.exists() {
        return move_path(&new, destination);
    }
    let mut moves = Vec::new();
    if let Err(error) = replace_contents(
        &new,
        destination,
        &temporary.path().join("previous"),
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
