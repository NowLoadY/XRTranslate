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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_requires_beta_and_an_exact_first_line() {
        let root = std::env::temp_dir().join(format!("prompt-access-{}", uuid::Uuid::new_v4()));
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        assert!(!editor_enabled(true, &root));
        for (text, valid) in [
            ("hello", true),
            ("hello\nnotes", true),
            ("hello\r\nnotes", true),
            ("Hello", false),
            ("hello ", false),
            (" hello", false),
            ("\nhello", false),
            ("hello world", false),
            ("", false),
        ] {
            std::fs::write(runtime.join("debug.md"), text).unwrap();
            assert_eq!(editor_enabled(true, &root), valid, "{text:?}");
            assert!(!editor_enabled(false, &root));
        }
        std::fs::remove_file(runtime.join("debug.md")).unwrap();
        assert!(!editor_enabled(true, &root));
        std::fs::remove_dir(runtime).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
