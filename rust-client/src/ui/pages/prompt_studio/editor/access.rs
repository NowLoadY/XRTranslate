use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

/// Deliberately opt-in on installed builds too. This never creates the marker.
pub(crate) fn editor_enabled(beta: bool, project_root: &Path) -> bool {
    if !beta {
        return false;
    }
    let Ok(file) = File::open(project_root.join("runtime/debug.md")) else {
        return false;
    };
    let mut first_line = Vec::new();
    BufReader::new(file)
        .take(8)
        .read_until(b'\n', &mut first_line)
        .is_ok()
        && matches!(first_line.as_slice(), b"hello" | b"hello\n" | b"hello\r\n")
}
