use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

const MARKER_PATH: &str = "runtime/debug.md";

/// Both the beta channel and an explicit local marker are required.
pub(crate) fn editor_enabled(beta: bool, project_root: &Path) -> bool {
    if !beta {
        return false;
    }
    let Ok(file) = File::open(project_root.join(MARKER_PATH)) else {
        return false;
    };
    let mut first_line = Vec::new();
    BufReader::new(file)
        .take(8)
        .read_until(b'\n', &mut first_line)
        .is_ok()
        && matches!(first_line.as_slice(), b"hello" | b"hello\n" | b"hello\r\n")
}

#[cfg(target_os = "android")]
pub(crate) fn unlock_editor(project_root: &Path) -> std::io::Result<()> {
    if editor_enabled(true, project_root) {
        return Ok(());
    }
    let path = project_root.join(MARKER_PATH);
    let previous = match std::fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    // Keep any existing notes while adding the first line accepted by the gate.
    let mut contents = b"hello\n".to_vec();
    contents.extend_from_slice(&previous);
    std::fs::create_dir_all(project_root.join("runtime"))?;
    std::fs::write(path, contents)
}
