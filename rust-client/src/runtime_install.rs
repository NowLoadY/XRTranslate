//! Native llama.cpp runtime discovery and installation.
//!
//! The configured `model_manager.llama_cpp.downloads` list is the contract:
//! we select CUDA when the installed NVIDIA driver reports a compatible
//! runtime, adequate compute capability, and at least 8 GiB of VRAM. Managed
//! model packages never fall back to CPU; bundled small ONNX components are a
//! separate application resource class and do not use this installer.

use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
};
use xrtranslate_config::{
    AppConfig, LlamaCppArchiveFormat, LlamaCppAssetKind, LlamaCppRuntimeConfig,
    ManagedRuntimeArchive, NativeRuntimeBackend, NativeRuntimeSelection, OnnxRuntimeConfig,
    RuntimeLayout, RuntimeRequirements,
};
use xrtranslate_download::{DownloadCancellation, DownloadClient, DownloadSource, DownloadSpec};

const MIN_CUDA_COMPUTE_CAPABILITY: (u16, u16) = (6, 0);
const MIN_LOCAL_MODEL_VRAM_BYTES: u64 = xrtranslate_assets::MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES;
const TURING_COMPUTE_CAPABILITY: (u16, u16) = (7, 5);
const BLACKWELL_MINIMUM_CUDA: (u16, u16) = (12, 8);
pub(crate) const NVIDIA_APP_URL: &str = "https://www.nvidia.com/en-us/software/nvidia-app/";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeBackend {
    Cpu,
    Cuda,
}

#[derive(Clone, Debug)]
struct RuntimeSelection {
    assets: Vec<ReleaseAsset>,
    backend: RuntimeBackend,
    executable: String,
    fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct OnnxRuntimeSelection {
    backend: RuntimeBackend,
    provider: Option<ManagedRuntimeAsset>,
    cuda_runtime: Option<ReleaseAsset>,
    cudnn: Option<ManagedRuntimeAsset>,
    cuda_version: Option<String>,
    fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct RuntimePlan {
    llama_cpp: Option<RuntimeSelection>,
    onnx: Option<OnnxRuntimeSelection>,
    download_bytes: u64,
    marker_ready: bool,
    requirements: RuntimeRequirements,
    local_models: LocalModelAvailability,
    blocking_error: Option<String>,
}

impl RuntimePlan {
    fn total_bytes(&self) -> u64 {
        self.download_bytes
    }

    fn backend(&self) -> RuntimeBackend {
        self.onnx
            .as_ref()
            .map(|selection| selection.backend)
            .or_else(|| self.llama_cpp.as_ref().map(|selection| selection.backend))
            .unwrap_or(RuntimeBackend::Cpu)
    }

    fn is_ready(&self) -> bool {
        self.blocking_error.is_none() && self.download_bytes == 0 && self.marker_ready
    }

    fn requires_marker_repair(&self) -> bool {
        self.blocking_error.is_none() && self.download_bytes == 0 && !self.marker_ready
    }
}

impl RuntimeBackend {
    const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda => "CUDA",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NvidiaCuda {
    gpu: String,
    compute_capability: (u16, u16),
    driver_cuda: String,
    memory_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalModelAvailability {
    Detecting,
    Available { gpu: String, memory_bytes: u64 },
    Unavailable(String),
}

#[derive(Clone, Debug)]
pub enum RuntimeInstallState {
    Idle,
    Detecting,
    Ready,
    Downloading {
        asset: String,
        downloaded: u64,
        total: u64,
    },
    Extracting,
    Installed,
    Failed(String),
}

impl RuntimeInstallState {
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Detecting | Self::Downloading { .. } | Self::Extracting
        )
    }
}

#[derive(Debug)]
enum Event {
    Prepared(Result<RuntimePlan, String>),
    Downloading {
        asset: String,
        downloaded: u64,
        total: u64,
    },
    Extracting,
    Cancelled,
    Finished(Result<PathBuf, String>),
}

/// One background worker for the optional automatic llama.cpp installer.
pub struct RuntimeInstaller {
    state: RuntimeInstallState,
    events: Option<Receiver<Event>>,
    selection: Option<RuntimePlan>,
    proxy_url: Option<String>,
    use_mirror: bool,
    cancellation: Option<DownloadCancellation>,
    active_project_root: Option<PathBuf>,
    restart_after_source_switch: bool,
}

impl Default for RuntimeInstaller {
    fn default() -> Self {
        Self {
            state: RuntimeInstallState::Idle,
            events: None,
            selection: None,
            proxy_url: None,
            use_mirror: false,
            cancellation: None,
            active_project_root: None,
            restart_after_source_switch: false,
        }
    }
}

impl RuntimeInstaller {
    pub fn set_proxy_url(&mut self, proxy_url: &str) {
        self.proxy_url = (!proxy_url.trim().is_empty()).then(|| proxy_url.trim().to_owned());
    }

    /// Switches the runtime transfer source without mixing partial archives
    /// from two channels. In-flight downloads stop cooperatively, staging is
    /// cleared after file handles close, then the selected plan restarts.
    pub fn switch_download_source(
        &mut self,
        project_root: PathBuf,
        use_mirror: bool,
    ) -> Result<(), String> {
        if self.use_mirror == use_mirror {
            return Ok(());
        }
        self.use_mirror = use_mirror;
        if self.cancellation.is_some() && self.is_busy() {
            self.restart_after_source_switch = true;
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            }
            return Ok(());
        }
        clear_runtime_staging(&project_root)
    }

    #[must_use]
    pub const fn use_mirror(&self) -> bool {
        self.use_mirror
    }
    #[must_use]
    pub fn state(&self) -> &RuntimeInstallState {
        &self.state
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.state.is_busy()
    }

    #[must_use]
    pub fn download_size_bytes(&self) -> Option<u64> {
        self.selection.as_ref().map(RuntimePlan::total_bytes)
    }

    #[must_use]
    pub fn plan_is_ready(&self) -> bool {
        self.selection.as_ref().is_some_and(RuntimePlan::is_ready)
    }

    #[must_use]
    pub fn plan_matches(&self, requirements: RuntimeRequirements) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| selection.requirements == requirements)
    }

    #[must_use]
    pub fn backend_label(&self) -> Option<&'static str> {
        self.selection
            .as_ref()
            .filter(|selection| selection.blocking_error.is_none())
            .map(|selection| selection.backend().label())
    }

    #[must_use]
    pub fn local_model_availability(&self) -> LocalModelAvailability {
        if self.is_busy() {
            return LocalModelAvailability::Detecting;
        }
        if let RuntimeInstallState::Failed(error) = &self.state
            && self.selection.is_none()
        {
            return LocalModelAvailability::Unavailable(error.clone());
        }
        self.selection
            .as_ref()
            .map(|plan| plan.local_models.clone())
            .unwrap_or(LocalModelAvailability::Detecting)
    }

    #[must_use]
    pub fn cuda_version_label(&self) -> Option<&str> {
        let plan = self.selection.as_ref()?;
        plan.onnx
            .as_ref()
            .and_then(|selection| selection.cuda_version.as_deref())
            .or_else(|| {
                plan.llama_cpp.as_ref().and_then(|selection| {
                    selection
                        .assets
                        .iter()
                        .find(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime)
                        .and_then(|asset| asset.cuda_version.as_deref())
                })
            })
    }

    #[must_use]
    pub fn fallback_reason(&self) -> Option<&str> {
        let plan = self.selection.as_ref()?;
        plan.onnx
            .as_ref()
            .and_then(|selection| selection.fallback_reason.as_deref())
            .or_else(|| {
                plan.llama_cpp
                    .as_ref()
                    .and_then(|selection| selection.fallback_reason.as_deref())
            })
    }

    /// Rebuilds the union plan after provider choices change. Only required
    /// consumers participate, and CUDA archives shared by llama.cpp and ONNX
    /// are counted once.
    pub fn prepare_for(
        &mut self,
        project_root: PathBuf,
        requirements: RuntimeRequirements,
    ) -> Result<(), String> {
        if self.is_busy() {
            return Err("A native runtime installation is already running.".into());
        }
        let (sender, receiver) = unbounded();
        let worker_root = project_root.clone();
        thread::Builder::new()
            .name("native-runtime-planner".into())
            .spawn(move || {
                let result = configured_runtime_plan(&worker_root, requirements);
                let _ = sender.send(Event::Prepared(result));
            })
            .map_err(|error| format!("Cannot start native runtime planner: {error}"))?;
        self.selection = None;
        self.state = RuntimeInstallState::Detecting;
        self.events = Some(receiver);
        self.active_project_root = Some(project_root);
        Ok(())
    }

    pub fn install_recommended(&mut self, project_root: PathBuf) -> Result<(), String> {
        if self.is_busy() {
            return Err("A llama.cpp installation is already running.".into());
        }
        let selection = self.selection.clone().ok_or_else(|| {
            "The llama.cpp download plan is not ready. Wait for hardware detection to finish."
                .to_owned()
        })?;
        if let Some(error) = selection.blocking_error.as_deref() {
            return Err(error.to_owned());
        }
        let (sender, receiver) = unbounded();
        let proxy_url = self.proxy_url.clone();
        let source = DownloadSource::from_mirror_enabled(self.use_mirror);
        let cancellation = DownloadCancellation::default();
        let worker_cancellation = cancellation.clone();
        let worker_root = project_root.clone();
        thread::Builder::new()
            .name("llama-cpp-installer".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("Cannot create download runtime: {error}"))
                    .and_then(|runtime| {
                        runtime.block_on(async {
                            install_runtime_plan(
                                worker_root.clone(),
                                selection,
                                sender.clone(),
                                proxy_url.as_deref(),
                                source,
                                worker_cancellation.clone(),
                            )
                            .await
                        })
                    });
                if worker_cancellation.is_cancelled() {
                    let cleanup = clear_runtime_staging(&worker_root);
                    let _ = match cleanup {
                        Ok(()) => sender.send(Event::Cancelled),
                        Err(error) => sender.send(Event::Finished(Err(error))),
                    };
                } else {
                    let _ = sender.send(Event::Finished(result));
                }
            })
            .map_err(|error| format!("Cannot start llama.cpp installer: {error}"))?;
        self.state = RuntimeInstallState::Downloading {
            asset: String::new(),
            downloaded: 0,
            total: self.download_size_bytes().unwrap_or(0),
        };
        self.events = Some(receiver);
        self.cancellation = Some(cancellation);
        self.active_project_root = Some(project_root);
        Ok(())
    }

    #[must_use]
    pub fn managed_resources_are_present(&self, project_root: &Path) -> bool {
        let Ok(config) = load_app_config(project_root) else {
            return false;
        };
        let layout = config.runtime_layout(project_root);
        if !paths_refer_to_same_location(
            layout.runtime_root(),
            &project_root.join(RuntimeLayout::DEFAULT_RUNTIME_DIRECTORY),
        ) {
            return false;
        }
        layout.llama_cpp_directory().is_dir()
            || config
                .model_manager
                .llama_cpp
                .downloads
                .iter()
                .filter_map(|asset| asset.cuda_version.as_deref())
                .any(|version| layout.cuda_runtime_directory(version).is_dir())
            || config
                .model_manager
                .onnxruntime
                .downloads
                .iter()
                .any(|asset| layout.onnx_runtime_directory(&asset.cuda_version).is_dir())
            || config
                .model_manager
                .onnxruntime
                .cudnn_downloads
                .iter()
                .any(|asset| layout.cudnn_runtime_directory(&asset.cuda_version).is_dir())
    }

    /// Removes every runtime component managed by the download catalogue while
    /// preserving the packaged CPU ONNX core and any user-selected external
    /// runtime directory.
    pub fn delete_managed_resources(&mut self, project_root: &Path) -> Result<(), String> {
        if self.is_busy() {
            return Err("Wait for runtime preparation before deleting runtime resources.".into());
        }
        let config = load_app_config(project_root)?;
        let layout = config.runtime_layout(project_root);
        let managed_root = project_root.join(RuntimeLayout::DEFAULT_RUNTIME_DIRECTORY);
        if !paths_refer_to_same_location(layout.runtime_root(), &managed_root) {
            return Err(
                "The selected runtime directory is external and will not be deleted automatically."
                    .into(),
            );
        }

        remove_managed_directory(&layout.llama_cpp_directory())?;
        let cuda_versions = config
            .model_manager
            .llama_cpp
            .downloads
            .iter()
            .filter_map(|asset| asset.cuda_version.as_deref())
            .collect::<HashSet<_>>();
        for version in cuda_versions {
            remove_managed_directory(&layout.cuda_runtime_directory(version))?;
        }
        let onnx_versions = config
            .model_manager
            .onnxruntime
            .downloads
            .iter()
            .map(|asset| asset.cuda_version.as_str())
            .collect::<HashSet<_>>();
        for version in onnx_versions {
            remove_managed_directory(&layout.onnx_runtime_directory(version))?;
        }
        let cudnn_versions = config
            .model_manager
            .onnxruntime
            .cudnn_downloads
            .iter()
            .map(|asset| asset.cuda_version.as_str())
            .collect::<HashSet<_>>();
        for version in cudnn_versions {
            remove_managed_directory(&layout.cudnn_runtime_directory(version))?;
        }
        match fs::remove_file(layout.native_runtime_selection_file()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot remove native runtime marker: {error}")),
        }
        clear_runtime_staging(project_root)?;
        self.state = RuntimeInstallState::Idle;
        self.events = None;
        self.selection = None;
        self.cancellation = None;
        self.active_project_root = None;
        Ok(())
    }

    pub fn poll(&mut self) -> Option<PathBuf> {
        let Some(events) = &self.events else {
            return None;
        };
        let mut finished = false;
        let mut cancelled = false;
        let mut repair_prepared_marker = false;
        let mut installed_executable = None;
        loop {
            match events.try_recv() {
                Ok(Event::Prepared(result)) => {
                    match result {
                        Ok(selection) => {
                            repair_prepared_marker = selection.requires_marker_repair();
                            self.state = selection
                                .blocking_error
                                .as_ref()
                                .map_or(RuntimeInstallState::Ready, |error| {
                                    RuntimeInstallState::Failed(error.clone())
                                });
                            self.selection = Some(selection);
                        }
                        Err(error) => self.state = RuntimeInstallState::Failed(error),
                    }
                    finished = true;
                    break;
                }
                Ok(Event::Downloading {
                    asset,
                    downloaded,
                    total,
                }) => {
                    self.state = RuntimeInstallState::Downloading {
                        asset,
                        downloaded,
                        total,
                    };
                }
                Ok(Event::Extracting) => self.state = RuntimeInstallState::Extracting,
                Ok(Event::Cancelled) => {
                    self.state = RuntimeInstallState::Idle;
                    cancelled = true;
                    finished = true;
                    break;
                }
                Ok(Event::Finished(result)) => {
                    if result.is_ok()
                        && let Some(selection) = self.selection.as_mut()
                    {
                        selection.download_bytes = 0;
                        selection.marker_ready = true;
                    }
                    self.state = match result {
                        Ok(path) => {
                            installed_executable = Some(path);
                            RuntimeInstallState::Installed
                        }
                        Err(error) => RuntimeInstallState::Failed(error),
                    };
                    finished = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state = RuntimeInstallState::Failed(
                        "The llama.cpp installer stopped unexpectedly.".into(),
                    );
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.events = None;
            self.cancellation = None;
            let project_root = self.active_project_root.take();
            if repair_prepared_marker {
                if let Some(project_root) = project_root
                    && let Err(error) = self.install_recommended(project_root)
                {
                    self.state = RuntimeInstallState::Failed(error);
                }
            } else if cancelled && self.restart_after_source_switch {
                self.restart_after_source_switch = false;
                if let Some(project_root) = project_root
                    && let Err(error) = self.install_recommended(project_root)
                {
                    self.state = RuntimeInstallState::Failed(error);
                }
            }
        }
        installed_executable
    }
}

fn paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    let left = std::path::absolute(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::path::absolute(right).unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn remove_managed_directory(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Cannot remove managed runtime resource {}: {error}",
            path.display()
        )),
    }
}

#[derive(Clone, Debug)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    sha256: String,
    archive_format: LlamaCppArchiveFormat,
    kind: LlamaCppAssetKind,
    target: String,
    cuda_version: Option<String>,
    executable: String,
    required_files: Vec<String>,
    required_file_prefixes: Vec<String>,
}

#[derive(Clone, Debug)]
struct ManagedRuntimeAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    sha256: String,
    archive_format: LlamaCppArchiveFormat,
    target: String,
    cuda_version: String,
    archive_directory: String,
    required_files: Vec<String>,
}

async fn install_onnx_runtime(
    project_root: PathBuf,
    selection: OnnxRuntimeSelection,
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
    progress_base: u64,
    progress_total: u64,
) -> Result<PathBuf, String> {
    let layout = load_runtime_layout(&project_root);
    if selection.backend == RuntimeBackend::Cpu {
        let existing = load_native_runtime_selection(&layout)?;
        let marker = NativeRuntimeSelection {
            schema_version: 1,
            backend: NativeRuntimeBackend::Cpu,
            llama_cpp_backend: existing
                .as_ref()
                .and_then(|marker| marker.llama_cpp_backend),
            onnx_backend: Some(NativeRuntimeBackend::Cpu),
            cuda_version: existing
                .as_ref()
                .and_then(|marker| marker.cuda_version.clone()),
            provider_dir: None,
            onnx_core_library: Some(layout.config_path_for(layout.onnx_cpu_core_library())),
            cuda_bin_dir: existing
                .as_ref()
                .and_then(|marker| marker.cuda_bin_dir.clone()),
            cudnn_bin_dir: existing
                .as_ref()
                .and_then(|marker| marker.cudnn_bin_dir.clone()),
            preload_libraries: Vec::new(),
            fallback_reason: selection.fallback_reason.clone(),
        };
        persist_native_runtime_selection(&layout, &marker)?;
        return Ok(layout.native_runtime_selection_file());
    }

    let provider = selection
        .provider
        .as_ref()
        .ok_or_else(|| "CUDA ONNX plan is missing its provider archive".to_owned())?;
    let cuda_runtime = selection
        .cuda_runtime
        .as_ref()
        .ok_or_else(|| "CUDA ONNX plan is missing its CUDA runtime archive".to_owned())?;
    let cudnn = selection
        .cudnn
        .as_ref()
        .ok_or_else(|| "CUDA ONNX plan is missing its cuDNN runtime archive".to_owned())?;
    let cuda_version = selection
        .cuda_version
        .as_deref()
        .ok_or_else(|| "CUDA ONNX plan has no CUDA version".to_owned())?;
    let cuda_directory = layout.cuda_runtime_directory(cuda_version);
    let cudnn_directory = layout.cudnn_runtime_directory(&cudnn.cuda_version);
    let provider_directory = layout.onnx_runtime_directory(&provider.cuda_version);
    let cuda_ready =
        validate_required_prefixes(&cuda_directory, &cuda_runtime.required_file_prefixes).is_ok();
    let provider_ready =
        validate_required_files(&provider_directory, &provider.required_files).is_ok();
    let cudnn_ready = validate_required_files(&cudnn_directory, &cudnn.required_files).is_ok();

    let release = load_onnx_runtime_config(&project_root)?.release;
    let runtime_root = layout.runtime_root().to_path_buf();
    let staging = runtime_root.join(format!(".onnxruntime-{release}-staging"));
    prune_named_runtime_staging(&runtime_root, &staging, ".onnxruntime-")?;
    let downloads = staging.join("downloads");
    let payload = staging.join("payload");
    fs::create_dir_all(&downloads)
        .map_err(|error| format!("Cannot create ONNX runtime staging folder: {error}"))?;
    if payload.exists() {
        fs::remove_dir_all(&payload)
            .map_err(|error| format!("Cannot reset ONNX runtime extraction folder: {error}"))?;
    }
    fs::create_dir_all(&payload)
        .map_err(|error| format!("Cannot create ONNX runtime extraction folder: {error}"))?;
    let client = DownloadClient::with_proxy_source_and_cancellation(
        "XRTranslate ONNX runtime installer",
        proxy_url,
        source,
        cancellation,
    )
    .map_err(|error| error.to_string())?;
    let mut completed = 0_u64;

    if !cuda_ready {
        let archive = downloads.join(&cuda_runtime.name);
        download_runtime_asset(
            &client,
            cuda_runtime,
            &archive,
            progress_base.saturating_add(completed),
            progress_total,
            &sender,
        )
        .await?;
        completed = completed.saturating_add(cuda_runtime.size);
        let staged_cuda = payload.join("cuda");
        fs::create_dir_all(&staged_cuda)
            .map_err(|error| format!("Cannot create staged CUDA folder: {error}"))?;
        extract_archive(&archive, &staged_cuda, cuda_runtime.archive_format)?;
        validate_required_prefixes(&staged_cuda, &cuda_runtime.required_file_prefixes)?;
        activate_runtime_directory(&staged_cuda, &cuda_directory)?;
    }

    if !cudnn_ready {
        let archive = downloads.join(&cudnn.name);
        download_managed_runtime_asset(
            &client,
            cudnn,
            &archive,
            progress_base.saturating_add(completed),
            progress_total,
            &sender,
        )
        .await?;
        completed = completed.saturating_add(cudnn.size);
        let staged_cudnn = payload.join("cudnn");
        fs::create_dir_all(&staged_cudnn)
            .map_err(|error| format!("Cannot create staged cuDNN folder: {error}"))?;
        extract_declared_files(
            &archive,
            cudnn.archive_format,
            Path::new(&cudnn.archive_directory),
            &cudnn.required_files,
            &staged_cudnn,
        )?;
        validate_required_files(&staged_cudnn, &cudnn.required_files)?;
        activate_runtime_directory(&staged_cudnn, &cudnn_directory)?;
    }

    if !provider_ready {
        let archive = downloads.join(&provider.name);
        download_managed_runtime_asset(
            &client,
            provider,
            &archive,
            progress_base.saturating_add(completed),
            progress_total,
            &sender,
        )
        .await?;
        let staged_provider = payload.join("provider");
        fs::create_dir_all(&staged_provider)
            .map_err(|error| format!("Cannot create staged ONNX provider folder: {error}"))?;
        extract_declared_files(
            &archive,
            provider.archive_format,
            Path::new(&provider.archive_directory),
            &provider.required_files,
            &staged_provider,
        )?;
        validate_required_files(&staged_provider, &provider.required_files)?;
        activate_runtime_directory(&staged_provider, &provider_directory)?;
    }

    let mut preload_libraries =
        resolve_required_prefixes(&cuda_directory, &cuda_runtime.required_file_prefixes)?;
    preload_libraries.extend(resolve_required_files(
        &cudnn_directory,
        &cudnn.required_files,
    )?);
    let onnx_core_library = provider_directory.join(RuntimeLayout::ONNX_CORE_LIBRARY);
    let marker = NativeRuntimeSelection {
        schema_version: 1,
        backend: NativeRuntimeBackend::Cuda,
        llama_cpp_backend: load_native_runtime_selection(&layout)?
            .and_then(|marker| marker.llama_cpp_backend),
        onnx_backend: Some(NativeRuntimeBackend::Cuda),
        cuda_version: Some(cuda_version.into()),
        provider_dir: Some(layout.config_path_for(&provider_directory)),
        onnx_core_library: Some(layout.config_path_for(&onnx_core_library)),
        cuda_bin_dir: Some(layout.config_path_for(&cuda_directory)),
        cudnn_bin_dir: Some(layout.config_path_for(&cudnn_directory)),
        preload_libraries: preload_libraries
            .iter()
            .map(|path| layout.config_path_for(path))
            .collect(),
        fallback_reason: None,
    };
    persist_native_runtime_selection(&layout, &marker)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(layout.native_runtime_selection_file())
}

async fn download_runtime_asset(
    client: &DownloadClient,
    asset: &ReleaseAsset,
    archive: &Path,
    completed: u64,
    total: u64,
    sender: &crossbeam_channel::Sender<Event>,
) -> Result<(), String> {
    client
        .download_to(
            DownloadSpec::verified(
                &asset.name,
                &asset.browser_download_url,
                asset.size,
                &asset.sha256,
            ),
            archive,
            |progress| {
                let _ = sender.send(Event::Downloading {
                    asset: asset.name.clone(),
                    downloaded: completed.saturating_add(progress.downloaded_bytes),
                    total,
                });
            },
        )
        .await
        .map_err(|error| error.to_string())
}

async fn download_managed_runtime_asset(
    client: &DownloadClient,
    asset: &ManagedRuntimeAsset,
    archive: &Path,
    completed: u64,
    total: u64,
    sender: &crossbeam_channel::Sender<Event>,
) -> Result<(), String> {
    client
        .download_to(
            DownloadSpec::verified(
                &asset.name,
                &asset.browser_download_url,
                asset.size,
                &asset.sha256,
            ),
            archive,
            |progress| {
                let _ = sender.send(Event::Downloading {
                    asset: asset.name.clone(),
                    downloaded: completed.saturating_add(progress.downloaded_bytes),
                    total,
                });
            },
        )
        .await
        .map_err(|error| error.to_string())
}

async fn install_runtime_plan(
    project_root: PathBuf,
    plan: RuntimePlan,
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
) -> Result<PathBuf, String> {
    let progress_total =
        missing_runtime_bytes(&project_root, plan.llama_cpp.as_ref(), plan.onnx.as_ref());
    let llama_bytes = missing_runtime_bytes(&project_root, plan.llama_cpp.as_ref(), None);
    let mut llama_executable = None;
    let mut runtime_marker = None;
    if let Some(selection) = plan.llama_cpp {
        llama_executable = Some(
            install(
                project_root.clone(),
                selection,
                sender.clone(),
                proxy_url,
                source,
                cancellation.clone(),
                0,
                progress_total,
            )
            .await?,
        );
    }
    if let Some(selection) = plan.onnx {
        runtime_marker = Some(
            install_onnx_runtime(
                project_root.clone(),
                selection,
                sender,
                proxy_url,
                source,
                cancellation,
                llama_bytes,
                progress_total,
            )
            .await?,
        );
    }
    llama_executable
        .or(runtime_marker)
        .ok_or_else(|| "No native runtime is required by the selected providers.".into())
}

async fn install(
    project_root: PathBuf,
    selection: RuntimeSelection,
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
    progress_base: u64,
    progress_total: u64,
) -> Result<PathBuf, String> {
    let executable_name = selection.executable.clone();
    let layout = load_runtime_layout(&project_root);
    let target = layout.llama_cpp_directory();
    let executable = target.join(&executable_name);
    let server_assets = selection
        .assets
        .iter()
        .filter(|asset| asset.kind != LlamaCppAssetKind::CudaRuntime)
        .collect::<Vec<_>>();
    let server_required_files = server_assets
        .iter()
        .flat_map(|asset| asset.required_files.iter().cloned())
        .collect::<Vec<_>>();
    let server_required_prefixes = server_assets
        .iter()
        .flat_map(|asset| asset.required_file_prefixes.iter().cloned())
        .collect::<Vec<_>>();
    let cuda_asset = selection
        .assets
        .iter()
        .find(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime);
    let cuda_version = cuda_asset.and_then(|asset| asset.cuda_version.as_deref());
    let cuda_directory = cuda_version.map(|version| layout.cuda_runtime_directory(version));
    let server_ready = validate_runtime_files(
        &target,
        &selection.executable,
        &server_required_files,
        &server_required_prefixes,
    )
    .is_ok();
    let cuda_ready = match (cuda_asset, cuda_directory.as_deref()) {
        (Some(asset), Some(directory)) => {
            validate_required_prefixes(directory, &asset.required_file_prefixes).is_ok()
        }
        (None, None) => true,
        _ => false,
    };
    if server_ready && cuda_ready {
        persist_llama_runtime_marker(
            &layout,
            selection.backend,
            cuda_asset,
            cuda_directory.as_deref(),
            selection.fallback_reason.as_deref(),
        )?;
        return crate::backend::BackendManager::persist_llama_server_path_with_layout(
            &layout,
            &executable,
        );
    }
    if !server_ready && target.exists() && !target.is_dir() {
        return Err(format!(
            "{} already exists but is not a runtime directory containing {}.",
            target.display(),
            executable_name
        ));
    }

    let client = DownloadClient::with_proxy_source_and_cancellation(
        "XRTranslate runtime installer",
        proxy_url,
        source,
        cancellation,
    )
    .map_err(|error| error.to_string())?;
    let release = load_runtime_config(&project_root)?.release;
    let runtime_root = layout.runtime_root().to_path_buf();
    let staging = runtime_root.join(format!(".llama.cpp-{release}-staging"));
    prune_obsolete_runtime_staging(&runtime_root, &staging)?;
    let downloads = staging.join("downloads");
    let payload = staging.join("payload");
    fs::create_dir_all(&downloads)
        .map_err(|error| format!("Cannot create runtime staging folder: {error}"))?;
    let mut completed = 0_u64;
    for asset in selection.assets.iter().filter(|asset| {
        (asset.kind == LlamaCppAssetKind::CudaRuntime && !cuda_ready)
            || (asset.kind != LlamaCppAssetKind::CudaRuntime && !server_ready)
    }) {
        let archive = downloads.join(&asset.name);
        download_runtime_asset(
            &client,
            asset,
            &archive,
            progress_base.saturating_add(completed),
            progress_total,
            &sender,
        )
        .await?;
        completed = completed.saturating_add(asset.size);
    }
    let _ = sender.send(Event::Extracting);
    if payload.exists() {
        fs::remove_dir_all(&payload)
            .map_err(|error| format!("Cannot reset runtime extraction folder: {error}"))?;
    }
    fs::create_dir_all(&payload)
        .map_err(|error| format!("Cannot create runtime extraction folder: {error}"))?;
    if !server_ready {
        let staged_server = payload.join("llama.cpp");
        fs::create_dir_all(&staged_server)
            .map_err(|error| format!("Cannot create staged llama.cpp folder: {error}"))?;
        for asset in &server_assets {
            extract_archive(
                &downloads.join(&asset.name),
                &staged_server,
                asset.archive_format,
            )?;
        }
        let staged_executable = staged_server.join(&executable_name);
        if !staged_executable.is_file() {
            return Err(format!(
                "The selected llama.cpp release did not contain {}.",
                executable_name
            ));
        }
        make_executable(&staged_executable)?;
        validate_runtime_files(
            &staged_server,
            &selection.executable,
            &server_required_files,
            &server_required_prefixes,
        )?;
        activate_runtime_directory(&staged_server, &target)?;
    }
    if !cuda_ready {
        let asset = cuda_asset.ok_or_else(|| "CUDA runtime asset is missing".to_owned())?;
        let directory = cuda_directory
            .as_deref()
            .ok_or_else(|| "CUDA runtime directory is missing".to_owned())?;
        let staged_cuda = payload.join("cuda");
        fs::create_dir_all(&staged_cuda)
            .map_err(|error| format!("Cannot create staged CUDA folder: {error}"))?;
        extract_archive(
            &downloads.join(&asset.name),
            &staged_cuda,
            asset.archive_format,
        )?;
        validate_required_prefixes(&staged_cuda, &asset.required_file_prefixes)?;
        activate_runtime_directory(&staged_cuda, directory)?;
    }
    persist_llama_runtime_marker(
        &layout,
        selection.backend,
        cuda_asset,
        cuda_directory.as_deref(),
        selection.fallback_reason.as_deref(),
    )?;
    let _ = fs::remove_dir_all(&staging);
    crate::backend::BackendManager::persist_llama_server_path(&project_root, &executable)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("Cannot inspect {}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Cannot mark {} executable: {error}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn prune_obsolete_runtime_staging(runtime_root: &Path, current: &Path) -> Result<(), String> {
    prune_named_runtime_staging(runtime_root, current, ".llama.cpp-")
}

fn prune_named_runtime_staging(
    runtime_root: &Path,
    current: &Path,
    prefix: &str,
) -> Result<(), String> {
    let entries = match fs::read_dir(runtime_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot inspect runtime staging folders: {error}")),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Cannot inspect runtime staging entry: {error}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path != current
            && entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            && name.starts_with(prefix)
            && name.ends_with("-staging")
        {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "Cannot remove obsolete runtime staging {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn extract_archive(
    archive: &Path,
    destination: &Path,
    format: LlamaCppArchiveFormat,
) -> Result<(), String> {
    match format {
        LlamaCppArchiveFormat::Zip => extract_zip(archive, destination),
        LlamaCppArchiveFormat::TarGz => extract_tar_gz(archive, destination),
    }
}

fn extract_declared_files(
    archive: &Path,
    format: LlamaCppArchiveFormat,
    archive_directory: &Path,
    files: &[String],
    destination: &Path,
) -> Result<(), String> {
    match format {
        LlamaCppArchiveFormat::Zip => {
            let input = fs::File::open(archive)
                .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
            let mut zip = zip::ZipArchive::new(input)
                .map_err(|error| format!("Invalid archive {}: {error}", archive.display()))?;
            for file in files {
                let source = archive_directory.join(file);
                let source = source.to_string_lossy().replace('\\', "/");
                let mut entry = zip.by_name(&source).map_err(|error| {
                    format!("Archive {} is missing {source}: {error}", archive.display())
                })?;
                let output = destination.join(file);
                let mut target = fs::File::create(&output)
                    .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
                std::io::copy(&mut entry, &mut target)
                    .map_err(|error| format!("Cannot extract {}: {error}", output.display()))?;
            }
        }
        LlamaCppArchiveFormat::TarGz => {
            let input = fs::File::open(archive)
                .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
            let decoder = flate2::read::GzDecoder::new(input);
            let mut tar = tar::Archive::new(decoder);
            let expected = files
                .iter()
                .map(|file| (archive_directory.join(file), file))
                .collect::<Vec<_>>();
            let mut found = HashSet::new();
            for entry in tar
                .entries()
                .map_err(|error| format!("Invalid tar.gz archive: {error}"))?
            {
                let mut entry = entry.map_err(|error| format!("Cannot read tar entry: {error}"))?;
                let path = entry
                    .path()
                    .map_err(|error| format!("Cannot read tar entry path: {error}"))?;
                let Some((_, file)) = expected.iter().find(|(expected, _)| path == *expected)
                else {
                    continue;
                };
                let output = destination.join(file);
                let mut target = fs::File::create(&output)
                    .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
                std::io::copy(&mut entry, &mut target)
                    .map_err(|error| format!("Cannot extract {}: {error}", output.display()))?;
                found.insert((*file).clone());
            }
            if let Some(missing) = files.iter().find(|file| !found.contains(*file)) {
                return Err(format!(
                    "Archive {} is missing {}",
                    archive.display(),
                    archive_directory.join(missing).display()
                ));
            }
        }
    }
    Ok(())
}

fn safe_archive_path(destination: &Path, name: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if name.is_absolute()
        || name.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(format!(
            "archive entry escapes extraction directory: {}",
            name.display()
        ));
    }
    Ok(destination.join(name))
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive)
        .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("Invalid archive {}: {error}", archive.display()))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("Cannot read archive entry: {error}"))?;
        let name = entry.enclosed_name().ok_or_else(|| {
            format!(
                "archive entry escapes extraction directory: {}",
                entry.name()
            )
        })?;
        let output = safe_archive_path(destination, &name)?;
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::File::create(&output)
            .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Cannot extract {}: {error}", output.display()))?;
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive)
        .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .map_err(|error| format!("Invalid tar.gz archive: {error}"))?
    {
        let mut entry = entry.map_err(|error| format!("Cannot read tar entry: {error}"))?;
        let name = entry
            .path()
            .map_err(|error| format!("Cannot read tar entry path: {error}"))?
            .into_owned();
        let output = safe_archive_path(destination, &name)?;
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(format!("unsupported tar entry type: {}", name.display()));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::File::create(&output)
            .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Cannot extract {}: {error}", output.display()))?;
        #[cfg(unix)]
        if let Ok(mode) = entry.header().mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(mode)).map_err(|error| {
                format!(
                    "Cannot restore permissions for {}: {error}",
                    output.display()
                )
            })?;
        }
    }
    Ok(())
}

fn activate_runtime_directory(staged: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create runtime directory: {error}"))?;
    }
    if !target.exists() {
        return fs::rename(staged, target).map_err(|error| {
            format!(
                "Cannot atomically activate runtime {}: {error}",
                target.display()
            )
        });
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid runtime target: {}", target.display()))?;
    let backup = target.with_file_name(format!(".{name}.replaced-{}", std::process::id()));
    if backup.exists() {
        return Err(format!(
            "Cannot repair runtime while backup exists: {}",
            backup.display()
        ));
    }
    fs::rename(target, &backup).map_err(|error| {
        format!(
            "Cannot stage existing runtime {}: {error}",
            target.display()
        )
    })?;
    if let Err(error) = fs::rename(staged, target) {
        let _ = fs::rename(&backup, target);
        return Err(format!(
            "Cannot atomically activate repaired runtime {}: {error}",
            target.display()
        ));
    }
    fs::remove_dir_all(&backup).map_err(|error| {
        format!(
            "Repaired runtime activated, but old backup {} could not be removed: {error}",
            backup.display()
        )
    })
}

fn validate_required_files(directory: &Path, files: &[String]) -> Result<(), String> {
    resolve_required_files(directory, files).map(|_| ())
}

fn resolve_required_files(directory: &Path, files: &[String]) -> Result<Vec<PathBuf>, String> {
    files
        .iter()
        .map(|file| {
            let path = directory.join(file);
            path.is_file()
                .then_some(path)
                .ok_or_else(|| format!("runtime is missing required file: {file}"))
        })
        .collect()
}

fn validate_required_prefixes(directory: &Path, prefixes: &[String]) -> Result<(), String> {
    resolve_required_prefixes(directory, prefixes).map(|_| ())
}

fn resolve_required_prefixes(
    directory: &Path,
    prefixes: &[String],
) -> Result<Vec<PathBuf>, String> {
    prefixes
        .iter()
        .map(|prefix| {
            let mut matches = fs::read_dir(directory)
                .map_err(|error| format!("Cannot inspect {}: {error}", directory.display()))?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            matches.sort();
            match matches.as_slice() {
                [path] => Ok(path.clone()),
                [] => Err(format!(
                    "runtime is missing a required file with prefix: {prefix}"
                )),
                _ => Err(format!(
                    "runtime contains multiple files with required prefix {prefix}; the preload order would be ambiguous"
                )),
            }
        })
        .collect()
}

fn persist_native_runtime_selection(
    layout: &RuntimeLayout,
    selection: &NativeRuntimeSelection,
) -> Result<(), String> {
    let path = layout.native_runtime_selection_file();
    let parent = path
        .parent()
        .ok_or_else(|| "native runtime marker has no parent directory".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create native runtime directory: {error}"))?;
    let temporary = parent.join(format!(".native-runtime.json.tmp-{}", std::process::id()));
    let mut bytes = serde_json::to_vec_pretty(selection)
        .map_err(|error| format!("Cannot serialize native runtime marker: {error}"))?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)
        .map_err(|error| format!("Cannot write native runtime marker: {error}"))?;
    atomic_replace_file(&temporary, &path)?;
    Ok(())
}

fn load_native_runtime_selection(
    layout: &RuntimeLayout,
) -> Result<Option<NativeRuntimeSelection>, String> {
    let path = layout.native_runtime_selection_file();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Cannot read native runtime marker {}: {error}",
                path.display()
            ));
        }
    };
    let marker: NativeRuntimeSelection = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid native runtime marker {}: {error}", path.display()))?;
    if marker.schema_version != 1 {
        return Err(format!(
            "Unsupported native runtime marker schema {} in {}",
            marker.schema_version,
            path.display()
        ));
    }
    Ok(Some(marker))
}

fn persist_llama_runtime_marker(
    layout: &RuntimeLayout,
    backend: RuntimeBackend,
    cuda_asset: Option<&ReleaseAsset>,
    cuda_directory: Option<&Path>,
    fallback_reason: Option<&str>,
) -> Result<(), String> {
    let existing = load_native_runtime_selection(layout)?;
    let llama_backend = match backend {
        RuntimeBackend::Cpu => NativeRuntimeBackend::Cpu,
        RuntimeBackend::Cuda => NativeRuntimeBackend::Cuda,
    };
    let marker = NativeRuntimeSelection {
        schema_version: 1,
        backend: existing
            .as_ref()
            .and_then(|marker| marker.onnx_backend)
            .unwrap_or(llama_backend),
        llama_cpp_backend: Some(llama_backend),
        onnx_backend: existing.as_ref().and_then(|marker| marker.onnx_backend),
        cuda_version: cuda_asset
            .and_then(|asset| asset.cuda_version.clone())
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|marker| marker.cuda_version.clone())
            }),
        provider_dir: existing
            .as_ref()
            .and_then(|marker| marker.provider_dir.clone()),
        onnx_core_library: existing
            .as_ref()
            .and_then(|marker| marker.onnx_core_library.clone()),
        cuda_bin_dir: cuda_directory
            .map(|path| layout.config_path_for(path))
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|marker| marker.cuda_bin_dir.clone())
            }),
        cudnn_bin_dir: existing
            .as_ref()
            .and_then(|marker| marker.cudnn_bin_dir.clone()),
        preload_libraries: existing
            .as_ref()
            .map(|marker| marker.preload_libraries.clone())
            .unwrap_or_default(),
        fallback_reason: fallback_reason.map(str::to_owned).or_else(|| {
            existing
                .as_ref()
                .and_then(|marker| marker.fallback_reason.clone())
        }),
    };
    persist_native_runtime_selection(layout, &marker)
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(format!(
            "Cannot atomically replace native runtime marker: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("Cannot atomically replace native runtime marker: {error}"))
}

fn load_app_config(project_root: &Path) -> Result<AppConfig, String> {
    let path = project_root.join("config.json");
    AppConfig::from_path_with_user_config(&path, project_root).map_err(|error| {
        format!(
            "Cannot read native runtime configuration from {}: {error}",
            path.display()
        )
    })
}

/// Removes only resumable runtime staging for the releases declared by the
/// active configuration. Final runtime directories are never touched.
fn clear_runtime_staging(project_root: &Path) -> Result<(), String> {
    let config = load_app_config(project_root)?;
    let layout = load_runtime_layout(project_root);
    let paths = [
        layout.runtime_root().join(format!(
            ".llama.cpp-{}-staging",
            config.model_manager.llama_cpp.release
        )),
        layout.runtime_root().join(format!(
            ".onnxruntime-{}-staging",
            config.model_manager.onnxruntime.release
        )),
    ];
    for path in paths {
        match fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Cannot clear runtime staging {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// Filesystem-backed startup preflight. This avoids treating the installer's
/// empty, not-yet-planned UI state as proof that the runtime is missing.
pub fn configured_runtime_is_ready(
    project_root: &Path,
    requirements: RuntimeRequirements,
) -> Result<bool, String> {
    configured_runtime_plan(project_root, requirements).map(|plan| plan.is_ready())
}

fn configured_runtime_plan(
    project_root: &Path,
    requirements: RuntimeRequirements,
) -> Result<RuntimePlan, String> {
    let config = load_app_config(project_root)?;
    let nvidia = if cfg!(target_os = "windows") {
        supported_nvidia_cuda()?
    } else {
        None
    };
    let local_models = local_model_availability(nvidia.as_ref());
    let requires_managed_model = requirements.llama_cpp || requirements.onnx_tts;
    let blocking_error = if requires_managed_model {
        match &local_models {
            LocalModelAvailability::Available { .. } => None,
            LocalModelAvailability::Unavailable(reason) => Some(reason.clone()),
            LocalModelAvailability::Detecting => {
                Some("NVIDIA GPU detection did not complete.".to_owned())
            }
        }
    } else {
        None
    };
    let eligible_nvidia = blocking_error
        .is_none()
        .then_some(nvidia.as_ref())
        .flatten();
    // ONNX CUDA deliberately reuses the declared llama.cpp CUDA redistributable
    // catalogue. Small bundled ONNX components do not participate in this plan.
    let llama_assets = (requirements.llama_cpp || requirements.onnx_cuda)
        .then(|| release_assets_from_config(&config.model_manager.llama_cpp))
        .transpose()?
        .unwrap_or_default();
    let llama_cpp = (requirements.llama_cpp && blocking_error.is_none())
        .then(|| select_assets_for_hardware(&llama_assets, eligible_nvidia))
        .transpose()?;
    let onnx = if requirements.onnx_tts && requirements.onnx_cuda && blocking_error.is_none() {
        let providers = onnx_assets_from_config(&config.model_manager.onnxruntime)?;
        let cudnn_runtimes = cudnn_assets_from_config(&config.model_manager.onnxruntime)?;
        let cuda_runtimes = llama_assets
            .iter()
            .filter(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime)
            .cloned()
            .collect::<Vec<_>>();
        Some(select_onnx_assets_for_hardware(
            &providers,
            &cuda_runtimes,
            &cudnn_runtimes,
            eligible_nvidia,
        )?)
    } else if requirements.onnx_tts {
        None
    } else {
        None
    };
    let download_bytes = missing_runtime_bytes(project_root, llama_cpp.as_ref(), onnx.as_ref());
    let marker_ready = runtime_marker_matches_plan(project_root, llama_cpp.as_ref(), onnx.as_ref());
    Ok(RuntimePlan {
        llama_cpp,
        onnx,
        download_bytes,
        marker_ready,
        requirements,
        local_models,
        blocking_error,
    })
}

/// Verifies that the immutable files selected by a runtime plan are also
/// represented by the exact marker the backend will consume. File presence
/// alone is insufficient: without this binding the backend cannot safely know
/// which CUDA/ONNX/cuDNN closure it is allowed to load.
fn runtime_marker_matches_plan(
    project_root: &Path,
    llama: Option<&RuntimeSelection>,
    onnx: Option<&OnnxRuntimeSelection>,
) -> bool {
    if llama.is_none() && onnx.is_none() {
        return true;
    }
    let layout = load_runtime_layout(project_root);
    let Ok(Some(marker)) = load_native_runtime_selection(&layout) else {
        return false;
    };
    let resolved = layout.resolve_native_runtime_selection(&marker);

    if let Some(selection) = llama {
        let expected_backend = match selection.backend {
            RuntimeBackend::Cpu => NativeRuntimeBackend::Cpu,
            RuntimeBackend::Cuda => NativeRuntimeBackend::Cuda,
        };
        if marker.llama_cpp_backend != Some(expected_backend) {
            return false;
        }
        if let Some(cuda_asset) = selection
            .assets
            .iter()
            .find(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime)
        {
            let Some(version) = cuda_asset.cuda_version.as_deref() else {
                return false;
            };
            if marker.cuda_version.as_deref() != Some(version)
                || resolved.cuda_bin_dir.as_deref()
                    != Some(layout.cuda_runtime_directory(version).as_path())
            {
                return false;
            }
        }
    }

    if let Some(selection) = onnx {
        let expected_backend = match selection.backend {
            RuntimeBackend::Cpu => NativeRuntimeBackend::Cpu,
            RuntimeBackend::Cuda => NativeRuntimeBackend::Cuda,
        };
        if marker.backend != expected_backend || marker.onnx_backend != Some(expected_backend) {
            return false;
        }
        if selection.backend == RuntimeBackend::Cuda {
            let (Some(version), Some(provider), Some(cuda), Some(cudnn)) = (
                selection.cuda_version.as_deref(),
                selection.provider.as_ref(),
                selection.cuda_runtime.as_ref(),
                selection.cudnn.as_ref(),
            ) else {
                return false;
            };
            let provider_directory = layout.onnx_runtime_directory(&provider.cuda_version);
            let cuda_directory = layout.cuda_runtime_directory(version);
            let cudnn_directory = layout.cudnn_runtime_directory(&cudnn.cuda_version);
            if marker.cuda_version.as_deref() != Some(version)
                || resolved.provider_dir.as_deref() != Some(provider_directory.as_path())
                || resolved.onnx_core_library.as_deref()
                    != Some(
                        provider_directory
                            .join(RuntimeLayout::ONNX_CORE_LIBRARY)
                            .as_path(),
                    )
                || resolved.cuda_bin_dir.as_deref() != Some(cuda_directory.as_path())
                || resolved.cudnn_bin_dir.as_deref() != Some(cudnn_directory.as_path())
            {
                return false;
            }
            let Ok(mut expected_preloads) =
                resolve_required_prefixes(&cuda_directory, &cuda.required_file_prefixes)
            else {
                return false;
            };
            let Ok(cudnn_preloads) =
                resolve_required_files(&cudnn_directory, &cudnn.required_files)
            else {
                return false;
            };
            expected_preloads.extend(cudnn_preloads);
            if resolved.preload_libraries != expected_preloads {
                return false;
            }
        }
    }
    true
}

fn local_model_availability(nvidia: Option<&NvidiaCuda>) -> LocalModelAvailability {
    let Some(nvidia) = nvidia else {
        return LocalModelAvailability::Unavailable(
            "Managed local models require an NVIDIA GPU with at least 8 GiB of VRAM. Small bundled ONNX components remain available.".to_owned(),
        );
    };
    if nvidia.compute_capability < MIN_CUDA_COMPUTE_CAPABILITY {
        return LocalModelAvailability::Unavailable(format!(
            "NVIDIA GPU {} has compute capability {}, below the required {}.",
            nvidia.gpu,
            format_version(nvidia.compute_capability),
            format_version(MIN_CUDA_COMPUTE_CAPABILITY),
        ));
    }
    if nvidia.memory_bytes < MIN_LOCAL_MODEL_VRAM_BYTES {
        return LocalModelAvailability::Unavailable(format!(
            "NVIDIA GPU {} has {:.1} GiB of VRAM; managed local models require at least 8 GiB.",
            nvidia.gpu,
            nvidia.memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        ));
    }
    LocalModelAvailability::Available {
        gpu: nvidia.gpu.clone(),
        memory_bytes: nvidia.memory_bytes,
    }
}

fn missing_runtime_bytes(
    project_root: &Path,
    llama: Option<&RuntimeSelection>,
    onnx: Option<&OnnxRuntimeSelection>,
) -> u64 {
    let layout = load_runtime_layout(project_root);
    let mut missing = HashSet::new();
    let mut total = 0_u64;
    let mut add = |name: &str, size: u64| {
        if missing.insert(name.to_owned()) {
            total = total.saturating_add(size);
        }
    };
    if let Some(selection) = llama {
        let server_files = selection
            .assets
            .iter()
            .filter(|asset| asset.kind != LlamaCppAssetKind::CudaRuntime)
            .flat_map(|asset| asset.required_files.iter().cloned())
            .collect::<Vec<_>>();
        let server_prefixes = selection
            .assets
            .iter()
            .filter(|asset| asset.kind != LlamaCppAssetKind::CudaRuntime)
            .flat_map(|asset| asset.required_file_prefixes.iter().cloned())
            .collect::<Vec<_>>();
        let server_ready = validate_runtime_files(
            &layout.llama_cpp_directory(),
            &selection.executable,
            &server_files,
            &server_prefixes,
        )
        .is_ok();
        for asset in &selection.assets {
            let ready = if asset.kind == LlamaCppAssetKind::CudaRuntime {
                asset.cuda_version.as_deref().is_some_and(|version| {
                    validate_required_prefixes(
                        &layout.cuda_runtime_directory(version),
                        &asset.required_file_prefixes,
                    )
                    .is_ok()
                })
            } else {
                server_ready
            };
            if !ready {
                add(&asset.name, asset.size);
            }
        }
    }
    if let Some(selection) = onnx {
        if let Some(asset) = &selection.cuda_runtime {
            let ready = selection.cuda_version.as_deref().is_some_and(|version| {
                validate_required_prefixes(
                    &layout.cuda_runtime_directory(version),
                    &asset.required_file_prefixes,
                )
                .is_ok()
            });
            if !ready {
                add(&asset.name, asset.size);
            }
        }
        if let Some(asset) = &selection.cudnn {
            let ready = validate_required_files(
                &layout.cudnn_runtime_directory(&asset.cuda_version),
                &asset.required_files,
            )
            .is_ok();
            if !ready {
                add(&asset.name, asset.size);
            }
        }
        if let Some(asset) = &selection.provider {
            let ready = validate_required_files(
                &layout.onnx_runtime_directory(&asset.cuda_version),
                &asset.required_files,
            )
            .is_ok();
            if !ready {
                add(&asset.name, asset.size);
            }
        }
    }
    total
}

fn load_runtime_layout(project_root: &Path) -> RuntimeLayout {
    let path = project_root.join("config.json");
    let layout = AppConfig::from_path_with_user_config(&path, project_root)
        .map(|config| config.runtime_layout(project_root))
        .unwrap_or_else(|_| RuntimeLayout::for_project_root(project_root));
    migrate_legacy_runtime_layout(&layout);
    layout
}

/// Discovers and automatically organizes legacy unshared runtime directories into
/// the unified `RuntimeLayout` structure (`llama.cpp/`, `cuda/<version>/`, `onnxruntime/`).
///
/// In older versions, CUDA DLLs (`cublas64_*.dll`, `cudart64_*.dll`, `cublasLt64_*.dll`)
/// were extracted directly into `llama.cpp/`. This migration identifies the exact CUDA
/// version (e.g. 13.3 or 12.4) by inspecting the filename suffix, and copies them to
/// `<runtime>/cuda/<version>/` so both llama.cpp and ONNX Runtime can share them
/// without requiring any re-downloading.
pub(crate) fn migrate_legacy_runtime_layout(layout: &RuntimeLayout) {
    migrate_legacy_directory(layout, &layout.llama_cpp_directory());
    let onnx_root = layout.runtime_root().join("onnxruntime");
    if onnx_root.is_dir() {
        migrate_legacy_directory(layout, &onnx_root);
    }
}

pub(crate) fn migrate_legacy_directory(layout: &RuntimeLayout, source_dir: &Path) {
    if !source_dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(source_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = file_name.to_ascii_lowercase();

        // 1. CUDA runtime shared DLL migration with version preservation
        if lower.starts_with("cudart64_")
            || lower.starts_with("cublas64_")
            || lower.starts_with("cublaslt64_")
        {
            let cuda_version = if lower.contains("_13") {
                Some("13.3")
            } else if lower.contains("_12") {
                Some("12.4")
            } else if lower.contains("_11") {
                Some("11.8")
            } else {
                None
            };
            if let Some(version) = cuda_version {
                let target_dir = layout.cuda_runtime_directory(version);
                let target_file = target_dir.join(file_name);
                if !target_file.exists() {
                    if let Ok(()) = fs::create_dir_all(&target_dir) {
                        let _ = fs::copy(&path, &target_file);
                    }
                }
            }
        }

        // 2. ONNX runtime DLL migration if found in legacy mixed folder
        if lower.starts_with("onnxruntime") && lower.ends_with(".dll") {
            let cuda_version = if layout.cuda_runtime_directory("13.3").is_dir()
                || lower.contains("cuda13")
            {
                "13"
            } else if layout.cuda_runtime_directory("12.4").is_dir() || lower.contains("cuda12") {
                "12"
            } else {
                ""
            };
            let target_dir = if !cuda_version.is_empty() {
                layout.onnx_runtime_directory(cuda_version)
            } else {
                layout.onnx_cpu_runtime_directory()
            };
            let target_file = target_dir.join(file_name);
            if !target_file.exists() {
                if let Ok(()) = fs::create_dir_all(&target_dir) {
                    let _ = fs::copy(&path, &target_file);
                }
            }
        }
    }
}

fn load_runtime_config(project_root: &Path) -> Result<LlamaCppRuntimeConfig, String> {
    let path = project_root.join("config.json");
    AppConfig::from_path_with_user_config(&path, project_root)
        .map(|config| config.model_manager.llama_cpp)
        .map_err(|error| {
            format!(
                "Cannot read llama.cpp download configuration from {}: {error}",
                path.display()
            )
        })
}

fn load_onnx_runtime_config(project_root: &Path) -> Result<OnnxRuntimeConfig, String> {
    let path = project_root.join("config.json");
    AppConfig::from_path_with_user_config(&path, project_root)
        .map(|config| config.model_manager.onnxruntime)
        .map_err(|error| {
            format!(
                "Cannot read ONNX Runtime download configuration from {}: {error}",
                path.display()
            )
        })
}

fn onnx_assets_from_config(config: &OnnxRuntimeConfig) -> Result<Vec<ManagedRuntimeAsset>, String> {
    if config.release.trim().is_empty() {
        return Err("model_manager.onnxruntime.release is empty in config.json.".into());
    }
    if config.downloads.is_empty() {
        return Err("model_manager.onnxruntime.downloads is empty in config.json.".into());
    }
    managed_runtime_assets_from_config("model_manager.onnxruntime.downloads", &config.downloads)
}

fn cudnn_assets_from_config(
    config: &OnnxRuntimeConfig,
) -> Result<Vec<ManagedRuntimeAsset>, String> {
    if config.cudnn_downloads.is_empty() {
        return Err("model_manager.onnxruntime.cudnn_downloads is empty in config.json.".into());
    }
    managed_runtime_assets_from_config(
        "model_manager.onnxruntime.cudnn_downloads",
        &config.cudnn_downloads,
    )
}

fn managed_runtime_assets_from_config(
    config_path: &str,
    downloads: &[ManagedRuntimeArchive],
) -> Result<Vec<ManagedRuntimeAsset>, String> {
    let mut names = HashSet::new();
    downloads
        .iter()
        .map(|download| {
            let name = download.name.trim();
            let url = download.url.trim();
            if name.is_empty()
                || (download.archive_format == LlamaCppArchiveFormat::Zip
                    && !name.ends_with(".zip"))
                || (download.archive_format == LlamaCppArchiveFormat::TarGz
                    && !name.ends_with(".tar.gz"))
            {
                return Err(format!(
                    "{config_path} contains an archive name incompatible with its declared format: {:?}.",
                    download.name
                ));
            }
            if !names.insert(name.to_owned()) {
                return Err(format!(
                    "{config_path} contains duplicate archive {name:?}."
                ));
            }
            if !url.starts_with("https://") || download.bytes == 0 {
                return Err(format!(
                    "{config_path}[{name}] must declare an HTTPS URL and non-zero byte size."
                ));
            }
            let sha256 = download.sha256.trim();
            if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!(
                    "{config_path}[{name}].sha256 must be a 64-character hexadecimal digest."
                ));
            }
            let cuda_version = download.cuda_version.trim();
            if cuda_version.parse::<u16>().is_err() {
                return Err(format!(
                    "{config_path}[{name}].cuda_version must be a CUDA major version."
                ));
            }
            if download.target.trim().is_empty()
                || download.archive_directory.trim().is_empty()
                || download.required_files.is_empty()
                || download.required_files.iter().any(|file| {
                    let path = Path::new(file);
                    file.trim().is_empty()
                        || path.is_absolute()
                        || path.components().count() != 1
                })
            {
                return Err(format!(
                    "{config_path}[{name}] has incomplete extraction metadata."
                ));
            }
            Ok(ManagedRuntimeAsset {
                name: name.into(),
                browser_download_url: url.into(),
                size: download.bytes,
                sha256: sha256.to_ascii_lowercase(),
                archive_format: download.archive_format,
                target: download.target.trim().into(),
                cuda_version: cuda_version.into(),
                archive_directory: download.archive_directory.trim().into(),
                required_files: download.required_files.clone(),
            })
        })
        .collect()
}

fn select_onnx_assets_for_hardware(
    providers: &[ManagedRuntimeAsset],
    cuda_runtimes: &[ReleaseAsset],
    cudnn_runtimes: &[ManagedRuntimeAsset],
    nvidia: Option<&NvidiaCuda>,
) -> Result<OnnxRuntimeSelection, String> {
    let Some(nvidia) = nvidia else {
        return Err(
            "Managed ONNX models require an eligible NVIDIA GPU; CPU fallback is disabled."
                .to_owned(),
        );
    };
    let driver = parse_version(&nvidia.driver_cuda).ok_or_else(|| {
        format!(
            "NVIDIA GPU {} reported an invalid CUDA version {:?}.",
            nvidia.gpu, nvidia.driver_cuda
        )
    })?;
    let minimum = minimum_cuda_for_compute_capability(nvidia.compute_capability);
    let target = current_runtime_target();
    let selected = cuda_runtimes
        .iter()
        .filter(|runtime| runtime.target == target)
        .filter_map(|runtime| {
            let runtime_version = parse_version(runtime.cuda_version.as_deref()?)?;
            if runtime_version > driver
                || runtime_version < minimum
                || !cuda_supports_compute_capability(runtime_version, nvidia.compute_capability)
            {
                return None;
            }
            let provider = providers.iter().find(|provider| {
                provider.target == target
                    && provider.cuda_version.parse::<u16>().ok() == Some(runtime_version.0)
            })?;
            let cudnn = cudnn_runtimes.iter().find(|cudnn| {
                cudnn.target == target
                    && cudnn.cuda_version.parse::<u16>().ok() == Some(runtime_version.0)
            })?;
            Some((
                runtime_version,
                runtime.clone(),
                provider.clone(),
                cudnn.clone(),
            ))
        })
        .max_by_key(|(version, _, _, _)| *version);
    let Some((cuda_version, cuda_runtime, provider, cudnn)) = selected else {
        return Err(format!(
            "NVIDIA GPU {} supports CUDA {}, but no complete ONNX Runtime, CUDA and cuDNN bundle is configured for {target}.",
            nvidia.gpu, nvidia.driver_cuda
        ));
    };
    Ok(OnnxRuntimeSelection {
        backend: RuntimeBackend::Cuda,
        provider: Some(provider),
        cuda_runtime: Some(cuda_runtime),
        cudnn: Some(cudnn),
        cuda_version: Some(format_version(cuda_version)),
        fallback_reason: None,
    })
}

fn release_assets_from_config(config: &LlamaCppRuntimeConfig) -> Result<Vec<ReleaseAsset>, String> {
    if config.release.trim().is_empty() {
        return Err("model_manager.llama_cpp.release is empty in config.json.".into());
    }
    if config.downloads.is_empty() {
        return Err("model_manager.llama_cpp.downloads is empty in config.json.".into());
    }

    let mut names = HashSet::new();
    config
        .downloads
        .iter()
        .map(|download| {
            let name = download.name.trim();
            let url = download.url.trim();
            if name.is_empty()
                || (download.archive_format == LlamaCppArchiveFormat::Zip
                    && !name.ends_with(".zip"))
                || (download.archive_format == LlamaCppArchiveFormat::TarGz
                    && !name.ends_with(".tar.gz"))
            {
                return Err(format!(
                    "model_manager.llama_cpp.downloads contains an archive name incompatible with its declared format: {:?}.",
                    download.name
                ));
            }
            if !names.insert(name.to_owned()) {
                return Err(format!(
                    "model_manager.llama_cpp.downloads contains duplicate archive {name:?}."
                ));
            }
            if !url.starts_with("https://") {
                return Err(format!(
                    "model_manager.llama_cpp.downloads[{name}] must use an HTTPS URL."
                ));
            }
            if download.bytes == 0 {
                return Err(format!(
                    "model_manager.llama_cpp.downloads[{name}].bytes must be greater than zero."
                ));
            }
            let sha256 = download.sha256.trim();
            if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!(
                    "model_manager.llama_cpp.downloads[{name}].sha256 must be a 64-character hexadecimal digest."
                ));
            }
            let target = if download.target.trim().is_empty() {
                legacy_target_from_name(name)
            } else {
                download.target.trim().to_owned()
            };
            let (kind, cuda_version, executable, required_files, required_file_prefixes) =
                normalize_runtime_metadata(download, name, &target)?;
            Ok(ReleaseAsset {
                name: name.into(),
                browser_download_url: url.into(),
                size: download.bytes,
                sha256: sha256.to_ascii_lowercase(),
                archive_format: download.archive_format,
                kind,
                target,
                cuda_version,
                executable,
                required_files,
                required_file_prefixes,
            })
        })
        .collect()
}

fn select_assets_for_hardware(
    assets: &[ReleaseAsset],
    nvidia: Option<&NvidiaCuda>,
) -> Result<RuntimeSelection, String> {
    let target = current_runtime_target();
    let assets: Vec<_> = assets
        .iter()
        .filter(|asset| asset.target == target)
        .cloned()
        .collect();
    if assets.is_empty() {
        return Err(format!(
            "no llama.cpp runtime assets are configured for {target}"
        ));
    }
    if let Some(nvidia) = nvidia {
        let supported = parse_version(&nvidia.driver_cuda).ok_or_else(|| {
            format!(
                "NVIDIA GPU {} reported an invalid CUDA version {:?}.",
                nvidia.gpu, nvidia.driver_cuda
            )
        })?;
        let minimum = minimum_cuda_for_compute_capability(nvidia.compute_capability);
        let Some(runtime) = best_cuda_asset(&assets, supported, minimum, nvidia.compute_capability)
        else {
            let reason = if nvidia.compute_capability.0 >= 10 {
                let package_cuda = assets
                    .iter()
                    .filter(|asset| asset.kind == LlamaCppAssetKind::ServerCuda)
                    .filter_map(|asset| asset.cuda_version.as_deref())
                    .filter_map(parse_version)
                    .filter(|version| {
                        *version >= minimum
                            && cuda_supports_compute_capability(*version, nvidia.compute_capability)
                            && assets.iter().any(|asset| {
                                asset.kind == LlamaCppAssetKind::CudaRuntime
                                    && asset.cuda_version.as_deref().and_then(parse_version)
                                        == Some(*version)
                            })
                    })
                    .min()
                    .map(format_version)
                    .unwrap_or_else(|| format_version(minimum));
                format!(
                    "This RTX 50-series GPU needs a CUDA {package_cuda}-capable NVIDIA driver to use the packaged llama.cpp runtime. The installed driver only reports CUDA {}; using the CPU runtime. Update the graphics driver with NVIDIA App: {NVIDIA_APP_URL}",
                    nvidia.driver_cuda,
                )
            } else {
                format!(
                    "NVIDIA GPU {} (compute capability {}) requires CUDA {} or newer, and the driver supports up to CUDA {}, but the configured llama.cpp download list has no compatible CUDA package for {}; using the CPU runtime.",
                    nvidia.gpu,
                    format_version(nvidia.compute_capability),
                    format_version(minimum),
                    nvidia.driver_cuda,
                    target
                )
            };
            return Err(reason.replace("; using the CPU runtime", ""));
        };
        let cuda_version = runtime
            .cuda_version
            .as_deref()
            .ok_or_else(|| "selected CUDA asset has no CUDA version".to_owned())?;
        let Some(cudart) = assets
            .iter()
            .find(|asset| {
                asset.kind == LlamaCppAssetKind::CudaRuntime
                    && asset.cuda_version.as_deref() == Some(cuda_version)
            })
            .cloned()
        else {
            return Err(format!(
                "The configured llama.cpp download list is missing the CUDA runtime package for version {cuda_version}."
            ));
        };
        let executable = runtime.executable.clone();
        let selected_version = parse_version(cuda_version)
            .ok_or_else(|| "selected CUDA asset has an invalid CUDA version".to_owned())?;
        let newer_version = assets
            .iter()
            .filter(|asset| asset.kind == LlamaCppAssetKind::ServerCuda)
            .filter_map(|asset| asset.cuda_version.as_deref())
            .filter_map(parse_version)
            .filter(|version| *version > selected_version)
            .max();
        let fallback_reason = (nvidia.compute_capability.0 >= 10)
            .then_some(newer_version)
            .flatten()
            .map(|newer| {
                format!(
                    "Using the compatible CUDA {cuda_version} llama.cpp runtime for this RTX 50-series GPU. Update the graphics driver with NVIDIA App to enable the newer CUDA {} runtime: {NVIDIA_APP_URL}",
                    format_version(newer),
                )
            });
        return Ok(RuntimeSelection {
            assets: vec![runtime, cudart],
            backend: RuntimeBackend::Cuda,
            executable,
            fallback_reason,
        });
    }
    Err(
        "Managed llama.cpp models require an eligible NVIDIA GPU; CPU fallback is disabled."
            .to_owned(),
    )
}

/// Converts persisted runtime metadata into the installer representation.
/// The filename checks here are intentionally limited to legacy entries that
/// predate the declarative fields; new entries never use vendor filenames.
fn normalize_runtime_metadata(
    download: &xrtranslate_config::LlamaCppDownload,
    name: &str,
    target: &str,
) -> Result<
    (
        LlamaCppAssetKind,
        Option<String>,
        String,
        Vec<String>,
        Vec<String>,
    ),
    String,
> {
    let legacy = download.target.trim().is_empty();
    let legacy_cuda = legacy && name.contains("-cuda-");
    let legacy_cudart = legacy && name.contains("cudart");
    let kind = if legacy_cudart {
        LlamaCppAssetKind::CudaRuntime
    } else if legacy_cuda {
        LlamaCppAssetKind::ServerCuda
    } else {
        download.kind
    };
    let cuda_version = download.cuda_version.clone().or_else(|| {
        legacy_cuda
            .then_some(name)
            .and_then(|name| name.split("-cuda-").nth(1))
            .and_then(|version| version.split('-').next())
            .map(str::to_owned)
    });
    let executable = if download.executable.trim().is_empty() {
        if !legacy && kind != LlamaCppAssetKind::CudaRuntime {
            return Err(format!(
                "model_manager.llama_cpp.downloads[{name}].executable must be declared for new-format server assets."
            ));
        } else if target.starts_with("windows-") {
            "llama-server.exe".into()
        } else {
            "llama-server".into()
        }
    } else {
        download.executable.trim().to_owned()
    };
    let migrate_windows_requirements =
        download.required_files.is_empty() && target.starts_with("windows-");
    let required_files = if migrate_windows_requirements {
        match kind {
            LlamaCppAssetKind::ServerCpu => vec!["ggml.dll".into()],
            LlamaCppAssetKind::ServerCuda => vec!["ggml.dll".into(), "ggml-cuda.dll".into()],
            LlamaCppAssetKind::CudaRuntime => Vec::new(),
        }
    } else {
        download.required_files.clone()
    };
    let required_file_prefixes = if download.required_file_prefixes.is_empty()
        && target.starts_with("windows-")
        && kind == LlamaCppAssetKind::CudaRuntime
    {
        vec!["cudart64_".into(), "cublas64_".into(), "cublasLt64_".into()]
    } else {
        download.required_file_prefixes.clone()
    };
    Ok((
        kind,
        cuda_version,
        executable,
        required_files,
        required_file_prefixes,
    ))
}

fn best_cuda_asset(
    assets: &[ReleaseAsset],
    supported: (u16, u16),
    minimum: (u16, u16),
    compute_capability: (u16, u16),
) -> Option<ReleaseAsset> {
    assets
        .iter()
        .filter_map(|asset| {
            if asset.kind != LlamaCppAssetKind::ServerCuda {
                return None;
            }
            let version = asset.cuda_version.as_deref()?;
            let version = parse_version(version)?;
            (version >= minimum
                && version <= supported
                && cuda_supports_compute_capability(version, compute_capability))
            .then_some((version, asset.clone()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, asset)| asset)
}

fn current_runtime_target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn legacy_target_from_name(name: &str) -> String {
    if name.contains("-win-") {
        "windows-x86_64".into()
    } else {
        current_runtime_target()
    }
}

fn parse_version(value: &str) -> Option<(u16, u16)> {
    let mut parts = value.split('.');
    let version = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(version)
}

fn cuda_supports_compute_capability(
    cuda_version: (u16, u16),
    compute_capability: (u16, u16),
) -> bool {
    cuda_version.0 < 13 || compute_capability >= TURING_COMPUTE_CAPABILITY
}

fn format_version(version: (u16, u16)) -> String {
    format!("{}.{}", version.0, version.1)
}

fn minimum_cuda_for_compute_capability(capability: (u16, u16)) -> (u16, u16) {
    if capability.0 >= 10 {
        BLACKWELL_MINIMUM_CUDA
    } else {
        (0, 0)
    }
}

fn supported_nvidia_cuda() -> Result<Option<NvidiaCuda>, String> {
    let Some((program, query)) = run_nvidia_smi(&[
        "--query-gpu=name,compute_cap,memory.total",
        "--format=csv,noheader,nounits",
    ])?
    else {
        return Ok(None);
    };
    if !query.status.success() {
        return Err(command_failure("Cannot query NVIDIA GPUs", &query));
    }
    let gpus = parse_nvidia_gpu_rows(&String::from_utf8_lossy(&query.stdout))?;
    // ORT and llama.cpp currently use CUDA's default device. Gate the same
    // primary device instead of approving the plan from a different adapter.
    let Some(mut selected) = gpus.into_iter().next() else {
        return Ok(None);
    };

    let version_output = crate::child_process::hide_console(&mut Command::new(&program))
        .output()
        .map_err(|error| format!("Cannot run {}: {error}", program.display()))?;
    if !version_output.status.success() {
        return Err(command_failure(
            "Cannot query the NVIDIA driver CUDA version",
            &version_output,
        ));
    }
    let version_text = String::from_utf8_lossy(&version_output.stdout);
    selected.driver_cuda = cuda_version_from_nvidia_smi(&version_text).ok_or_else(|| {
        "nvidia-smi did not report a parseable CUDA Version or CUDA UMD Version; refusing to silently install the CPU runtime on an NVIDIA system.".to_owned()
    })?;
    Ok(Some(selected))
}

fn parse_nvidia_gpu_rows(output: &str) -> Result<Vec<NvidiaCuda>, String> {
    let mut gpus = Vec::new();
    let mut invalid = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
        let [gpu, capability, memory_mib] = columns.as_slice() else {
            invalid.push(line.to_owned());
            continue;
        };
        let Some(compute_capability) = parse_version(capability) else {
            invalid.push(line.to_owned());
            continue;
        };
        let Ok(memory_mib) = memory_mib.parse::<u64>() else {
            invalid.push(line.to_owned());
            continue;
        };
        gpus.push(NvidiaCuda {
            gpu: (*gpu).to_owned(),
            compute_capability,
            driver_cuda: String::new(),
            memory_bytes: memory_mib.saturating_mul(1024 * 1024),
        });
    }
    if gpus.is_empty() {
        let detail = if invalid.is_empty() {
            "nvidia-smi returned no GPU rows".to_owned()
        } else {
            format!("unparseable rows: {}", invalid.join(" | "))
        };
        Err(format!("Cannot identify an NVIDIA GPU ({detail})."))
    } else {
        Ok(gpus)
    }
}

fn run_nvidia_smi(args: &[&str]) -> Result<Option<(PathBuf, std::process::Output)>, String> {
    let mut candidates = Vec::new();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        candidates.push(PathBuf::from(system_root).join("System32/nvidia-smi.exe"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates
            .push(PathBuf::from(program_files).join("NVIDIA Corporation/NVSMI/nvidia-smi.exe"));
    }
    candidates.push(PathBuf::from("nvidia-smi"));

    for program in candidates {
        match crate::child_process::hide_console(&mut Command::new(&program))
            .args(args)
            .output()
        {
            Ok(output) => return Ok(Some((program, output))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("Cannot run {}: {error}", program.display()));
            }
        }
    }

    if windows_reports_nvidia_adapter() {
        Err(format!(
            "Windows reports an NVIDIA display adapter, but nvidia-smi could not be found. Reinstall or update the NVIDIA driver with NVIDIA App ({NVIDIA_APP_URL}); the installer will not silently substitute a CPU runtime."
        ))
    } else {
        Ok(None)
    }
}

fn windows_reports_nvidia_adapter() -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    crate::child_process::hide_console(&mut Command::new("reg"))
        .args([
            "query",
            "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}",
            "/s",
            "/v",
            "DriverDesc",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .to_ascii_lowercase()
                .contains("nvidia")
        })
}

fn command_failure(context: &str, output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.is_empty() {
        format!("{context} (exit status {})", output.status)
    } else {
        format!("{context}: {detail}")
    }
}

fn cuda_version_from_nvidia_smi(version_text: &str) -> Option<String> {
    let version = ["CUDA Version: ", "CUDA UMD Version: "]
        .into_iter()
        .find_map(|marker| {
            let start = version_text.find(marker)? + marker.len();
            Some(
                version_text[start..]
                    .split_whitespace()
                    .next()?
                    .trim_end_matches('|'),
            )
        })?;
    parse_version(version)?;
    Some(version.to_owned())
}

fn validate_runtime_files(
    directory: &Path,
    executable: &str,
    required_files: &[String],
    required_file_prefixes: &[String],
) -> Result<(), String> {
    let executable_path = directory.join(executable);
    if !executable_path.is_file() {
        return Err(format!(
            "runtime executable is missing: {}",
            executable_path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if executable_path
            .metadata()
            .map_err(|error| error.to_string())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(format!(
                "runtime executable is not executable: {}",
                executable_path.display()
            ));
        }
    }
    for required in required_files {
        if !directory.join(required).is_file() {
            return Err(format!("runtime is missing required file: {required}"));
        }
    }
    for prefix in required_file_prefixes {
        if !directory_contains_file_prefix(directory, prefix)? {
            return Err(format!(
                "runtime is missing a required file with prefix: {prefix}"
            ));
        }
    }
    Ok(())
}

fn directory_contains_file_prefix(directory: &Path, prefix: &str) -> Result<bool, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Cannot inspect {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect an entry in {}: {error}",
                directory.display()
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_returns_the_installed_executable_to_host_coordination() {
        let executable = PathBuf::from("runtime/llama.cpp/llama-server.exe");
        let (sender, receiver) = unbounded();
        sender
            .send(Event::Finished(Ok(executable.clone())))
            .unwrap();
        let mut installer = RuntimeInstaller {
            events: Some(receiver),
            ..RuntimeInstaller::default()
        };

        assert_eq!(installer.poll(), Some(executable));
        assert!(matches!(installer.state(), RuntimeInstallState::Installed));
        assert!(installer.events.is_none());
    }

    fn asset(name: &str) -> ReleaseAsset {
        let is_cuda_runtime = name.contains("cudart");
        ReleaseAsset {
            name: name.into(),
            browser_download_url: "https://example.invalid/file.zip".into(),
            size: 1,
            sha256: "0".repeat(64),
            archive_format: LlamaCppArchiveFormat::Zip,
            kind: if is_cuda_runtime {
                LlamaCppAssetKind::CudaRuntime
            } else if name.contains("cuda") {
                LlamaCppAssetKind::ServerCuda
            } else {
                LlamaCppAssetKind::ServerCpu
            },
            target: current_runtime_target(),
            cuda_version: name
                .contains("cuda-12.4")
                .then(|| "12.4".into())
                .or_else(|| name.contains("cuda-13.1").then(|| "13.1".into()))
                .or_else(|| name.contains("cuda-13.3").then(|| "13.3".into())),
            executable: "llama-server.exe".into(),
            required_files: vec!["ggml.dll".into()],
            required_file_prefixes: if is_cuda_runtime {
                vec!["cudart64_".into(), "cublasLt64_".into(), "cublas64_".into()]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn automatic_installer_uses_the_configured_download_urls() {
        let config = AppConfig::from_json_str(include_str!("../../config.json")).unwrap();
        let assets = release_assets_from_config(&config.model_manager.llama_cpp).unwrap();
        assert_eq!(assets.len(), 8);
        assert_eq!(config.model_manager.llama_cpp.release, "b10333");
        assert!(
            !config
                .model_manager
                .llama_cpp
                .release_page
                .ends_with("/latest")
        );
        for asset in assets {
            assert!(!asset.browser_download_url.contains("api.github.com"));
            assert!(!asset.browser_download_url.contains("/latest"));
            assert!(asset.browser_download_url.ends_with(&asset.name));
            assert!(asset.size > 0);
            assert_eq!(asset.sha256.len(), 64);
        }
        let cuda_runtime = release_assets_from_config(&config.model_manager.llama_cpp)
            .unwrap()
            .into_iter()
            .find(|asset| {
                asset.cuda_version.as_deref() == Some("13.3")
                    && asset.kind == LlamaCppAssetKind::CudaRuntime
            })
            .unwrap();
        assert_eq!(
            cuda_runtime.required_file_prefixes,
            ["cudart64_", "cublasLt64_", "cublas64_"]
        );
        let blackwell_compatible = release_assets_from_config(&config.model_manager.llama_cpp)
            .unwrap()
            .into_iter()
            .filter(|asset| asset.cuda_version.as_deref() == Some("13.1"))
            .collect::<Vec<_>>();
        assert_eq!(blackwell_compatible.len(), 2);
        assert!(blackwell_compatible.iter().all(|asset| {
            asset
                .browser_download_url
                .contains("/releases/download/b8913/")
                && asset.sha256.len() == 64
                && asset.size > 0
        }));
        let server_13_1 = blackwell_compatible
            .iter()
            .find(|asset| asset.kind == LlamaCppAssetKind::ServerCuda)
            .unwrap();
        assert_eq!(server_13_1.size, 145_463_676);
        assert_eq!(
            server_13_1.sha256,
            "16cb6fb46efe3923833dc08eaeb7ab29c6251e29a11d9ae32581e226172e2af0"
        );
        let cudart_13_1 = blackwell_compatible
            .iter()
            .find(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime)
            .unwrap();
        assert_eq!(cudart_13_1.size, 402_582_216);
        assert_eq!(
            cudart_13_1.sha256,
            "f96935e7e385e3b2d0189239077c10fe8fd7e95690fea4afec455b1b6c7e3f18"
        );
        let linux = release_assets_from_config(&config.model_manager.llama_cpp)
            .unwrap()
            .into_iter()
            .find(|asset| asset.target == "linux-x86_64")
            .expect("verified Linux x86_64 runtime asset");
        assert_eq!(linux.archive_format, LlamaCppArchiveFormat::TarGz);
        assert_eq!(linux.executable, "llama-b10333/llama-server");
        assert_eq!(
            linux.sha256,
            "936ce04d98abe2a977e9dd2ff92659bb96947e136acee8f2bc3e21d8eaebbf23"
        );

        let onnx = onnx_assets_from_config(&config.model_manager.onnxruntime).unwrap();
        assert_eq!(onnx.len(), 2);
        assert!(onnx.iter().all(|asset| {
            asset.required_files
                == [
                    "onnxruntime.dll",
                    "onnxruntime_providers_shared.dll",
                    "onnxruntime_providers_cuda.dll",
                ]
        }));
        assert_eq!(onnx[0].cuda_version, "12");
        assert_eq!(onnx[0].size, 455_344_532);
        assert_eq!(
            onnx[0].sha256,
            "6b7bf16d6d30180db7f386fb179aa4e4f1313f0924531a2879b7b090b56518c1"
        );
        assert_eq!(onnx[1].cuda_version, "13");
        assert_eq!(onnx[1].size, 365_825_268);
        assert_eq!(
            onnx[1].sha256,
            "137f0822a4923b1d84d3e09496e0792ebbb221eb3a61a0657f71a12ab68ab1e2"
        );
        assert!(onnx.iter().all(|asset| {
            asset
                .browser_download_url
                .starts_with("https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/")
        }));
        let cudnn = cudnn_assets_from_config(&config.model_manager.onnxruntime).unwrap();
        assert_eq!(cudnn.len(), 2);
        assert_eq!(cudnn[0].cuda_version, "12");
        assert_eq!(cudnn[1].cuda_version, "13");
        assert_eq!(cudnn[1].size, 349_802_474);
        assert_eq!(
            cudnn[1].sha256,
            "d3ccce59130f10f68fe09365feea65b622bcecace79a0682fe43ee07b88a6a29"
        );
        assert!(cudnn.iter().all(|asset| {
            asset
                .browser_download_url
                .starts_with("https://developer.download.nvidia.com/compute/cudnn/redist/")
                && asset.required_files
                    == [
                        "cudnn64_9.dll",
                        "cudnn_graph64_9.dll",
                        "cudnn_ops64_9.dll",
                        "cudnn_heuristic64_9.dll",
                        "cudnn_engines_precompiled64_9.dll",
                        "cudnn_engines_runtime_compiled64_9.dll",
                        "cudnn_adv64_9.dll",
                        "cudnn_cnn64_9.dll",
                    ]
        }));
    }

    fn onnx_asset(cuda_version: &str) -> ManagedRuntimeAsset {
        ManagedRuntimeAsset {
            name: format!("onnxruntime-cuda-{cuda_version}.zip"),
            browser_download_url: "https://example.invalid/onnx.zip".into(),
            size: 1,
            sha256: "0".repeat(64),
            archive_format: LlamaCppArchiveFormat::Zip,
            target: current_runtime_target(),
            cuda_version: cuda_version.into(),
            archive_directory: "onnx/lib".into(),
            required_files: vec![
                "onnxruntime_providers_shared.dll".into(),
                "onnxruntime_providers_cuda.dll".into(),
            ],
        }
    }

    fn cudnn_asset(cuda_version: &str) -> ManagedRuntimeAsset {
        ManagedRuntimeAsset {
            name: format!("cudnn-cuda-{cuda_version}.zip"),
            browser_download_url: "https://example.invalid/cudnn.zip".into(),
            size: 1,
            sha256: "0".repeat(64),
            archive_format: LlamaCppArchiveFormat::Zip,
            target: current_runtime_target(),
            cuda_version: cuda_version.into(),
            archive_directory: "cudnn/bin".into(),
            required_files: vec!["cudnn64_9.dll".into()],
        }
    }

    #[test]
    fn onnx_and_llama_choose_the_same_cuda_major() {
        let cuda_runtimes = vec![
            asset("cudart-llama-bin-win-cuda-12.4-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.1-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.3-x64.zip"),
        ];
        let providers = vec![onnx_asset("12"), onnx_asset("13")];
        for (driver, expected) in [("12.9", "12.4"), ("13.2", "13.1"), ("13.3", "13.3")] {
            let selection = select_onnx_assets_for_hardware(
                &providers,
                &cuda_runtimes,
                &[cudnn_asset("12"), cudnn_asset("13")],
                Some(&NvidiaCuda {
                    gpu: "NVIDIA GeForce RTX 4090".into(),
                    compute_capability: (8, 9),
                    driver_cuda: driver.into(),
                    memory_bytes: 24 * 1024 * 1024 * 1024,
                }),
            )
            .unwrap();
            assert_eq!(selection.backend, RuntimeBackend::Cuda);
            assert_eq!(selection.cuda_version.as_deref(), Some(expected));
            assert_eq!(
                selection.provider.as_ref().unwrap().cuda_version,
                expected.split('.').next().unwrap()
            );
            assert_eq!(
                selection.cudnn.as_ref().unwrap().cuda_version,
                expected.split('.').next().unwrap()
            );
        }
    }

    #[test]
    fn onnx_rejects_missing_compatible_cuda_bundle() {
        let error = select_onnx_assets_for_hardware(
            &[onnx_asset("13")],
            &[asset("cudart-llama-bin-win-cuda-13.3-x64.zip")],
            &[cudnn_asset("13")],
            Some(&NvidiaCuda {
                gpu: "NVIDIA GeForce RTX 5080".into(),
                compute_capability: (12, 0),
                driver_cuda: "13.2".into(),
                memory_bytes: 16 * 1024 * 1024 * 1024,
            }),
        )
        .unwrap_err();
        assert!(error.contains("no complete ONNX Runtime"));
    }

    #[test]
    fn union_missing_size_counts_shared_cuda_archive_once() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-runtime-union-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let server = asset("llama-b1-bin-win-cuda-13.3-x64.zip");
        let cuda = asset("cudart-llama-bin-win-cuda-13.3-x64.zip");
        let llama = RuntimeSelection {
            assets: vec![server, cuda.clone()],
            backend: RuntimeBackend::Cuda,
            executable: "llama-server.exe".into(),
            fallback_reason: None,
        };
        let onnx = OnnxRuntimeSelection {
            backend: RuntimeBackend::Cuda,
            provider: Some(onnx_asset("13")),
            cuda_runtime: Some(cuda),
            cudnn: Some(cudnn_asset("13")),
            cuda_version: Some("13.3".into()),
            fallback_reason: None,
        };
        assert_eq!(missing_runtime_bytes(&root, Some(&llama), Some(&onnx)), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_onnx_files_without_a_marker_are_repaired_automatically() {
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-runtime-marker-repair-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.json"), include_str!("../../config.json")).unwrap();
        let layout = RuntimeLayout::for_project_root(&root);
        let cuda = asset("cudart-llama-bin-win-cuda-13.1-x64.zip");
        let mut provider = onnx_asset("13");
        provider.required_files.push("onnxruntime.dll".into());
        let cudnn = cudnn_asset("13");
        let selection = OnnxRuntimeSelection {
            backend: RuntimeBackend::Cuda,
            provider: Some(provider.clone()),
            cuda_runtime: Some(cuda.clone()),
            cudnn: Some(cudnn.clone()),
            cuda_version: Some("13.1".into()),
            fallback_reason: None,
        };
        let cuda_directory = layout.cuda_runtime_directory("13.1");
        let provider_directory = layout.onnx_runtime_directory("13");
        let cudnn_directory = layout.cudnn_runtime_directory("13");
        for (directory, files) in [
            (&cuda_directory, &cuda.required_file_prefixes),
            (&provider_directory, &provider.required_files),
            (&cudnn_directory, &cudnn.required_files),
        ] {
            fs::create_dir_all(directory).unwrap();
            for file in files {
                let filename = if file.ends_with('_') {
                    format!("{file}13.dll")
                } else {
                    file.clone()
                };
                fs::write(directory.join(filename), b"runtime").unwrap();
            }
        }

        assert_eq!(missing_runtime_bytes(&root, None, Some(&selection)), 0);
        assert!(!runtime_marker_matches_plan(&root, None, Some(&selection)));

        let plan = RuntimePlan {
            llama_cpp: None,
            onnx: Some(selection.clone()),
            download_bytes: 0,
            marker_ready: false,
            requirements: RuntimeRequirements {
                onnx_tts: true,
                onnx_cuda: true,
                ..RuntimeRequirements::default()
            },
            local_models: LocalModelAvailability::Available {
                gpu: "test GPU".into(),
                memory_bytes: 16 * 1024 * 1024 * 1024,
            },
            blocking_error: None,
        };
        assert!(plan.requires_marker_repair());
        let (sender, receiver) = unbounded();
        sender.send(Event::Prepared(Ok(plan))).unwrap();
        let mut installer = RuntimeInstaller {
            state: RuntimeInstallState::Detecting,
            events: Some(receiver),
            active_project_root: Some(root.clone()),
            ..RuntimeInstaller::default()
        };
        installer.poll();
        for _ in 0..100 {
            installer.poll();
            if layout.native_runtime_selection_file().is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(layout.native_runtime_selection_file().is_file());
        assert!(runtime_marker_matches_plan(&root, None, Some(&selection)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn onnx_extraction_keeps_only_declared_runtime_dlls() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("xrtranslate-onnx-extract-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("runtime.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for name in [
            "onnx/lib/onnxruntime.dll",
            "onnx/lib/onnxruntime_providers_shared.dll",
            "onnx/lib/onnxruntime_providers_cuda.dll",
            "onnx/lib/onnxruntime_providers_cuda.pdb",
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(name.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let output = root.join("output");
        fs::create_dir_all(&output).unwrap();
        let files = vec![
            "onnxruntime.dll".into(),
            "onnxruntime_providers_shared.dll".into(),
            "onnxruntime_providers_cuda.dll".into(),
        ];
        extract_declared_files(
            &archive,
            LlamaCppArchiveFormat::Zip,
            Path::new("onnx/lib"),
            &files,
            &output,
        )
        .unwrap();
        assert!(output.join(&files[0]).is_file());
        assert!(output.join(&files[1]).is_file());
        assert!(output.join(&files[2]).is_file());
        assert!(!output.join("onnxruntime_providers_cuda.pdb").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cuda_preload_order_is_cudart_then_cublas_lt_then_cublas() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-cuda-preload-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for file in ["cublas64_13.dll", "cudart64_13.dll", "cublasLt64_13.dll"] {
            fs::write(root.join(file), b"runtime").unwrap();
        }
        let ordered = resolve_required_prefixes(
            &root,
            &["cudart64_".into(), "cublasLt64_".into(), "cublas64_".into()],
        )
        .unwrap();
        assert_eq!(
            ordered
                .iter()
                .filter_map(|path| path.file_name())
                .collect::<Vec<_>>(),
            ["cudart64_13.dll", "cublasLt64_13.dll", "cublas64_13.dll",]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn onnx_cpu_marker_preserves_llama_cuda_search_directory() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-runtime-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let layout = RuntimeLayout::for_project_root(&root);
        persist_native_runtime_selection(
            &layout,
            &NativeRuntimeSelection {
                schema_version: 1,
                backend: NativeRuntimeBackend::Cuda,
                llama_cpp_backend: Some(NativeRuntimeBackend::Cuda),
                onnx_backend: None,
                cuda_version: Some("13.3".into()),
                provider_dir: None,
                onnx_core_library: None,
                cuda_bin_dir: Some(PathBuf::from("runtime/cuda/13.3")),
                cudnn_bin_dir: None,
                preload_libraries: Vec::new(),
                fallback_reason: None,
            },
        )
        .unwrap();
        let (sender, _receiver) = unbounded();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(install_onnx_runtime(
                root.clone(),
                OnnxRuntimeSelection {
                    backend: RuntimeBackend::Cpu,
                    provider: None,
                    cuda_runtime: None,
                    cudnn: None,
                    cuda_version: None,
                    fallback_reason: Some("CUDA provider unavailable; using CPU inference.".into()),
                },
                sender,
                None,
                DownloadSource::Official,
                DownloadCancellation::default(),
                0,
                0,
            ))
            .unwrap();
        let marker = load_native_runtime_selection(&layout).unwrap().unwrap();
        assert_eq!(marker.backend, NativeRuntimeBackend::Cpu);
        assert_eq!(marker.llama_cpp_backend, Some(NativeRuntimeBackend::Cuda));
        assert_eq!(marker.onnx_backend, Some(NativeRuntimeBackend::Cpu));
        assert_eq!(
            marker.onnx_core_library.as_deref(),
            Some(Path::new("runtime/onnxruntime/cpu/onnxruntime.dll"))
        );
        assert_eq!(
            marker.cuda_bin_dir.as_deref(),
            Some(Path::new("runtime/cuda/13.3"))
        );
        assert!(marker.preload_libraries.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_downloads_reject_duplicate_names_and_non_https_urls() {
        let config = LlamaCppRuntimeConfig {
            release: "test".into(),
            release_page: "https://example.invalid/releases/test".into(),
            downloads: vec![
                xrtranslate_config::LlamaCppDownload {
                    name: "llama-test-bin-win-cpu-x64.zip".into(),
                    url: "https://example.invalid/one.zip".into(),
                    bytes: 1,
                    sha256: "0".repeat(64),
                    ..Default::default()
                },
                xrtranslate_config::LlamaCppDownload {
                    name: "llama-test-bin-win-cpu-x64.zip".into(),
                    url: "http://example.invalid/two.zip".into(),
                    bytes: 1,
                    sha256: "0".repeat(64),
                    ..Default::default()
                },
            ],
        };
        let error = release_assets_from_config(&config).unwrap_err();
        assert!(error.contains("duplicate archive"));
    }

    #[test]
    fn selects_complete_cuda_runtime_for_blackwell() {
        let assets = vec![
            asset("llama-b1-bin-win-cpu-x64.zip"),
            asset("llama-b1-bin-win-cuda-12.4-x64.zip"),
            asset("cudart-llama-bin-win-cuda-12.4-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.3-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.3-x64.zip"),
        ];
        let nvidia = NvidiaCuda {
            gpu: "NVIDIA GeForce RTX 5080".into(),
            compute_capability: (12, 0),
            driver_cuda: "13.3".into(),
            memory_bytes: 16 * 1024 * 1024 * 1024,
        };
        let selected = select_assets_for_hardware(&assets, Some(&nvidia)).unwrap();
        assert_eq!(selected.backend, RuntimeBackend::Cuda);
        assert_eq!(
            selected
                .assets
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>(),
            [
                "llama-b1-bin-win-cuda-13.3-x64.zip",
                "cudart-llama-bin-win-cuda-13.3-x64.zip"
            ]
        );
    }

    #[test]
    fn pre_turing_gpu_never_selects_cuda_13() {
        let assets = vec![
            asset("llama-b1-bin-win-cuda-12.4-x64.zip"),
            asset("cudart-llama-bin-win-cuda-12.4-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.3-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.3-x64.zip"),
        ];
        for (gpu, compute_capability) in [
            ("NVIDIA GeForce GTX 1080", (6, 1)),
            ("NVIDIA TITAN V", (7, 0)),
        ] {
            let selected = select_assets_for_hardware(
                &assets,
                Some(&NvidiaCuda {
                    gpu: gpu.into(),
                    compute_capability,
                    driver_cuda: "13.3".into(),
                    memory_bytes: 16 * 1024 * 1024 * 1024,
                }),
            )
            .unwrap();
            assert_eq!(
                selected.assets[0].name,
                "llama-b1-bin-win-cuda-12.4-x64.zip"
            );
        }
    }

    #[test]
    fn turing_and_newer_can_select_cuda_13() {
        assert!(!cuda_supports_compute_capability((13, 3), (7, 0)));
        assert!(cuda_supports_compute_capability((13, 3), (7, 5)));
        assert!(cuda_supports_compute_capability((13, 3), (8, 9)));
    }

    #[test]
    fn blackwell_selects_cuda_13_1_when_the_driver_cannot_load_cuda_13_3() {
        let assets = vec![
            asset("llama-b1-bin-win-cpu-x64.zip"),
            asset("llama-b1-bin-win-cuda-12.4-x64.zip"),
            asset("cudart-llama-bin-win-cuda-12.4-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.1-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.1-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.3-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.3-x64.zip"),
        ];
        let nvidia = NvidiaCuda {
            gpu: "NVIDIA GeForce RTX 5080".into(),
            compute_capability: (12, 0),
            driver_cuda: "13.2".into(),
            memory_bytes: 16 * 1024 * 1024 * 1024,
        };
        let selected = select_assets_for_hardware(&assets, Some(&nvidia)).unwrap();
        assert_eq!(selected.backend, RuntimeBackend::Cuda);
        assert_eq!(selected.assets[0].cuda_version.as_deref(), Some("13.1"));
        let notice = selected.fallback_reason.unwrap();
        assert!(notice.contains("NVIDIA App"));
        assert!(notice.contains(NVIDIA_APP_URL));
    }

    #[test]
    fn blackwell_driver_below_13_1_reports_the_minimum_complete_gpu_package() {
        let assets = vec![
            asset("llama-b1-bin-win-cpu-x64.zip"),
            asset("llama-b1-bin-win-cuda-12.4-x64.zip"),
            asset("cudart-llama-bin-win-cuda-12.4-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.1-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.1-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.3-x64.zip"),
            asset("cudart-llama-bin-win-cuda-13.3-x64.zip"),
        ];
        let error = select_assets_for_hardware(
            &assets,
            Some(&NvidiaCuda {
                gpu: "NVIDIA GeForce RTX 5080".into(),
                compute_capability: (12, 0),
                driver_cuda: "13.0".into(),
                memory_bytes: 16 * 1024 * 1024 * 1024,
            }),
        )
        .unwrap_err();
        assert!(error.contains("CUDA 13.1-capable"));
        assert!(!error.contains("needs a CUDA 13.3-capable"));
        assert!(error.contains(NVIDIA_APP_URL));
    }

    #[test]
    fn missing_cudart_is_rejected_without_cpu_fallback() {
        let assets = vec![
            asset("llama-b1-bin-win-cpu-x64.zip"),
            asset("llama-b1-bin-win-cuda-13.3-x64.zip"),
        ];
        let nvidia = NvidiaCuda {
            gpu: "NVIDIA GeForce RTX 5080".into(),
            compute_capability: (12, 0),
            driver_cuda: "13.3".into(),
            memory_bytes: 16 * 1024 * 1024 * 1024,
        };
        let error = select_assets_for_hardware(&assets, Some(&nvidia)).unwrap_err();
        assert!(error.contains("missing the CUDA runtime package"));
    }

    #[test]
    fn parses_all_nvidia_gpus_instead_of_only_the_first() {
        let gpus = parse_nvidia_gpu_rows(
            "Unavailable virtual adapter, N/A, N/A\nNVIDIA GeForce GTX 580, 2.0, 1536\nNVIDIA GeForce RTX 5080, 12.0, 16384\n",
        )
        .unwrap();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].gpu, "NVIDIA GeForce RTX 5080");
        assert_eq!(gpus[1].compute_capability, (12, 0));
        assert_eq!(gpus[1].memory_bytes, 16 * 1024 * 1024 * 1024);
    }

    #[test]
    fn managed_local_models_require_eight_gib_of_vram() {
        let low_memory = NvidiaCuda {
            gpu: "NVIDIA GeForce RTX test".into(),
            compute_capability: (8, 9),
            driver_cuda: "13.0".into(),
            memory_bytes: 7 * 1024 * 1024 * 1024,
        };
        assert!(matches!(
            local_model_availability(Some(&low_memory)),
            LocalModelAvailability::Unavailable(reason) if reason.contains("at least 8 GiB")
        ));
        assert!(matches!(
            local_model_availability(None),
            LocalModelAvailability::Unavailable(reason) if reason.contains("require an NVIDIA GPU")
        ));
    }

    #[test]
    fn runtime_assets_use_declared_cuda_versions() {
        let assets = release_assets_from_config(&LlamaCppRuntimeConfig {
            release: "test".into(),
            downloads: vec![xrtranslate_config::LlamaCppDownload {
                name: "server.zip".into(),
                url: "https://example.invalid/server.zip".into(),
                archive_format: LlamaCppArchiveFormat::Zip,
                bytes: 1,
                sha256: "0".repeat(64),
                kind: LlamaCppAssetKind::ServerCuda,
                target: current_runtime_target(),
                cuda_version: Some("13.3".into()),
                executable: "llama-server".into(),
                required_files: vec!["libggml.so".into()],
                required_file_prefixes: Vec::new(),
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(assets[0].cuda_version.as_deref(), Some("13.3"));
    }

    #[test]
    fn declared_tar_gz_format_is_preserved_without_filename_inference() {
        let assets = release_assets_from_config(&LlamaCppRuntimeConfig {
            release: "test".into(),
            downloads: vec![xrtranslate_config::LlamaCppDownload {
                name: "server.tar.gz".into(),
                url: "https://example.invalid/server.tar.gz".into(),
                bytes: 1,
                sha256: "0".repeat(64),
                archive_format: LlamaCppArchiveFormat::TarGz,
                target: current_runtime_target(),
                kind: LlamaCppAssetKind::ServerCpu,
                executable: "bin/llama-server".into(),
                required_files: vec!["lib/libggml.so".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(assets[0].archive_format, LlamaCppArchiveFormat::TarGz);
        assert_eq!(assets[0].kind, LlamaCppAssetKind::ServerCpu);
    }

    #[test]
    fn archive_paths_reject_parent_and_absolute_entries() {
        let root = Path::new("runtime/staging");
        assert!(safe_archive_path(root, Path::new("../escape")).is_err());
        assert!(safe_archive_path(root, Path::new("/absolute")).is_err());
        assert_eq!(
            safe_archive_path(root, Path::new("bin/server")).unwrap(),
            root.join("bin/server")
        );
    }

    #[test]
    fn source_switch_cleanup_removes_only_declared_runtime_staging() {
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-runtime-source-switch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.json"), include_str!("../../config.json")).unwrap();
        let config = load_app_config(&root).unwrap();
        let layout = load_runtime_layout(&root);
        let llama_staging = layout.runtime_root().join(format!(
            ".llama.cpp-{}-staging",
            config.model_manager.llama_cpp.release
        ));
        let onnx_staging = layout.runtime_root().join(format!(
            ".onnxruntime-{}-staging",
            config.model_manager.onnxruntime.release
        ));
        let installed = layout.llama_cpp_directory();
        std::fs::create_dir_all(&llama_staging).unwrap();
        std::fs::create_dir_all(&onnx_staging).unwrap();
        std::fs::create_dir_all(&installed).unwrap();

        clear_runtime_staging(&root).unwrap();

        assert!(!llama_staging.exists());
        assert!(!onnx_staging.exists());
        assert!(installed.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_managed_runtime_keeps_the_packaged_cpu_core() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-runtime-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.json"), include_str!("../../config.json")).unwrap();
        let config = load_app_config(&root).unwrap();
        let layout = load_runtime_layout(&root);
        std::fs::create_dir_all(layout.llama_cpp_directory()).unwrap();
        for version in config
            .model_manager
            .llama_cpp
            .downloads
            .iter()
            .filter_map(|asset| asset.cuda_version.as_deref())
        {
            std::fs::create_dir_all(layout.cuda_runtime_directory(version)).unwrap();
        }
        for asset in &config.model_manager.onnxruntime.downloads {
            std::fs::create_dir_all(layout.onnx_runtime_directory(&asset.cuda_version)).unwrap();
        }
        for asset in &config.model_manager.onnxruntime.cudnn_downloads {
            std::fs::create_dir_all(layout.cudnn_runtime_directory(&asset.cuda_version)).unwrap();
        }
        let cpu_core = layout.onnx_cpu_core_library();
        std::fs::create_dir_all(cpu_core.parent().unwrap()).unwrap();
        std::fs::write(&cpu_core, b"packaged").unwrap();
        std::fs::write(layout.native_runtime_selection_file(), b"{}").unwrap();

        let mut installer = RuntimeInstaller::default();
        installer.delete_managed_resources(&root).unwrap();

        assert!(!layout.llama_cpp_directory().exists());
        assert!(!layout.native_runtime_selection_file().exists());
        assert!(!layout.cudnn_runtime_directory("13").exists());
        assert!(cpu_core.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_current_nvidia_smi_cuda_umd_output() {
        let output = "| NVIDIA-SMI 610.47  CUDA UMD Version: 13.3 |";
        assert_eq!(
            cuda_version_from_nvidia_smi(output).as_deref(),
            Some("13.3")
        );
    }

    #[test]
    fn legacy_cuda_13_dlls_are_migrated_to_shared_cuda_13_directory() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-legacy-cuda13-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = RuntimeLayout::new(&root, Some(Path::new("runtime")));
        let llama_dir = layout.llama_cpp_directory();
        std::fs::create_dir_all(&llama_dir).unwrap();
        std::fs::write(llama_dir.join("llama-server.exe"), b"bin").unwrap();
        std::fs::write(llama_dir.join("cudart64_13.dll"), b"cuda13").unwrap();
        std::fs::write(llama_dir.join("cublas64_13.dll"), b"cublas13").unwrap();
        std::fs::write(llama_dir.join("cublasLt64_13.dll"), b"cublaslt13").unwrap();

        migrate_legacy_runtime_layout(&layout);

        let cuda13_dir = layout.cuda_runtime_directory("13.3");
        assert!(cuda13_dir.join("cudart64_13.dll").is_file());
        assert!(cuda13_dir.join("cublas64_13.dll").is_file());
        assert!(cuda13_dir.join("cublasLt64_13.dll").is_file());
        assert!(!layout.cuda_runtime_directory("12.4").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_cuda_12_dlls_are_migrated_to_shared_cuda_12_directory() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-legacy-cuda12-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = RuntimeLayout::new(&root, Some(Path::new("runtime")));
        let llama_dir = layout.llama_cpp_directory();
        std::fs::create_dir_all(&llama_dir).unwrap();
        std::fs::write(llama_dir.join("llama-server.exe"), b"bin").unwrap();
        std::fs::write(llama_dir.join("cudart64_12.dll"), b"cuda12").unwrap();
        std::fs::write(llama_dir.join("cublas64_12.dll"), b"cublas12").unwrap();
        std::fs::write(llama_dir.join("cublasLt64_12.dll"), b"cublaslt12").unwrap();

        migrate_legacy_runtime_layout(&layout);

        let cuda12_dir = layout.cuda_runtime_directory("12.4");
        assert!(cuda12_dir.join("cudart64_12.dll").is_file());
        assert!(cuda12_dir.join("cublas64_12.dll").is_file());
        assert!(cuda12_dir.join("cublasLt64_12.dll").is_file());
        assert!(!layout.cuda_runtime_directory("13.3").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_cpu_runtime_does_not_create_cuda_directories() {
        let root =
            std::env::temp_dir().join(format!("xrtranslate-legacy-cpu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = RuntimeLayout::new(&root, Some(Path::new("runtime")));
        let llama_dir = layout.llama_cpp_directory();
        std::fs::create_dir_all(&llama_dir).unwrap();
        std::fs::write(llama_dir.join("llama-server.exe"), b"bin").unwrap();
        std::fs::write(llama_dir.join("ggml-cpu.dll"), b"cpu").unwrap();

        migrate_legacy_runtime_layout(&layout);

        assert!(!layout.cuda_runtime_directory("13.3").exists());
        assert!(!layout.cuda_runtime_directory("12.4").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
