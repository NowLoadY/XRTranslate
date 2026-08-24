use serde_json::Value;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};
use xrtranslate_config::{AppConfig, RuntimeLayout, StorageConfig};

const MIN_LOG_FILE_BYTES: u64 = 64 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_FILES: usize = 8;
const DIAGNOSTIC_READ_BYTES: usize = 64 * 1024;
const STARTUP_ERROR_MARKER: &str = "[XRTRANSLATE_STARTUP_ERROR]";
const CORPUS_SERVER_URL: &str = "http://127.0.0.1:7766";

pub enum BackendStart {
    Ready,
    Starting(BackendStartupStage),
}

pub enum BackendStatus {
    Ready,
    Starting(BackendStartupStage),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendStartupStage {
    Corpus,
    Inference,
}

impl BackendStartupStage {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Corpus => "Starting terminology service...",
            Self::Inference => "Starting translation models...",
        }
    }
}

pub struct BackendManager {
    project_root: PathBuf,
    pub runtime_directory: String,
    pub llama_server_path: String,
    log_policy: BackendLogPolicy,
    corpus_log_policy: BackendLogPolicy,
    log_capture: Option<BoundedLogCapture>,
    corpus_log_capture: Option<BoundedLogCapture>,
    child: Option<Child>,
    corpus_child: Option<Child>,
    #[cfg(windows)]
    job: Option<KillOnCloseJob>,
}

impl BackendManager {
    pub fn load() -> Self {
        let project_root = project_root();
        let config = load_project_config(&project_root).ok();
        let configured_runtime_dir = config
            .as_ref()
            .and_then(|config| config.model_manager.runtime_directory.clone());
        let layout = RuntimeLayout::new(&project_root, configured_runtime_dir.as_deref());
        let runtime_directory = layout.runtime_root().display().to_string();
        let configured_path = config
            .as_ref()
            .map(|config| config.model_manager.llama_server_path.clone())
            .unwrap_or_default();
        let llama_server_path = preferred_llama_server_path(&layout, &configured_path);
        let log_policy = BackendLogPolicy::new(
            &project_root,
            config
                .as_ref()
                .map(|config| config.storage.clone())
                .unwrap_or_default(),
            "backend_startup.log",
        );
        let corpus_log_policy = BackendLogPolicy::new(
            &project_root,
            config
                .as_ref()
                .map(|config| config.storage.clone())
                .unwrap_or_default(),
            "corpus_startup.log",
        );
        let manager = Self {
            project_root,
            runtime_directory,
            llama_server_path,
            log_policy,
            corpus_log_policy,
            log_capture: None,
            corpus_log_capture: None,
            child: None,
            corpus_child: None,
            #[cfg(windows)]
            job: None,
        };
        if manager.llama_server_path != configured_path
            && !manager.llama_server_path.trim().is_empty()
            && let Err(error) = Self::write_llama_server_path(
                &manager.project_root,
                &config_path_value(
                    &manager
                        .runtime_layout()
                        .config_path_for(std::path::Path::new(&manager.llama_server_path)),
                ),
            )
        {
            log::warn!("Cannot persist recovered llama-server path: {error}");
        }
        manager
    }

    pub fn runtime_layout(&self) -> RuntimeLayout {
        let requested = self.runtime_directory.trim();
        let dir = if requested.is_empty() {
            None
        } else {
            Some(Path::new(requested))
        };
        RuntimeLayout::new(&self.project_root, dir)
    }

    pub fn project_root(&self) -> PathBuf {
        self.project_root.clone()
    }

    pub fn llama_server_path_is_valid(&self) -> bool {
        let value = self.llama_server_path.trim();
        !value.is_empty() && configured_llama_server_path(&self.runtime_layout(), value).is_file()
    }

    pub(crate) fn use_installed_llama_server(&mut self, path: &std::path::Path) {
        self.llama_server_path = path.display().to_string();
    }

    pub fn save_runtime_directory(&mut self) -> Result<(), String> {
        let requested = self.runtime_directory.trim();
        let value = if requested.is_empty() {
            None
        } else {
            let path = Path::new(requested);
            let canonical = if path.is_absolute() {
                if let Ok(rel) = path.strip_prefix(&self.project_root) {
                    rel.display().to_string()
                } else {
                    path.display().to_string()
                }
            } else {
                path.display().to_string()
            };
            Some(canonical)
        };
        Self::write_runtime_directory(&self.project_root, value.as_deref())?;
        let layout = self.runtime_layout();
        self.runtime_directory = layout.runtime_root().display().to_string();
        if self.llama_server_path.trim().is_empty()
            || is_managed_llama_server_path(&layout, Path::new(&self.llama_server_path))
        {
            let candidate = layout
                .managed_llama_server(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
            self.llama_server_path = candidate.display().to_string();
        }
        Ok(())
    }

    pub fn write_runtime_directory(
        project_root: &std::path::Path,
        value: Option<&str>,
    ) -> Result<(), String> {
        let config_path = project_root.join("config.json");
        let mut document =
            xrtranslate_config::load_user_config_document(&config_path, project_root)
                .map_err(|error| format!("Cannot read {}: {error}", config_path.display()))?;
        let root = document
            .as_object_mut()
            .ok_or("config.json root must be an object")?;
        let model_manager = root
            .entry("model_manager")
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or("config.json model_manager must be an object")?;
        match value {
            Some(v) if !v.trim().is_empty() => {
                model_manager.insert(
                    "runtime_directory".into(),
                    Value::String(v.trim().replace('\\', "/")),
                );
            }
            _ => {
                model_manager.remove("runtime_directory");
                model_manager.remove("runtime_root");
            }
        }
        xrtranslate_config::save_user_config_document(&config_path, project_root, &document)
    }

    /// Stores the local llama.cpp executable where the Rust backend already
    /// expects it: `model_manager.llama_server_path` in `config.json`.
    pub fn save_llama_server_path(&mut self) -> Result<(), String> {
        let requested = self.llama_server_path.trim();
        if requested.is_empty() {
            return Err("llama-server path is empty".into());
        }
        let layout = self.runtime_layout();
        let path = configured_llama_server_path(&layout, requested);
        let persisted = Self::persist_llama_server_path_with_layout(&layout, &path)?;
        self.llama_server_path = persisted.display().to_string();
        Ok(())
    }

    pub(crate) fn persist_llama_server_path(
        project_root: &std::path::Path,
        path: &std::path::Path,
    ) -> Result<PathBuf, String> {
        let layout = RuntimeLayout::for_project_root(project_root);
        Self::persist_llama_server_path_with_layout(&layout, path)
    }

    pub(crate) fn persist_llama_server_path_with_layout(
        layout: &RuntimeLayout,
        path: &std::path::Path,
    ) -> Result<PathBuf, String> {
        let path = absolute_from_project_root(layout.project_root(), path.into());
        if !path.is_file() {
            if is_managed_llama_server_path(layout, &path) {
                return Err(format!(
                    "llama.cpp runtime is not installed. Open the Welcome Page and download the recommended runtime first. Expected executable: {}",
                    path.display()
                ));
            }
            return Err(format!(
                "llama-server executable does not exist: {}",
                path.display()
            ));
        }
        let value = config_path_value(&layout.config_path_for(&path));
        Self::write_llama_server_path(layout.project_root(), &value)?;
        Ok(path)
    }

    fn write_llama_server_path(project_root: &std::path::Path, value: &str) -> Result<(), String> {
        let config_path = project_root.join("config.json");
        let mut document =
            xrtranslate_config::load_user_config_document(&config_path, project_root)
                .map_err(|error| format!("Cannot read {}: {error}", config_path.display()))?;
        let root = document
            .as_object_mut()
            .ok_or("config.json root must be an object")?;
        let model_manager = root
            .entry("model_manager")
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or("config.json model_manager must be an object")?;
        model_manager.insert(
            "llama_server_path".into(),
            Value::String(value.trim().into()),
        );
        xrtranslate_config::save_user_config_document(&config_path, project_root, &document)
    }

    pub fn prepare(&mut self, server_url: &str) -> Result<BackendStart, String> {
        if server_reachable(server_url) {
            return Ok(BackendStart::Ready);
        }
        if !is_local_server(server_url) {
            return Err(format!(
                "Backend at {server_url} is unavailable. Automatic startup is only available for localhost."
            ));
        }
        if !server_reachable(CORPUS_SERVER_URL) {
            if self.corpus_child.is_none() {
                self.start_corpus()?;
            }
            return Ok(BackendStart::Starting(BackendStartupStage::Corpus));
        }
        if self.child.is_some() {
            return Ok(BackendStart::Starting(BackendStartupStage::Inference));
        }
        self.start_backend()?;
        Ok(BackendStart::Starting(BackendStartupStage::Inference))
    }

    pub fn status(&mut self, server_url: &str) -> BackendStatus {
        if server_reachable(server_url) {
            return BackendStatus::Ready;
        }
        if !server_reachable(CORPUS_SERVER_URL) {
            if let Some(child) = &mut self.corpus_child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        self.corpus_child = None;
                        self.finish_corpus_log_capture();
                        return BackendStatus::Failed(format!(
                            "XR Corpus exited before it became ready ({status})\n\nLog Traceback:\n{}",
                            self.corpus_log_policy
                                .read_current(DIAGNOSTIC_READ_BYTES)
                                .trim()
                        ));
                    }
                    Ok(None) => {
                        return BackendStatus::Starting(BackendStartupStage::Corpus);
                    }
                    Err(error) => {
                        return BackendStatus::Failed(format!(
                            "Cannot inspect XR Corpus process: {error}"
                        ));
                    }
                }
            }
            return BackendStatus::Failed("XR Corpus is unavailable".into());
        }
        if self.child.is_none() {
            if let Err(error) = self.start_backend() {
                return BackendStatus::Failed(error);
            }
            return BackendStatus::Starting(BackendStartupStage::Inference);
        }
        let Some(child) = &mut self.child else {
            return BackendStatus::Failed("Backend process is no longer running".into());
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.child = None;
                #[cfg(windows)]
                {
                    self.job = None;
                }
                self.finish_log_capture();
                let log = self.get_latest_log();
                let detail = if log.trim().is_empty() {
                    format!("Backend launcher exited before it became ready ({status})")
                } else {
                    let summary = startup_error_summary(&log).unwrap_or_else(|| {
                        format!("Backend launcher exited before it became ready ({status})")
                    });
                    format!("{summary}\n\nLog Traceback:\n{}", log.trim())
                };
                BackendStatus::Failed(detail)
            }
            Ok(None) => BackendStatus::Starting(BackendStartupStage::Inference),
            Err(error) => BackendStatus::Failed(format!("Cannot inspect backend process: {error}")),
        }
    }

    pub fn get_latest_log(&self) -> String {
        self.log_policy.read_current(DIAGNOSTIC_READ_BYTES)
    }

    pub fn shutdown(&mut self) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.terminate();
        }

        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if let Some(mut child) = self.corpus_child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        self.finish_log_capture();
        self.finish_corpus_log_capture();
    }

    fn finish_log_capture(&mut self) {
        if let Some(capture) = self.log_capture.take() {
            capture.finish();
        }
    }

    fn finish_corpus_log_capture(&mut self) {
        if let Some(capture) = self.corpus_log_capture.take() {
            capture.finish();
        }
    }

    fn start_backend(&mut self) -> Result<(), String> {
        // Revalidate and persist immediately before the backend reads
        // config.json. This also retries a recovery write that may have failed
        // transiently during application startup.
        let config = load_project_config(&self.project_root)
            .map_err(|error| format!("Cannot read native route: {error}"))?;
        let use_local_runtime = config
            .native_model_route()
            .map_err(|error| error.to_string())?
            .uses_local_runtime();
        if use_local_runtime {
            self.save_llama_server_path()?;
        }
        let (mut command, capture_output) = self.native_backend_command_with_log()?;
        command
            .arg("--config")
            .arg(self.project_root.join("config.json"))
            .arg("--corpus-url")
            .arg(CORPUS_SERVER_URL);
        if use_local_runtime {
            command.arg("--manage-llama-servers");
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot start backend: {error}"))?;
        let log_capture = capture_output.then(|| {
            BoundedLogCapture::start(
                &mut child,
                self.log_policy.clone(),
                std::env::var_os("XRTRANSLATE_BACKEND_CONSOLE_LOG").is_some(),
            )
        });

        #[cfg(windows)]
        {
            let job = KillOnCloseJob::new()?;
            if let Err(error) = job.assign(&child) {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                if let Some(capture) = log_capture {
                    capture.finish();
                }
                return Err(error);
            }
            if let Some(corpus_child) = &self.corpus_child
                && let Err(error) = job.assign(corpus_child)
            {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                if let Some(capture) = log_capture {
                    capture.finish();
                }
                return Err(error);
            }
            self.job = Some(job);
            self.child = Some(child);
            self.log_capture = log_capture;
        }
        #[cfg(not(windows))]
        {
            self.child = Some(child);
            self.log_capture = log_capture;
        }
        Ok(())
    }

    fn start_corpus(&mut self) -> Result<(), String> {
        let executable = self.resolve_corpus_executable()?;
        let mut command = Command::new(executable);
        command
            .current_dir(&self.project_root)
            .stdin(Stdio::null())
            .arg("--config")
            .arg(self.project_root.join("config.json"));
        let mirror_to_console = std::env::var_os("XRTRANSLATE_BACKEND_CONSOLE_LOG").is_some();
        let capture_output = fs::create_dir_all(&self.corpus_log_policy.directory).is_ok();
        if capture_output {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else if mirror_to_console {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        crate::child_process::hide_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot start XR Corpus: {error}"))?;
        self.corpus_log_capture = capture_output.then(|| {
            BoundedLogCapture::start(
                &mut child,
                self.corpus_log_policy.clone(),
                mirror_to_console,
            )
        });
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.assign(&child)?;
        }
        self.corpus_child = Some(child);
        Ok(())
    }

    fn native_backend_command_with_log(&self) -> Result<(Command, bool), String> {
        let executable = self.resolve_native_backend_executable()?;
        let mut command = Command::new(executable);
        command.current_dir(&self.project_root).stdin(Stdio::null());

        let mirror_to_console = std::env::var_os("XRTRANSLATE_BACKEND_CONSOLE_LOG").is_some();
        let capture_output = fs::create_dir_all(&self.log_policy.directory).is_ok();
        if capture_output {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else if mirror_to_console {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        crate::child_process::hide_console(&mut command);
        Ok((command, capture_output))
    }

    fn resolve_native_backend_executable(&self) -> Result<PathBuf, String> {
        let executable = if cfg!(windows) {
            "xrtranslate-backend.exe"
        } else {
            "xrtranslate-backend"
        };
        let debug_binary = self
            .project_root
            .join("target")
            .join("debug")
            .join(executable);
        let release_binary = self
            .project_root
            .join("target")
            .join("release")
            .join(executable);
        let packaged_binary = self.project_root.join("bin").join(executable);
        let candidates = if cfg!(debug_assertions) {
            [
                self.project_root.join(executable),
                packaged_binary.clone(),
                self.project_root.join("backend").join(executable),
                debug_binary,
                release_binary,
            ]
        } else {
            [
                self.project_root.join(executable),
                packaged_binary,
                self.project_root.join("backend").join(executable),
                release_binary,
                debug_binary,
            ]
        };
        candidates.iter().find(|path| path.is_file()).cloned().ok_or_else(|| {
            format!(
                "Native backend executable was not found. Build xrtranslate-backend or use the packaged application. Looked for:\n{}",
                candidates
                    .iter()
                    .map(|path| format!("- {}", path.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
    }

    fn resolve_corpus_executable(&self) -> Result<PathBuf, String> {
        self.resolve_managed_executable("xr-corpus-server")
    }

    fn resolve_managed_executable(&self, name: &str) -> Result<PathBuf, String> {
        let executable = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let candidates = [
            self.project_root.join(&executable),
            self.project_root.join("bin").join(&executable),
            self.project_root.join("backend").join(&executable),
            self.project_root
                .join("target")
                .join("debug")
                .join(&executable),
            self.project_root
                .join("target")
                .join("release")
                .join(&executable),
        ];
        candidates
            .iter()
            .find(|path| path.is_file())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{name} executable was not found. Looked for:\n{}",
                    candidates
                        .iter()
                        .map(|path| format!("- {}", path.display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })
    }
}

#[derive(Clone)]
struct BackendLogPolicy {
    directory: PathBuf,
    file_name: String,
    max_file_bytes: u64,
    retained_files: usize,
}

impl BackendLogPolicy {
    fn new(project_root: &std::path::Path, storage: StorageConfig, file_name: &str) -> Self {
        let directory = if storage.log_dir.as_os_str().is_empty() {
            project_root.join("runtime").join("logs")
        } else {
            absolute_from_project_root(project_root, storage.log_dir)
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

    fn active_path(&self) -> PathBuf {
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

    fn read_current(&self, maximum_bytes: usize) -> String {
        let mut bytes = Vec::new();
        append_file_tail(&mut bytes, &self.active_path(), maximum_bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

fn startup_error_summary(log: &str) -> Option<String> {
    log.lines().rev().find_map(|line| {
        line.find(STARTUP_ERROR_MARKER).and_then(|index| {
            let message = line[index + STARTUP_ERROR_MARKER.len()..].trim();
            (!message.is_empty()).then(|| message.to_owned())
        })
    })
}

struct BoundedLogCapture {
    readers: Vec<JoinHandle<()>>,
    writer: JoinHandle<()>,
}

impl BoundedLogCapture {
    fn start(child: &mut Child, policy: BackendLogPolicy, mirror_to_console: bool) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(64);
        let writer = thread::Builder::new()
            .name("backend-log-writer".into())
            .spawn(move || write_bounded_log(policy, receiver, mirror_to_console))
            .expect("backend log writer thread must start");
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_log_reader("backend-stdout", stdout, sender.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_log_reader("backend-stderr", stderr, sender.clone()));
        }
        drop(sender);
        Self { readers, writer }
    }

    fn finish(mut self) {
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        let _ = self.writer.join();
    }
}

fn spawn_log_reader(
    name: &str,
    mut stream: impl Read + Send + 'static,
    sender: mpsc::SyncSender<Vec<u8>>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buffer = vec![0_u8; 16 * 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if sender.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .expect("backend log reader thread must start")
}

fn write_bounded_log(
    policy: BackendLogPolicy,
    receiver: mpsc::Receiver<Vec<u8>>,
    mirror_to_console: bool,
) {
    if fs::create_dir_all(&policy.directory).is_err() {
        for _ in receiver {}
        return;
    }
    policy.rotate();
    let mut output = fs::File::create(policy.active_path()).ok();
    let mut written = 0_u64;
    for mut chunk in receiver {
        if mirror_to_console {
            let _ = std::io::stdout().write_all(&chunk);
            let _ = std::io::stdout().flush();
        }
        if output.is_none() {
            continue;
        }
        if chunk.len() as u64 > policy.max_file_bytes {
            let keep = policy.max_file_bytes as usize;
            chunk.drain(..chunk.len() - keep);
        }
        if written > 0 && written.saturating_add(chunk.len() as u64) > policy.max_file_bytes {
            output.take();
            policy.rotate();
            output = fs::File::create(policy.active_path()).ok();
            written = 0;
        }
        if let Some(file) = output.as_mut() {
            if file.write_all(&chunk).is_err() {
                output.take();
                continue;
            }
            written = written.saturating_add(chunk.len() as u64);
        }
    }
    if let Some(mut output) = output {
        let _ = output.flush();
    }
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

impl Drop for BackendManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn project_root() -> PathBuf {
    for start in [std::env::current_dir().ok(), std::env::current_exe().ok()] {
        let Some(start) = start else {
            continue;
        };
        let directory = if start.is_dir() {
            start
        } else {
            start.parent().map(PathBuf::from).unwrap_or(start)
        };
        for ancestor in directory.ancestors() {
            if ancestor.join("config.json").exists()
                && (ancestor.join("Cargo.toml").exists()
                    || ancestor.join("xrtranslate-backend.exe").exists()
                    || ancestor
                        .join("bin")
                        .join("xrtranslate-backend.exe")
                        .exists()
                    || ancestor.join("xrtranslate-backend").exists()
                    || ancestor.join("bin").join("xrtranslate-backend").exists())
            {
                return ancestor.into();
            }
        }
    }
    PathBuf::from(".")
}

fn load_project_config(project_root: &std::path::Path) -> Result<AppConfig, String> {
    let path = project_root.join("config.json");
    AppConfig::from_path_with_user_config(&path, project_root)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))
}

fn absolute_from_project_root(project_root: &std::path::Path, path: PathBuf) -> PathBuf {
    let candidate = RuntimeLayout::for_project_root(project_root).resolve_configured_path(path);
    std::path::absolute(&candidate).unwrap_or(candidate)
}

fn config_path_value(path: &std::path::Path) -> String {
    let value = path.display().to_string();
    if path.is_relative() {
        value.replace('\\', "/")
    } else {
        value
    }
}

fn preferred_llama_server_path(layout: &RuntimeLayout, configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() {
        let candidate = configured_llama_server_path(layout, configured);
        if candidate.is_file() {
            return candidate.display().to_string();
        }

        // Keep the normalized managed path visible to the installer and the
        // error UI when the runtime has not been downloaded yet. External
        // stale paths retain their original value for manual repair.
        if is_managed_llama_server_path(layout, &candidate) {
            return candidate.display().to_string();
        }
    }

    let installed =
        layout.managed_llama_server(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
    let installed = std::path::absolute(&installed).unwrap_or(installed);
    if installed.is_file() {
        installed.display().to_string()
    } else {
        configured.to_owned()
    }
}

fn configured_llama_server_path(layout: &RuntimeLayout, configured: &str) -> PathBuf {
    let configured_path = PathBuf::from(configured);
    let mut candidate = layout.resolve_configured_path(&configured_path);

    // The shared config keeps the managed executable extensionless so the
    // same default works on Unix. Windows archives contain llama-server.exe.
    if cfg!(windows)
        && configured_path.is_relative()
        && candidate == layout.managed_llama_server("llama-server")
    {
        candidate.set_extension("exe");
    }

    std::path::absolute(&candidate).unwrap_or(candidate)
}

fn is_managed_llama_server_path(layout: &RuntimeLayout, path: &std::path::Path) -> bool {
    let is_supported_name = path.file_stem().is_some_and(|name| name == "llama-server")
        && match path.extension().and_then(|extension| extension.to_str()) {
            None => true,
            Some("exe") => cfg!(windows),
            Some(_) => false,
        };
    path.parent() == Some(layout.llama_cpp_directory().as_path()) && is_supported_name
}

fn is_local_server(server_url: &str) -> bool {
    let address = server_address(server_url).unwrap_or_default();
    let host = if let Some(ipv6) = address.strip_prefix('[') {
        ipv6.split(']').next().unwrap_or_default()
    } else {
        address.split(':').next().unwrap_or_default()
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn server_reachable(server_url: &str) -> bool {
    let Some(address) = server_address(server_url) else {
        return false;
    };
    let Ok(addresses) = address.to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(25)).is_ok())
}

fn server_address(server_url: &str) -> Option<&str> {
    let (scheme, without_scheme) = server_url.split_once("://")?;
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ws" | "wss"
    ) {
        return None;
    }
    let address = without_scheme.split('/').next()?.trim();
    (!address.is_empty()).then_some(address)
}

#[cfg(windows)]
struct KillOnCloseJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl KillOnCloseJob {
    fn new() -> Result<Self, String> {
        use std::mem::size_of;
        use windows_sys::Win32::{
            Foundation::{GetLastError, INVALID_HANDLE_VALUE},
            System::JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(format!("Cannot create backend process job: {}", unsafe {
                GetLastError()
            }));
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return Err(format!(
                "Cannot configure backend process job: {}",
                unsafe { GetLastError() }
            ));
        }
        Ok(Self { handle })
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::GetLastError, System::JobObjects::AssignProcessToJobObject,
        };
        let assigned = unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as _) };
        if assigned == 0 {
            return Err(format!("Cannot manage backend process tree: {}", unsafe {
                GetLastError()
            }));
        }
        Ok(())
    }

    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("xrtranslate-test-backend-{label}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn create_server(root: &std::path::Path) -> PathBuf {
        let server = root
            .join("runtime")
            .join("llama.cpp")
            .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(server.parent().unwrap()).unwrap();
        std::fs::write(&server, b"test").unwrap();
        server
    }

    #[test]
    fn relative_configured_runtime_is_resolved_from_project_root() {
        let root = temp_root("relative");
        let server = create_server(&root);
        let layout = RuntimeLayout::for_project_root(&root);
        let configured = format!(
            "runtime/llama.cpp/llama-server{}",
            std::env::consts::EXE_SUFFIX
        );
        let selected = preferred_llama_server_path(&layout, &configured);
        assert_eq!(PathBuf::from(selected), server);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn extensionless_managed_runtime_is_resolved_to_windows_executable() {
        let root = temp_root("extensionless-windows");
        let server = create_server(&root);
        let layout = RuntimeLayout::for_project_root(&root);
        let selected = preferred_llama_server_path(&layout, "runtime/llama.cpp/llama-server");
        assert_eq!(PathBuf::from(selected), server);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standard_runtime_is_recovered_when_config_is_empty_or_stale() {
        let root = temp_root("recover");
        let server = create_server(&root);
        let layout = RuntimeLayout::for_project_root(&root);
        assert_eq!(
            PathBuf::from(preferred_llama_server_path(&layout, "")),
            server
        );
        assert_eq!(
            PathBuf::from(preferred_llama_server_path(
                &layout,
                "C:/missing/llama-server.exe"
            )),
            server
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_runtime_directory_is_used_by_preferred_llama_server() {
        let root = temp_root("custom-runtime-dir");
        let custom_runtime = root.join("custom_ai_runtime");
        let server = custom_runtime
            .join("llama.cpp")
            .join(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(server.parent().unwrap()).unwrap();
        std::fs::write(&server, b"test").unwrap();

        let layout = RuntimeLayout::new(&root, Some("custom_ai_runtime"));
        assert_eq!(
            PathBuf::from(preferred_llama_server_path(&layout, "")),
            server
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_runtime_is_written_to_config_as_a_project_relative_path() {
        let root = temp_root("persist");
        let server = create_server(&root);
        std::fs::write(root.join("config.json"), b"{\"model_manager\":{}}").unwrap();

        let persisted = BackendManager::persist_llama_server_path(&root, &server).unwrap();
        let config =
            xrtranslate_config::load_user_config_document(root.join("config.json"), &root).unwrap();

        assert!(persisted.is_absolute());
        assert_eq!(
            config["model_manager"]["llama_server_path"],
            format!(
                "runtime/llama.cpp/llama-server{}",
                std::env::consts::EXE_SUFFIX
            )
        );
        let base: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("config.json")).unwrap())
                .unwrap();
        assert!(base["model_manager"]["llama_server_path"].is_null());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_service_urls_accept_http_and_websocket_schemes() {
        assert_eq!(
            server_address("http://127.0.0.1:7766/healthz"),
            Some("127.0.0.1:7766")
        );
        assert_eq!(
            server_address("ws://localhost:8000/ws"),
            Some("localhost:8000")
        );
        assert!(is_local_server("https://[::1]:7766/healthz"));
        assert_eq!(server_address("file:///tmp/service"), None);
    }

    #[test]
    fn startup_error_marker_is_preferred_over_noisy_model_logs() {
        let log = "normal model output\n[XRTRANSLATE_STARTUP_ERROR] port 8001 is already in use\nmore shutdown output";
        assert_eq!(
            startup_error_summary(log).as_deref(),
            Some("port 8001 is already in use")
        );
    }
}
