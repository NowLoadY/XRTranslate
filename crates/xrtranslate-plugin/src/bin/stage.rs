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
) -> Result<(), Box<dyn std::error::Error>> {
    if destination.exists() {
        return Err(
            "Destination exists; disable and remove the old plugin directory before replacing it"
                .into(),
        );
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".plugin-")
        .tempdir_in(parent)?;
    let root = temporary.path();
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
    xrtranslate_plugin::directory::read(root).map_err(io::Error::other)?;
    fs::rename(root, destination)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 || args[0] != "stage" {
        return Err(
            "Usage: xrt-plugin stage <source-directory> <executable> <destination-directory>"
                .into(),
        );
    }
    stage(
        Path::new(&args[1]),
        Path::new(&args[2]),
        Path::new(&args[3]),
    )
}
