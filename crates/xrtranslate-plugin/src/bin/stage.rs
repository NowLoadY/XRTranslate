//! Optional build helper. Manual copies use exactly the same directory layout.
use std::{fs, io, path::Path};

fn copy(source: &Path, destination: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(source)?.file_type();
    if kind.is_file() {
        fs::copy(source, destination)?;
    } else if kind.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        return Err(io::Error::other(
            "Plugin resources cannot be symbolic links",
        ));
    }
    Ok(())
}
fn stage(
    source: &Path,
    binary: &Path,
    destination: &Path,
    replace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if destination.exists() && !replace {
        return Err("Destination exists; use --replace to update it".into());
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".plugin-")
        .tempdir_in(parent)?;
    let root = temporary.path().join("new");
    fs::create_dir(&root)?;
    let mut manifest: xrtranslate_plugin::Manifest =
        serde_json::from_slice(&fs::read(source.join("plugin.json"))?)?;
    manifest.executable = binary
        .file_name()
        .ok_or("Binary has no filename")?
        .to_string_lossy()
        .into_owned();
    fs::write(
        root.join("plugin.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    copy(binary, &root.join(&manifest.executable))?;
    for name in ["LICENSE", "NOTICE", "licenses", "assets"] {
        if source.join(name).exists() {
            copy(&source.join(name), &root.join(name))?;
        }
    }
    xrtranslate_plugin::directory::read(&root).map_err(io::Error::other)?;
    let previous = temporary.path().join("previous");
    if destination.exists() {
        let installed =
            xrtranslate_plugin::directory::read(destination).map_err(io::Error::other)?;
        if installed.id != manifest.id || fs::symlink_metadata(destination)?.is_symlink() {
            return Err("Destination must be a directory for the same plugin".into());
        }
        fs::rename(destination, &previous)?;
    }
    if let Err(error) = fs::rename(&root, destination) {
        if previous.exists() {
            // Keep the old install recoverable even if the destination becomes unavailable.
            let recovery = temporary.keep();
            fs::rename(recovery.join("previous"), destination)?;
            fs::remove_dir_all(recovery)?;
        }
        return Err(error.into());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let replace = args.len() == 5 && args[4] == "--replace";
    if !(args.len() == 4 || replace) || args[0] != "stage" {
        return Err(
            "Usage: xrt-plugin stage <source-directory> <executable> <destination-directory> [--replace]"
                .into(),
        );
    }
    stage(
        Path::new(&args[1]),
        Path::new(&args[2]),
        Path::new(&args[3]),
        replace,
    )
}
