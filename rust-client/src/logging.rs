//! Bounded persistent logs shared by the desktop client and its child services.
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};
#[cfg(not(target_os = "android"))]
use xrtranslate_config::AppConfig;
use xrtranslate_config::{RuntimeLayout, StorageConfig};

const MIN_LOG_FILE_BYTES: u64 = 64 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_FILES: usize = 8;

#[derive(Clone)]
pub(crate) struct LogPolicy {
    pub(crate) directory: PathBuf,
    file_name: String,
    max_file_bytes: u64,
    retained_files: usize,
}

impl LogPolicy {
    pub(crate) fn new(
        project_root: &std::path::Path,
        storage: StorageConfig,
        file_name: &str,
    ) -> Self {
        let directory = if storage.log_dir.as_os_str().is_empty() {
            project_root.join("runtime").join("logs")
        } else {
            RuntimeLayout::for_project_root(project_root).resolve_configured_path(storage.log_dir)
        };
        Self {
            directory,
            file_name: file_name.to_owned(),
            max_file_bytes: storage
                .log_max_bytes
                .clamp(MIN_LOG_FILE_BYTES, MAX_LOG_FILE_BYTES),
            retained_files: storage.log_retained_files.clamp(1, MAX_LOG_FILES),
        }
    }

    pub(crate) fn active_path(&self) -> PathBuf {
        self.directory.join(&self.file_name)
    }

    fn archive_path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("{}.{}", self.file_name, index))
    }

    fn rotate(&self) {
        let archive_count = self.retained_files.saturating_sub(1);
        if archive_count == 0 {
            let _ = fs::remove_file(self.active_path());
            return;
        }
        let _ = fs::remove_file(self.archive_path(archive_count));
        for index in (1..archive_count).rev() {
            let _ = fs::rename(self.archive_path(index), self.archive_path(index + 1));
        }
        let _ = fs::rename(self.active_path(), self.archive_path(1));
    }

    pub(crate) fn read_current(&self, maximum_bytes: usize) -> String {
        let mut bytes = Vec::new();
        append_file_tail(&mut bytes, &self.active_path(), maximum_bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

pub(crate) struct RotatingLog {
    policy: LogPolicy,
    file: Option<fs::File>,
    written: u64,
}

impl RotatingLog {
    pub(crate) fn open(policy: LogPolicy) -> io::Result<Self> {
        fs::create_dir_all(&policy.directory)?;
        policy.rotate();
        let file = Some(fs::File::create(policy.active_path())?);
        Ok(Self {
            policy,
            file,
            written: 0,
        })
    }
}

impl Write for RotatingLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let keep = bytes.len().min(self.policy.max_file_bytes as usize);
        let chunk = &bytes[bytes.len() - keep..];
        if self.written > 0 && self.written + keep as u64 > self.policy.max_file_bytes {
            self.file.take();
            self.policy.rotate();
            self.file = Some(fs::File::create(self.policy.active_path())?);
            self.written = 0;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is closed"))?;
        file.write_all(chunk)?;
        self.written += keep as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

#[cfg(not(target_os = "android"))]
pub(crate) fn init() {
    struct DesktopLog(Option<RotatingLog>);
    impl Write for DesktopLog {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let _ = io::stderr().write_all(bytes);
            if let Some(file) = self.0.as_mut()
                && let Err(error) = file.write_all(bytes)
            {
                eprintln!("Cannot write client log: {error}");
                self.0 = None;
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            if let Some(file) = self.0.as_mut() {
                file.flush()?;
            }
            Ok(())
        }
    }
    let root = crate::backend::project_root();
    let storage = AppConfig::from_path_with_user_config(root.join("config.json"), &root)
        .map(|config| config.storage)
        .unwrap_or_default();
    let file_name = if std::env::args().any(|arg| arg == "--overlay") {
        "overlay.log"
    } else {
        "client.log"
    };
    let policy = LogPolicy::new(&root, storage, file_name);
    let path = policy.active_path();
    let output = RotatingLog::open(policy)
        .map_err(|error| {
            eprintln!("Cannot open client log {}: {error}", path.display());
        })
        .ok();
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("rust_client", log::LevelFilter::Info)
        .filter_module("xrtranslate_client_core", log::LevelFilter::Info)
        .parse_env(env_logger::Env::default())
        .target(env_logger::Target::Pipe(Box::new(DesktopLog(output))))
        .write_style(env_logger::WriteStyle::Never)
        .init();
    log::info!("XRTranslate client started; log={}", path.display());
}

fn append_file_tail(output: &mut Vec<u8>, path: &std::path::Path, maximum_bytes: usize) {
    let Ok(mut file) = fs::File::open(path) else {
        return;
    };
    let length = file.metadata().map(|value| value.len()).unwrap_or(0);
    let keep = length.min(maximum_bytes as u64);
    if file
        .seek(SeekFrom::Start(length.saturating_sub(keep)))
        .is_err()
    {
        return;
    }
    let mut bytes = Vec::with_capacity(keep as usize);
    if file.take(keep).read_to_end(&mut bytes).is_ok() {
        output.extend_from_slice(&bytes);
        if output.len() > maximum_bytes {
            output.drain(..output.len() - maximum_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_visible_immediately_and_rotates_with_bounded_retention() {
        let directory =
            std::env::temp_dir().join(format!("xrtranslate-logs-{}", uuid::Uuid::new_v4()));
        let policy = LogPolicy {
            directory: directory.clone(),
            file_name: "test.log".into(),
            max_file_bytes: 8,
            retained_files: 2,
        };
        let mut output = RotatingLog::open(policy.clone()).unwrap();
        output.write_all(b"first").unwrap();
        assert_eq!(policy.read_current(8), "first");
        output.write_all(b"second").unwrap();
        assert_eq!(fs::read_to_string(policy.archive_path(1)).unwrap(), "first");
        output.write_all(b"third").unwrap();
        assert_eq!(policy.read_current(8), "third");
        assert_eq!(
            fs::read_to_string(policy.archive_path(1)).unwrap(),
            "second"
        );
        output.write_all(b"0123456789").unwrap();
        assert_eq!(policy.read_current(8), "23456789");
        drop(output);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }
}
