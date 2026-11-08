//! One exit for user-produced files. Desktop saves with the system dialog;
//! Android hands the file to its share sheet, including Open and Save actions.

#[derive(Clone, Copy)]
pub enum Format {
    Json,
    Markdown,
    Srt,
    Vtt,
    Lrc,
    Wav,
    #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
    Mp4,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Lrc => "lrc",
            Self::Wav => "wav",
            #[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
            Self::Mp4 => "mp4",
        }
    }
}

#[cfg(all(feature = "mpv", any(windows, target_os = "linux")))]
pub fn choose_path(name: &str, format: Format) -> Option<std::path::PathBuf> {
    let extension = format.extension();
    crate::file_dialog::FileDialog::new()
        .set_file_name(file_name(name, extension))
        .add_filter(extension, &[extension])
        .save_path()
}

pub fn save(name: &str, format: Format, content: impl AsRef<[u8]>) -> Result<(), String> {
    let extension = format.extension();
    crate::file_dialog::FileDialog::new()
        .set_file_name(file_name(name, extension))
        .add_filter(extension, &[extension])
        .save(content)
}

pub fn copy_file(name: &str, format: Format, source: std::path::PathBuf) -> Result<(), String> {
    let extension = format.extension();
    crate::file_dialog::FileDialog::new()
        .set_file_name(file_name(name, extension))
        .add_filter(extension, &[extension])
        .copy_file(source)
}

/// Generate a large export on the existing dialog worker rather than the UI thread.
pub fn write_file(
    name: &str,
    format: Format,
    write: impl FnOnce(std::path::PathBuf) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let extension = format.extension();
    crate::file_dialog::FileDialog::new()
        .set_file_name(file_name(name, extension))
        .add_filter(extension, &[extension])
        .write(move |path| write(path).map_err(std::io::Error::other))
}

fn file_name(name: &str, extension: &str) -> String {
    let mut clean: String = name
        .chars()
        .take(100)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if clean.len() > 180 {
        let mut end = 180;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean.truncate(end);
    }
    let stem = clean.trim().trim_matches('.').trim();
    let stem = if stem.is_empty() { "result" } else { stem };
    // A leading underscore also avoids Windows device names such as CON.txt.
    let base = stem.split('.').next().unwrap_or(stem).to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        });
    format!("{}{stem}.{extension}", if reserved { "_" } else { "" })
}
