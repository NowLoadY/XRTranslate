//! Native llama.cpp runtime discovery and installation.
//!
//! The configured `model_manager.llama_cpp.downloads` list is the contract:
//! CUDA and Vulkan selection follows verified GPU capabilities and the selected
//! packages' memory requirements. Managed
//! model packages never fall back to CPU. Bundled small ONNX components remain
//! a separate resource class; their missing files can be repaired here, reusing
//! a compatible local ONNX core before scheduling any download.

use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    thread,
};
#[cfg(any(not(target_os = "android"), test))]
use xrtranslate_config::ManagedRuntimeArchive;
use xrtranslate_config::{
    AppConfig, LlamaCppArchiveFormat, LlamaCppAssetKind, LlamaCppRuntimeConfig,
    NativeRuntimeBackend, NativeRuntimeSelection, OnnxRuntimeConfig, RuntimeLayout,
    RuntimeRequirements,
};
use xrtranslate_download::{DownloadCancellation, DownloadClient, DownloadSource, DownloadSpec};

#[cfg(any(not(target_os = "android"), test))]
const TURING_COMPUTE_CAPABILITY: (u16, u16) = (7, 5);
#[cfg(any(not(target_os = "android"), test))]
const BLACKWELL_MINIMUM_CUDA: (u16, u16) = (12, 8);
#[cfg(any(not(target_os = "android"), test))]
mod bundled_cpu;
mod hardware;
use hardware::Hardware;
pub use hardware::LocalModelAvailability;
pub(crate) use hardware::NVIDIA_APP_URL;
#[cfg(any(not(target_os = "android"), test))]
use hardware::{NvidiaCuda, VulkanGpu};

type RuntimeBackend = NativeRuntimeBackend;

#[derive(Clone, Debug)]
struct RuntimeSelection {
    assets: Vec<ReleaseAsset>,
    backend: RuntimeBackend,
    executable: String,
    vulkan_device: Option<u32>,
    fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct OnnxRuntimeSelection {
    backend: RuntimeBackend,
    provider: Option<ManagedRuntimeAsset>,
    cuda_runtime: Option<ReleaseAsset>,
    cuda_dependency: Option<ManagedRuntimeAsset>,
    cudnn: Option<ManagedRuntimeAsset>,
    cuda_version: Option<String>,
    fallback_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct RuntimePlan {
    llama_cpp: Option<RuntimeSelection>,
    onnx: Option<OnnxRuntimeSelection>,
    downloads: Vec<RuntimeDownload>,
    marker_ready: bool,
    requirements: RuntimeRequirements,
    model_assets: Vec<xrtranslate_assets::ModelAssetId>,
    local_models: LocalModelAvailability,
    blocking_error: Option<String>,
}

impl RuntimePlan {
    fn total_bytes(&self) -> u64 {
        self.downloads.iter().map(|download| download.bytes).sum()
    }

    fn backend(&self) -> RuntimeBackend {
        self.onnx
            .as_ref()
            .map(|selection| selection.backend)
            .or_else(|| self.llama_cpp.as_ref().map(|selection| selection.backend))
            .unwrap_or(RuntimeBackend::Cpu)
    }

    fn is_ready(&self) -> bool {
        self.blocking_error.is_none() && self.downloads.is_empty() && self.marker_ready
    }

    fn requires_marker_repair(&self) -> bool {
        self.blocking_error.is_none() && self.downloads.is_empty() && !self.marker_ready
    }
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

/// One missing archive selected for this computer's managed runtime closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDownload {
    pub label: String,
    pub archive_name: String,
    pub bytes: u64,
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
    // Only a real llama executable is returned to host coordination. An ONNX
    // marker is an installation side effect, never an executable candidate.
    Finished(Result<Option<PathBuf>, String>),
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
    pub fn planned_downloads(&self) -> &[RuntimeDownload] {
        self.selection
            .as_ref()
            .map_or(&[], |selection| selection.downloads.as_slice())
    }

    #[must_use]
    pub fn plan_is_ready(&self) -> bool {
        self.selection.as_ref().is_some_and(RuntimePlan::is_ready)
    }

    #[must_use]
    pub fn plan_matches(
        &self,
        requirements: RuntimeRequirements,
        model_assets: &[xrtranslate_assets::ModelAssetId],
    ) -> bool {
        self.selection.as_ref().is_some_and(|selection| {
            selection.requirements == requirements && selection.model_assets == model_assets
        })
    }

    #[must_use]
    pub fn has_plan(&self) -> bool {
        self.selection.is_some()
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
                        selection.downloads.clear();
                        selection.marker_ready = true;
                    }
                    self.state = match result {
                        Ok(executable) => {
                            installed_executable = executable;
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
    archive_format: LlamaCppArchiveFormat,
    archive_directory: String,
    kind: LlamaCppAssetKind,
    #[cfg(any(not(target_os = "android"), test))]
    target: String,
    cuda_version: Option<String>,
    #[cfg(any(not(target_os = "android"), test))]
    executable: String,
    required_files: Vec<String>,
    required_file_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManagedRuntimeAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    archive_format: LlamaCppArchiveFormat,
    #[cfg(any(not(target_os = "android"), test))]
    target: String,
    cuda_version: String,
    archive_directory: String,
    required_files: Vec<String>,
}

fn persist_cpu_onnx_marker(
    layout: &RuntimeLayout,
    fallback_reason: Option<&str>,
) -> Result<PathBuf, String> {
    let existing = load_native_runtime_selection(layout)?;
    let marker = NativeRuntimeSelection {
        schema_version: 1,
        backend: NativeRuntimeBackend::Cpu,
        llama_cpp_backend: existing
            .as_ref()
            .and_then(|marker| marker.llama_cpp_backend),
        vulkan_device: existing.as_ref().and_then(|marker| marker.vulkan_device),
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
        fallback_reason: fallback_reason.map(str::to_owned).or_else(|| {
            existing
                .as_ref()
                .and_then(|marker| marker.fallback_reason.clone())
        }),
    };
    persist_native_runtime_selection(layout, &marker)?;
    Ok(layout.native_runtime_selection_file())
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
        return persist_cpu_onnx_marker(&layout, selection.fallback_reason.as_deref());
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
    let cuda_dependency = selection
        .cuda_dependency
        .as_ref()
        .ok_or_else(|| "CUDA ONNX plan is missing its CUDA dependency archive".to_owned())?;
    let cuda_version = selection
        .cuda_version
        .as_deref()
        .ok_or_else(|| "CUDA ONNX plan has no CUDA version".to_owned())?;
    let cuda_directory = layout.cuda_runtime_directory(cuda_version);
    let cudnn_directory = layout.cudnn_runtime_directory(&cudnn.cuda_version);
    let provider_directory = layout.onnx_runtime_directory(&provider.cuda_version);
    let cuda_ready =
        validate_required_prefixes(&cuda_directory, &cuda_runtime.required_file_prefixes).is_ok();
    let cuda_dependency_ready =
        validate_required_files(&cuda_directory, &cuda_dependency.required_files).is_ok();
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

    if !cuda_ready || !cuda_dependency_ready {
        let staged_cuda = payload.join("cuda");
        if cuda_ready {
            fs::create_dir_all(&staged_cuda)
                .map_err(|error| format!("Cannot create staged CUDA folder: {error}"))?;
            for source in
                resolve_required_prefixes(&cuda_directory, &cuda_runtime.required_file_prefixes)?
            {
                let filename = source
                    .file_name()
                    .ok_or_else(|| format!("Invalid CUDA runtime file: {}", source.display()))?;
                fs::copy(&source, staged_cuda.join(filename)).map_err(|error| {
                    format!(
                        "Cannot stage existing CUDA file {}: {error}",
                        source.display()
                    )
                })?;
            }
        } else {
            let archive = downloads.join(&cuda_runtime.name);
            download_file(
                &client,
                &cuda_runtime.name,
                &cuda_runtime.browser_download_url,
                cuda_runtime.size,
                &archive,
                progress_base.saturating_add(completed),
                progress_total,
                &sender,
            )
            .await?;
            completed = completed.saturating_add(cuda_runtime.size);
            let extracted_cuda = payload.join("cuda-archive");
            fs::create_dir_all(&extracted_cuda)
                .map_err(|error| format!("Cannot create CUDA extraction folder: {error}"))?;
            extract_archive(&archive, &extracted_cuda, cuda_runtime.archive_format)?;
            let archive_directory = Path::new(&cuda_runtime.archive_directory);
            let cuda_contents = if archive_directory.as_os_str().is_empty() {
                extracted_cuda
            } else {
                safe_archive_path(&extracted_cuda, archive_directory)?
            };
            fs::rename(&cuda_contents, &staged_cuda).map_err(|error| {
                format!(
                    "Cannot stage CUDA runtime from {}: {error}",
                    cuda_contents.display()
                )
            })?;
        }
        if cuda_dependency_ready {
            for source in resolve_required_files(&cuda_directory, &cuda_dependency.required_files)?
            {
                let filename = source
                    .file_name()
                    .ok_or_else(|| format!("Invalid CUDA dependency file: {}", source.display()))?;
                fs::copy(&source, staged_cuda.join(filename)).map_err(|error| {
                    format!(
                        "Cannot stage existing CUDA dependency {}: {error}",
                        source.display()
                    )
                })?;
            }
        } else {
            let archive = downloads.join(&cuda_dependency.name);
            download_file(
                &client,
                &cuda_dependency.name,
                &cuda_dependency.browser_download_url,
                cuda_dependency.size,
                &archive,
                progress_base.saturating_add(completed),
                progress_total,
                &sender,
            )
            .await?;
            completed = completed.saturating_add(cuda_dependency.size);
            extract_declared_files(
                &archive,
                cuda_dependency.archive_format,
                Path::new(&cuda_dependency.archive_directory),
                &cuda_dependency.required_files,
                &staged_cuda,
            )?;
        }
        validate_required_prefixes(&staged_cuda, &cuda_runtime.required_file_prefixes)?;
        validate_required_files(&staged_cuda, &cuda_dependency.required_files)?;
        activate_runtime_directory(&staged_cuda, &cuda_directory)?;
    }

    if !cudnn_ready {
        let archive = downloads.join(&cudnn.name);
        download_file(
            &client,
            &cudnn.name,
            &cudnn.browser_download_url,
            cudnn.size,
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
        download_file(
            &client,
            &provider.name,
            &provider.browser_download_url,
            provider.size,
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
        &cuda_directory,
        &cuda_dependency.required_files,
    )?);
    preload_libraries.extend(resolve_required_files(
        &cudnn_directory,
        &cudnn.required_files,
    )?);
    let onnx_core_library = provider_directory.join(RuntimeLayout::ONNX_CORE_LIBRARY);
    let existing = load_native_runtime_selection(&layout)?;
    let marker = NativeRuntimeSelection {
        schema_version: 1,
        backend: NativeRuntimeBackend::Cuda,
        llama_cpp_backend: existing
            .as_ref()
            .and_then(|marker| marker.llama_cpp_backend),
        vulkan_device: existing.as_ref().and_then(|marker| marker.vulkan_device),
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

async fn download_file(
    client: &DownloadClient,
    label: &str,
    url: &str,
    bytes: u64,
    destination: &Path,
    completed: u64,
    total: u64,
    sender: &crossbeam_channel::Sender<Event>,
) -> Result<(), String> {
    let spec = DownloadSpec::new(label, url, bytes);
    let label_owned = label.to_owned();
    client
        .download_to(spec, destination, move |progress| {
            let _ = sender.send(Event::Downloading {
                asset: label_owned.clone(),
                downloaded: completed.saturating_add(progress.downloaded_bytes),
                total,
            });
        })
        .await
        .map_err(|error| error.to_string())
}

fn file_has_expected_size(path: &Path, expected: Option<u64>) -> bool {
    fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && expected.map_or(metadata.len() > 0, |bytes| metadata.len() == bytes)
    })
}

fn expected_cpu_core_bytes(config: &AppConfig) -> Option<u64> {
    // The Windows packaged constant describes the CUDA core, while its CPU
    // download is a different build whose extracted size is not catalogued.
    if cfg!(windows) {
        return None;
    }
    let target = current_runtime_target();
    let configured = config.model_manager.onnxruntime.resolved_cpu_downloads();
    let defaults = OnnxRuntimeConfig::default_cpu_downloads();
    let expected = defaults.iter().find(|archive| archive.target == target)?;
    (configured
        .iter()
        .find(|archive| archive.target.trim() == target)
        == Some(expected))
    .then_some(RuntimeLayout::ONNX_CPU_CORE_LINUX_BYTES)
}

fn expected_bundled_model_bytes(asset: &xrtranslate_config::BundledModelAsset) -> Option<u64> {
    match asset.relative_path.as_str() {
        RuntimeLayout::VAD_MODEL_PATH => Some(RuntimeLayout::VAD_MODEL_BYTES),
        RuntimeLayout::DENOISE_MODEL_PATH => Some(RuntimeLayout::DENOISE_MODEL_BYTES),
        RuntimeLayout::SPEAKER_MODEL_PATH => Some(RuntimeLayout::SPEAKER_MODEL_BYTES),
        _ if asset.archive_format.is_none() => Some(asset.bytes),
        _ => None,
    }
}

fn validate_extracted_file(
    archive: &Path,
    path: &Path,
    expected: Option<u64>,
) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        let message = format!("Cannot inspect extracted file {}: {error}", path.display());
        if error.kind() == std::io::ErrorKind::NotFound {
            archive_content_error(archive, message)
        } else {
            message
        }
    })?;
    if !metadata.is_file() || !expected.map_or(metadata.len() > 0, |bytes| metadata.len() == bytes)
    {
        return Err(archive_content_error(
            archive,
            format!(
                "Archive contains an incorrectly sized file: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

async fn install_base_bundled_resources(
    project_root: &Path,
    config: &AppConfig,
    plan: &RuntimePlan,
    client: &DownloadClient,
    sender: &crossbeam_channel::Sender<Event>,
    completed: &mut u64,
    total: u64,
) -> Result<(), String> {
    let layout = config.runtime_layout(project_root);
    let target = current_runtime_target();

    if !file_has_expected_size(
        &layout.onnx_cpu_core_library(),
        expected_cpu_core_bytes(config),
    ) {
        if let Some(archive) = config
            .model_manager
            .onnxruntime
            .resolved_cpu_downloads()
            .into_iter()
            .find(|archive| archive.target.trim() == target)
        {
            if plan
                .downloads
                .iter()
                .any(|d| d.archive_name == archive.name)
            {
                let staging = layout.runtime_root().join(".onnxruntime-cpu-staging");
                let downloads_dir = staging.join("downloads");
                fs::create_dir_all(&downloads_dir)
                    .map_err(|e| format!("Cannot create CPU ONNX staging: {e}"))?;
                let archive_file = downloads_dir.join(&archive.name);

                download_file(
                    client,
                    "ONNX Runtime (CPU)",
                    &archive.url,
                    archive.bytes,
                    &archive_file,
                    *completed,
                    total,
                    sender,
                )
                .await?;
                *completed = completed.saturating_add(archive.bytes);

                let payload = staging.join("payload");
                if payload.exists() {
                    fs::remove_dir_all(&payload)
                        .map_err(|error| format!("Cannot reset CPU ONNX staging: {error}"))?;
                }
                fs::create_dir_all(&payload)
                    .map_err(|error| format!("Cannot create CPU ONNX staging: {error}"))?;
                extract_declared_files(
                    &archive_file,
                    archive.archive_format,
                    Path::new(&archive.archive_directory),
                    &archive.required_files,
                    &payload,
                )?;
                validate_extracted_file(
                    &archive_file,
                    &payload.join(RuntimeLayout::ONNX_CORE_LIBRARY),
                    expected_cpu_core_bytes(config),
                )?;

                #[cfg(unix)]
                {
                    let link = payload.join("libonnxruntime.so");
                    let symlink_target = Path::new(RuntimeLayout::ONNX_CORE_LIBRARY);
                    if !link.exists() {
                        let _ = std::os::unix::fs::symlink(symlink_target, &link);
                    }
                }

                activate_runtime_directory(&payload, &layout.onnx_cpu_runtime_directory())?;
                let _ = fs::remove_dir_all(&staging);
            }
        }
    }

    for bundled in config.model_manager.resolved_bundled_models() {
        if !bundled
            .target
            .as_deref()
            .map_or(true, |t| t.trim() == target)
        {
            continue;
        }
        if !plan
            .downloads
            .iter()
            .any(|d| d.archive_name == bundled.name)
        {
            continue;
        }
        let destination = project_root.join(&bundled.relative_path);
        if file_has_expected_size(&destination, expected_bundled_model_bytes(&bundled)) {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }

        if let Some(archive_format) = bundled.archive_format {
            let archive_path = bundled
                .archive_path
                .as_deref()
                .ok_or_else(|| format!("Bundled model {} missing archive_path", bundled.name))?;
            let staging = layout.runtime_root().join(".model-staging");
            fs::create_dir_all(&staging)
                .map_err(|e| format!("Cannot create model staging: {e}"))?;
            let archive_file = staging.join(format!("{}.zip", bundled.name));

            download_file(
                client,
                &bundled.label,
                &bundled.url,
                bundled.bytes,
                &archive_file,
                *completed,
                total,
                sender,
            )
            .await?;
            *completed = completed.saturating_add(bundled.bytes);

            let archive_entry = Path::new(archive_path);
            let entry_dir = archive_entry.parent().unwrap_or(Path::new(""));
            let entry_file = archive_entry
                .file_name()
                .and_then(|f| f.to_str())
                .ok_or_else(|| {
                    format!("Invalid archive_path in {}: {archive_path}", bundled.name)
                })?;
            let payload = destination.with_file_name(format!(".{entry_file}-installing"));
            if payload.exists() {
                fs::remove_dir_all(&payload)
                    .map_err(|error| format!("Cannot reset model staging: {error}"))?;
            }
            fs::create_dir_all(&payload)
                .map_err(|error| format!("Cannot create model staging: {error}"))?;
            extract_declared_files(
                &archive_file,
                archive_format,
                entry_dir,
                &[entry_file.to_owned()],
                &payload,
            )?;
            let extracted = payload.join(entry_file);
            validate_extracted_file(
                &archive_file,
                &extracted,
                expected_bundled_model_bytes(&bundled),
            )?;
            atomic_replace_file(&extracted, &destination)?;
            let _ = fs::remove_dir_all(&payload);
            let _ = fs::remove_dir_all(&staging);
        } else {
            download_file(
                client,
                &bundled.label,
                &bundled.url,
                bundled.bytes,
                &destination,
                *completed,
                total,
                sender,
            )
            .await?;
            *completed = completed.saturating_add(bundled.bytes);
        }
    }

    Ok(())
}

async fn install_runtime_plan(
    project_root: PathBuf,
    plan: RuntimePlan,
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
) -> Result<Option<PathBuf>, String> {
    let config = load_app_config(&project_root)?;
    let layout = config.runtime_layout(&project_root);
    let progress_total = plan.total_bytes();
    let mut completed = 0_u64;
    #[cfg(not(target_os = "android"))]
    let restore_cpu_from_provider = bundled_cpu::provider_download_supplies_cpu_core(
        &config,
        plan.onnx.as_ref(),
        &plan.downloads,
    );

    if progress_total > 0 && !plan.downloads.is_empty() {
        let client = DownloadClient::with_proxy_source_and_cancellation(
            "XRTranslate runtime installer",
            proxy_url,
            source,
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;

        install_base_bundled_resources(
            &project_root,
            &config,
            &plan,
            &client,
            &sender,
            &mut completed,
            progress_total,
        )
        .await?;
    }

    let mut llama_executable = None;
    let mut runtime_marker = None;
    if let Some(selection) = plan.llama_cpp {
        let llama_bytes = missing_runtime_bytes(&project_root, Some(&selection), None);
        llama_executable = Some(
            install(
                project_root.clone(),
                selection,
                sender.clone(),
                proxy_url,
                source,
                cancellation.clone(),
                completed,
                progress_total,
            )
            .await?,
        );
        completed = completed.saturating_add(llama_bytes);
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
                completed,
                progress_total,
            )
            .await?,
        );
    } else if file_has_expected_size(
        &layout.onnx_cpu_core_library(),
        expected_cpu_core_bytes(&config),
    ) {
        runtime_marker = Some(persist_cpu_onnx_marker(&layout, None)?);
    }
    #[cfg(not(target_os = "android"))]
    if restore_cpu_from_provider && !bundled_cpu::restore_cpu_core(&layout, &config)? {
        return Err(
            "The installed ONNX archive does not contain the expected bundled CPU core. Repair the runtime configuration and try again."
                .into(),
        );
    }
    if llama_executable.is_none() && runtime_marker.is_none() {
        return Err("No native runtime is required by the selected providers.".into());
    }
    Ok(llama_executable)
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
            selection.vulkan_device,
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
        download_file(
            &client,
            &asset.name,
            &asset.browser_download_url,
            asset.size,
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
        let extracted_server = payload.join("llama.cpp");
        fs::create_dir_all(&extracted_server)
            .map_err(|error| format!("Cannot create staged llama.cpp folder: {error}"))?;
        for asset in &server_assets {
            extract_archive(
                &downloads.join(&asset.name),
                &extracted_server,
                asset.archive_format,
            )?;
        }
        let staged_server = safe_archive_path(
            &extracted_server,
            Path::new(&server_assets[0].archive_directory),
        )?;
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
        let extracted_cuda = payload.join("cuda");
        fs::create_dir_all(&extracted_cuda)
            .map_err(|error| format!("Cannot create staged CUDA folder: {error}"))?;
        extract_archive(
            &downloads.join(&asset.name),
            &extracted_cuda,
            asset.archive_format,
        )?;
        let staged_cuda = safe_archive_path(&extracted_cuda, Path::new(&asset.archive_directory))?;
        validate_required_prefixes(&staged_cuda, &asset.required_file_prefixes)?;
        activate_runtime_directory(&staged_cuda, directory)?;
    }
    persist_llama_runtime_marker(
        &layout,
        selection.backend,
        selection.vulkan_device,
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

pub(crate) fn archive_content_error(archive: &Path, message: String) -> String {
    let _ = fs::remove_file(archive);
    message
}

pub(crate) struct ArchiveReadError {
    message: String,
    invalid_content: bool,
}

impl ArchiveReadError {
    fn invalid(message: String) -> Self {
        Self {
            message,
            invalid_content: true,
        }
    }

    pub(crate) fn finish(self, archive: &Path) -> String {
        if self.invalid_content {
            archive_content_error(archive, self.message)
        } else {
            self.message
        }
    }
}

impl From<String> for ArchiveReadError {
    fn from(message: String) -> Self {
        Self {
            message,
            invalid_content: false,
        }
    }
}

pub(crate) fn archive_read_error(archive: &Path, error: std::io::Error) -> ArchiveReadError {
    use std::io::ErrorKind;
    // tar reports malformed headers as Other; operating-system I/O errors
    // retain their errno and must not trigger another download.
    let invalid_content = matches!(
        error.kind(),
        ErrorKind::InvalidData | ErrorKind::UnexpectedEof
    ) || (error.kind() == ErrorKind::Other
        && error.raw_os_error().is_none()
        && error
            .get_ref()
            .is_some_and(|source| source.source().is_none()));
    ArchiveReadError {
        message: format!("Cannot read archive {}: {error}", archive.display()),
        invalid_content,
    }
}

pub(crate) fn zip_read_error(archive: &Path, error: zip::result::ZipError) -> ArchiveReadError {
    match error {
        zip::result::ZipError::Io(error) => archive_read_error(archive, error),
        error => {
            ArchiveReadError::invalid(format!("Invalid archive {}: {error}", archive.display()))
        }
    }
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
    let result = (|| -> Result<(), ArchiveReadError> {
        match format {
            LlamaCppArchiveFormat::Zip => {
                let input = fs::File::open(archive)
                    .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
                let mut zip =
                    zip::ZipArchive::new(input).map_err(|error| zip_read_error(archive, error))?;
                for file in files {
                    let source = archive_directory.join(file);
                    let source = source.to_string_lossy().replace('\\', "/");
                    let mut entry = zip
                        .by_name(&source)
                        .map_err(|error| zip_read_error(archive, error))?;
                    let output = destination.join(file);
                    let mut target = fs::File::create(&output)
                        .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
                    let expected_bytes = entry.size();
                    let copied = std::io::copy(&mut entry, &mut target)
                        .map_err(|error| archive_read_error(archive, error))?;
                    if copied != expected_bytes {
                        return Err(ArchiveReadError::invalid(format!(
                            "Archive entry {} was truncated.",
                            output.display()
                        )));
                    }
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
                    .map_err(|error| archive_read_error(archive, error))?
                {
                    let mut entry = entry.map_err(|error| archive_read_error(archive, error))?;
                    let path = entry
                        .path()
                        .map_err(|error| archive_read_error(archive, error))?;
                    let Some((_, file)) = expected.iter().find(|(expected, _)| path == *expected)
                    else {
                        continue;
                    };
                    let output = destination.join(file);
                    let mut target = fs::File::create(&output)
                        .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
                    let expected_bytes = entry.size();
                    let copied = std::io::copy(&mut entry, &mut target)
                        .map_err(|error| archive_read_error(archive, error))?;
                    if copied != expected_bytes {
                        return Err(ArchiveReadError::invalid(format!(
                            "Archive entry {} was truncated.",
                            output.display()
                        )));
                    }
                    found.insert((*file).clone());
                }
                if let Some(missing) = files.iter().find(|file| !found.contains(*file)) {
                    return Err(ArchiveReadError::invalid(format!(
                        "Archive {} is missing {}",
                        archive.display(),
                        archive_directory.join(missing).display()
                    )));
                }
            }
        }
        Ok(())
    })();
    result.map_err(|error| error.finish(archive))
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
    let result = (|| -> Result<(), ArchiveReadError> {
        let file = fs::File::open(archive)
            .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|error| zip_read_error(archive, error))?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|error| zip_read_error(archive, error))?;
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
                .map_err(|error| archive_read_error(archive, error))?;
        }
        Ok(())
    })();
    result.map_err(|error| error.finish(archive))
}

fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), String> {
    let result = (|| -> Result<(), ArchiveReadError> {
        let file = fs::File::open(archive)
            .map_err(|error| format!("Cannot open {}: {error}", archive.display()))?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        #[cfg(unix)]
        let mut links = Vec::new();
        for entry in tar
            .entries()
            .map_err(|error| archive_read_error(archive, error))?
        {
            let mut entry = entry.map_err(|error| archive_read_error(archive, error))?;
            let name = entry
                .path()
                .map_err(|error| archive_read_error(archive, error))?
                .into_owned();
            let output = safe_archive_path(destination, &name)?;
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&output)
                    .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
                continue;
            }
            #[cfg(unix)]
            if entry.header().entry_type().is_symlink() {
                let target = entry
                    .link_name()
                    .map_err(|error| format!("Cannot read tar link: {error}"))?
                    .ok_or_else(|| format!("Tar link has no target: {}", name.display()))?;
                if target.components().count() != 1
                    || !matches!(
                        target.components().next(),
                        Some(std::path::Component::Normal(_))
                    )
                {
                    return Err(ArchiveReadError::invalid(format!(
                        "Tar link leaves its directory: {}",
                        name.display()
                    )));
                }
                links.push((output, target.into_owned()));
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(ArchiveReadError::invalid(format!(
                    "unsupported tar entry type: {}",
                    name.display()
                )));
            }
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
            }
            let mut file = fs::File::create(&output)
                .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
            std::io::copy(&mut entry, &mut file)
                .map_err(|error| archive_read_error(archive, error))?;
            #[cfg(unix)]
            if let Ok(mode) = entry.header().mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&output, fs::Permissions::from_mode(mode)).map_err(
                    |error| {
                        format!(
                            "Cannot restore permissions for {}: {error}",
                            output.display()
                        )
                    },
                )?;
            }
        }
        #[cfg(unix)]
        for (output, target) in &links {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
            }
            std::os::unix::fs::symlink(target, output)
                .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
        }
        #[cfg(unix)]
        for (output, _) in &links {
            if !output.is_file() {
                return Err(ArchiveReadError::invalid(format!(
                    "Tar link does not resolve to a file: {}",
                    output.display()
                )));
            }
        }
        Ok(())
    })();
    result.map_err(|error| error.finish(archive))
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
            let exact = directory.join(prefix);
            if exact.is_file() {
                return Ok(exact);
            }
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
    vulkan_device: Option<u32>,
    cuda_asset: Option<&ReleaseAsset>,
    cuda_directory: Option<&Path>,
    fallback_reason: Option<&str>,
) -> Result<(), String> {
    let existing = load_native_runtime_selection(layout)?;
    let marker = NativeRuntimeSelection {
        schema_version: 1,
        backend: existing
            .as_ref()
            .and_then(|marker| marker.onnx_backend)
            .unwrap_or(backend),
        llama_cpp_backend: Some(backend),
        vulkan_device,
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
            .and_then(|marker| marker.onnx_core_library.clone())
            .filter(|path| layout.project_root().join(path).is_file() || Path::new(path).is_file())
            .or_else(|| {
                let cpu_core = layout.onnx_cpu_core_library();
                if cpu_core.is_file() {
                    Some(layout.config_path_for(&cpu_core))
                } else {
                    None
                }
            }),
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
    move_file_windows(source, destination, true)
        .map_err(|error| format!("Cannot publish {}: {error}", destination.display()))
}

#[cfg(windows)]
fn publish_file_if_absent(source: &Path, destination: &Path) -> std::io::Result<()> {
    move_file_windows(source, destination, false)
}

#[cfg(all(not(windows), any(not(target_os = "android"), test)))]
fn publish_file_if_absent(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)
}

#[cfg(windows)]
fn move_file_windows(
    source: &Path,
    destination: &Path,
    replace_existing: bool,
) -> std::io::Result<()> {
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
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace_existing {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("Cannot publish {}: {error}", destination.display()))
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

fn configured_runtime_plan(
    project_root: &Path,
    requirements: RuntimeRequirements,
) -> Result<RuntimePlan, String> {
    let config = load_app_config(project_root)?;
    #[cfg(not(target_os = "android"))]
    {
        let layout = config.runtime_layout(project_root);
        migrate_legacy_runtime_layout(&layout);
        // Preparation runs on the planner worker, not in per-frame UI queries.
        bundled_cpu::restore_cpu_core(&layout, &config)?;
    }
    let model_assets = config
        .active_native_model_assets()
        .into_iter()
        .filter_map(|key| xrtranslate_assets::ModelAssetId::from_config_key(&key))
        .collect::<Vec<_>>();
    let preferred_gpu = config.model_manager.preferred_gpu.as_deref();
    let hardware = Hardware::detect(preferred_gpu);
    let local_models = hardware.availability.clone();
    let requires_managed_model = requirements.llama_cpp || requirements.onnx_tts;
    let blocking_error = requires_managed_model
        .then(|| {
            model_assets
                .iter()
                .map(|id| xrtranslate_assets::manifest_for(*id))
                .find(|model| !local_models.supports(model.hardware))
                .map(|model| {
                    if local_models.is_cpu() {
                        return format!(
                            "{} cannot run with the available memory and processor: {}",
                            model.label, local_models
                        );
                    }
                    format!(
                        "{} requires {} with at least {:.0} GiB of device memory. Detected: {}",
                        model.id,
                        model.hardware.accelerator.label(),
                        model.hardware.minimum_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        local_models,
                    )
                })
        })
        .flatten();
    #[cfg(target_os = "android")]
    let (llama_cpp, onnx, downloads, marker_ready) = {
        let executable = crate::android::native_executable("llama-server");
        let core = crate::android::native_executable("onnxruntime");
        let marker_ready = crate::android::resources_ready()
            && (!requirements.llama_cpp || executable.is_file())
            && (!(requirements.onnx_tts || requirements.onnx_cpu) || core.is_file())
            && project_root.join("runtime/native-runtime.json").is_file();
        let llama_cpp = requirements.llama_cpp.then(|| RuntimeSelection {
            assets: Vec::new(),
            backend: NativeRuntimeBackend::Cpu,
            executable: executable.display().to_string(),
            vulkan_device: None,
            fallback_reason: None,
        });
        (llama_cpp, None, Vec::new(), marker_ready)
    };
    #[cfg(not(target_os = "android"))]
    let (llama_cpp, onnx, downloads, marker_ready) = {
        let required_model_vram_bytes = required_local_model_vram_bytes(&config);
        let eligible_nvidia = hardware.nvidia.as_ref().filter(|_| {
            matches!(&local_models, LocalModelAvailability::Available { cuda_memory_bytes, .. }
                if *cuda_memory_bytes >= required_model_vram_bytes)
        });
        let eligible_amd = hardware
            .amd
            .as_ref()
            .filter(|gpu| gpu.memory_bytes >= required_model_vram_bytes);
        // ONNX CUDA deliberately reuses the declared llama.cpp CUDA redistributable
        // catalogue. Bundled CPU resources never add GPU acceleration requirements.
        let llama_assets = (requirements.llama_cpp || requirements.onnx_cuda)
            .then(|| release_assets_from_config(&config.model_manager.llama_cpp))
            .transpose()?
            .unwrap_or_default();
        let llama_cpp = (requirements.llama_cpp && blocking_error.is_none())
            .then(|| select_llama_assets(&llama_assets, eligible_nvidia, eligible_amd))
            .transpose()?;
        let onnx = if requirements.onnx_tts && requirements.onnx_cuda && blocking_error.is_none() {
            let providers = onnx_assets_from_config(&config.model_manager.onnxruntime)?;
            let cuda_dependencies =
                cuda_dependency_assets_from_config(&config.model_manager.onnxruntime)?;
            let cudnn_runtimes = cudnn_assets_from_config(&config.model_manager.onnxruntime)?;
            let cuda_runtimes = llama_assets
                .iter()
                .filter(|asset| asset.kind == LlamaCppAssetKind::CudaRuntime)
                .cloned()
                .collect::<Vec<_>>();
            Some(select_onnx_assets_for_hardware(
                &providers,
                &cuda_runtimes,
                &cuda_dependencies,
                &cudnn_runtimes,
                eligible_nvidia,
            )?)
        } else {
            None
        };
        let runtime_downloads =
            missing_runtime_downloads(project_root, llama_cpp.as_ref(), onnx.as_ref());
        let cpu_from_provider = bundled_cpu::provider_download_supplies_cpu_core(
            &config,
            onnx.as_ref(),
            &runtime_downloads,
        );
        let mut downloads =
            missing_base_bundled_downloads(project_root, &config, cpu_from_provider);
        downloads.extend(runtime_downloads);
        let marker_ready =
            runtime_marker_matches_plan(project_root, llama_cpp.as_ref(), onnx.as_ref());
        (llama_cpp, onnx, downloads, marker_ready)
    };
    Ok(RuntimePlan {
        llama_cpp,
        onnx,
        downloads,
        marker_ready,
        requirements,
        model_assets,
        local_models,
        blocking_error,
    })
}

#[cfg(any(not(target_os = "android"), test))]
fn required_local_model_vram_bytes(config: &AppConfig) -> u64 {
    config
        .active_native_model_assets()
        .into_iter()
        .filter_map(|key| xrtranslate_assets::ModelAssetId::from_config_key(&key))
        .map(|id| {
            xrtranslate_assets::manifest_for(id)
                .hardware
                .minimum_memory_bytes
        })
        .max()
        .unwrap_or(xrtranslate_assets::MANAGED_LOCAL_MODEL_MINIMUM_VRAM_BYTES)
}

/// Verifies that the immutable files selected by a runtime plan are also
/// represented by the exact marker the backend will consume. File presence
/// alone is insufficient: without this binding the backend cannot safely know
/// which CUDA/ONNX/cuDNN closure it is allowed to load.
#[cfg(any(not(target_os = "android"), test))]
fn runtime_marker_matches_plan(
    project_root: &Path,
    llama: Option<&RuntimeSelection>,
    onnx: Option<&OnnxRuntimeSelection>,
) -> bool {
    let layout = load_runtime_layout(project_root);
    if onnx
        .as_ref()
        .map_or(true, |o| o.backend != RuntimeBackend::Cuda)
        && !layout.onnx_cpu_core_library().is_file()
    {
        return false;
    }
    if llama.is_none() && onnx.is_none() {
        return true;
    }
    let Ok(Some(marker)) = load_native_runtime_selection(&layout) else {
        return false;
    };
    let resolved = layout.resolve_native_runtime_selection(&marker);

    if let Some(selection) = llama {
        let expected_backend = selection.backend;
        if marker.llama_cpp_backend != Some(expected_backend)
            || marker.vulkan_device != selection.vulkan_device
        {
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
        let expected_backend = selection.backend;
        if marker.backend != expected_backend || marker.onnx_backend != Some(expected_backend) {
            return false;
        }
        if selection.backend == RuntimeBackend::Cuda {
            let (Some(version), Some(provider), Some(cuda), Some(cuda_dependency), Some(cudnn)) = (
                selection.cuda_version.as_deref(),
                selection.provider.as_ref(),
                selection.cuda_runtime.as_ref(),
                selection.cuda_dependency.as_ref(),
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
            let Ok(cuda_dependency_preloads) =
                resolve_required_files(&cuda_directory, &cuda_dependency.required_files)
            else {
                return false;
            };
            expected_preloads.extend(cuda_dependency_preloads);
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

#[cfg(any(not(target_os = "android"), test))]
fn missing_base_bundled_downloads(
    project_root: &Path,
    config: &AppConfig,
    cpu_from_provider: bool,
) -> Vec<RuntimeDownload> {
    let layout = config.runtime_layout(project_root);
    let target = current_runtime_target();
    let mut downloads = Vec::new();

    if !file_has_expected_size(
        &layout.onnx_cpu_core_library(),
        expected_cpu_core_bytes(config),
    ) && !cpu_from_provider
    {
        if let Some(archive) = config
            .model_manager
            .onnxruntime
            .resolved_cpu_downloads()
            .into_iter()
            .find(|archive| archive.target.trim() == target)
        {
            downloads.push(RuntimeDownload {
                label: "ONNX Runtime (CPU)".to_owned(),
                archive_name: archive.name,
                bytes: archive.bytes,
            });
        }
    }

    for bundled in config.model_manager.resolved_bundled_models() {
        if bundled
            .target
            .as_deref()
            .map_or(true, |t| t.trim() == target)
        {
            let destination = project_root.join(&bundled.relative_path);
            if !file_has_expected_size(&destination, expected_bundled_model_bytes(&bundled)) {
                downloads.push(RuntimeDownload {
                    label: bundled.label,
                    archive_name: bundled.name,
                    bytes: bundled.bytes,
                });
            }
        }
    }

    downloads
}

fn missing_runtime_downloads(
    project_root: &Path,
    llama: Option<&RuntimeSelection>,
    onnx: Option<&OnnxRuntimeSelection>,
) -> Vec<RuntimeDownload> {
    let layout = load_runtime_layout(project_root);
    let mut missing = HashSet::new();
    let mut downloads = Vec::new();
    let mut add = |label: String, name: &str, size: u64| {
        if missing.insert(name.to_owned()) {
            downloads.push(RuntimeDownload {
                label,
                archive_name: name.to_owned(),
                bytes: size,
            });
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
                let label = match asset.kind {
                    LlamaCppAssetKind::ServerCpu => "llama.cpp (CPU)".to_owned(),
                    LlamaCppAssetKind::ServerVulkan => "llama.cpp (Vulkan)".to_owned(),
                    LlamaCppAssetKind::ServerCuda => format!(
                        "llama.cpp (CUDA {})",
                        asset.cuda_version.as_deref().unwrap_or_default()
                    ),
                    LlamaCppAssetKind::CudaRuntime => format!(
                        "CUDA {} runtime",
                        asset.cuda_version.as_deref().unwrap_or_default()
                    ),
                };
                add(label, &asset.name, asset.size);
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
                add(
                    format!(
                        "CUDA {} runtime",
                        asset.cuda_version.as_deref().unwrap_or_default()
                    ),
                    &asset.name,
                    asset.size,
                );
            }
        }
        if let Some(asset) = &selection.cuda_dependency {
            let ready = selection.cuda_version.as_deref().is_some_and(|version| {
                validate_required_files(
                    &layout.cuda_runtime_directory(version),
                    &asset.required_files,
                )
                .is_ok()
            });
            if !ready {
                let component = if asset
                    .required_files
                    .iter()
                    .any(|file| file.starts_with("cufft"))
                {
                    "cuFFT"
                } else if asset
                    .required_files
                    .iter()
                    .any(|file| file.starts_with("nvrtc"))
                {
                    "NVRTC"
                } else {
                    "CUDA dependency"
                };
                add(
                    format!("{component} (CUDA {})", asset.cuda_version),
                    &asset.name,
                    asset.size,
                );
            }
        }
        if let Some(asset) = &selection.cudnn {
            let ready = validate_required_files(
                &layout.cudnn_runtime_directory(&asset.cuda_version),
                &asset.required_files,
            )
            .is_ok();
            if !ready {
                add(
                    format!("cuDNN (CUDA {})", asset.cuda_version),
                    &asset.name,
                    asset.size,
                );
            }
        }
        if let Some(asset) = &selection.provider {
            let ready = validate_required_files(
                &layout.onnx_runtime_directory(&asset.cuda_version),
                &asset.required_files,
            )
            .is_ok();
            if !ready {
                add(
                    format!("ONNX Runtime (CUDA {})", asset.cuda_version),
                    &asset.name,
                    asset.size,
                );
            }
        }
    }
    downloads
}

fn missing_runtime_bytes(
    project_root: &Path,
    llama: Option<&RuntimeSelection>,
    onnx: Option<&OnnxRuntimeSelection>,
) -> u64 {
    missing_runtime_downloads(project_root, llama, onnx)
        .iter()
        .map(|download| download.bytes)
        .sum()
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

#[cfg(any(not(target_os = "android"), test))]
fn onnx_assets_from_config(config: &OnnxRuntimeConfig) -> Result<Vec<ManagedRuntimeAsset>, String> {
    if config.release.trim().is_empty() {
        return Err("model_manager.onnxruntime.release is empty in config.json.".into());
    }
    if config.downloads.is_empty() {
        return Err("model_manager.onnxruntime.downloads is empty in config.json.".into());
    }
    managed_runtime_assets_from_config("model_manager.onnxruntime.downloads", &config.downloads)
}

#[cfg(any(not(target_os = "android"), test))]
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

#[cfg(any(not(target_os = "android"), test))]
fn cuda_dependency_assets_from_config(
    config: &OnnxRuntimeConfig,
) -> Result<Vec<ManagedRuntimeAsset>, String> {
    if config.cuda_dependency_downloads.is_empty() {
        return Err(
            "model_manager.onnxruntime.cuda_dependency_downloads is empty in config.json.".into(),
        );
    }
    managed_runtime_assets_from_config(
        "model_manager.onnxruntime.cuda_dependency_downloads",
        &config.cuda_dependency_downloads,
    )
}

#[cfg(any(not(target_os = "android"), test))]
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
                    && !name.ends_with(".tar.gz") && !name.ends_with(".tgz"))
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
                archive_format: download.archive_format,
                target: download.target.trim().into(),
                cuda_version: cuda_version.into(),
                archive_directory: download.archive_directory.trim().into(),
                required_files: download.required_files.clone(),
            })
        })
        .collect()
}

#[cfg(any(not(target_os = "android"), test))]
fn select_onnx_assets_for_hardware(
    providers: &[ManagedRuntimeAsset],
    cuda_runtimes: &[ReleaseAsset],
    cuda_dependencies: &[ManagedRuntimeAsset],
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
            let cuda_dependency = cuda_dependencies.iter().find(|dependency| {
                dependency.target == target
                    && dependency.cuda_version.parse::<u16>().ok() == Some(runtime_version.0)
            })?;
            let cudnn = cudnn_runtimes.iter().find(|cudnn| {
                cudnn.target == target
                    && cudnn.cuda_version.parse::<u16>().ok() == Some(runtime_version.0)
            })?;
            Some((
                runtime_version,
                runtime.clone(),
                provider.clone(),
                cuda_dependency.clone(),
                cudnn.clone(),
            ))
        })
        .max_by_key(|(version, _, _, _, _)| *version);
    let Some((cuda_version, cuda_runtime, provider, cuda_dependency, cudnn)) = selected else {
        return Err(format!(
            "NVIDIA GPU {} supports CUDA {}, but no complete ONNX Runtime, CUDA math and cuDNN bundle is configured for {target}.",
            nvidia.gpu, nvidia.driver_cuda
        ));
    };
    Ok(OnnxRuntimeSelection {
        backend: RuntimeBackend::Cuda,
        provider: Some(provider),
        cuda_runtime: Some(cuda_runtime),
        cuda_dependency: Some(cuda_dependency),
        cudnn: Some(cudnn),
        cuda_version: Some(format_version(cuda_version)),
        fallback_reason: None,
    })
}

#[cfg(any(not(target_os = "android"), test))]
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
                    && !name.ends_with(".tar.gz") && !name.ends_with(".tgz"))
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
                archive_format: download.archive_format,
                archive_directory: download.archive_directory.trim().into(),
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

#[cfg(any(not(target_os = "android"), test))]
fn select_llama_assets(
    assets: &[ReleaseAsset],
    nvidia: Option<&NvidiaCuda>,
    amd: Option<&VulkanGpu>,
) -> Result<RuntimeSelection, String> {
    let cuda = select_cuda_assets(assets, nvidia);
    if cuda.is_ok() || amd.is_none() {
        return cuda;
    }
    let gpu = amd.unwrap();
    let asset = assets
        .iter()
        .find(|asset| {
            asset.target == current_runtime_target()
                && asset.kind == LlamaCppAssetKind::ServerVulkan
        })
        .ok_or_else(|| {
            format!(
                "No Vulkan llama.cpp runtime is configured for {}.",
                current_runtime_target()
            )
        })?;
    Ok(RuntimeSelection {
        assets: vec![asset.clone()],
        backend: RuntimeBackend::Vulkan,
        executable: asset.executable.clone(),
        vulkan_device: Some(gpu.index),
        fallback_reason: None,
    })
}

#[cfg(any(not(target_os = "android"), test))]
fn select_cuda_assets(
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
            vulkan_device: None,
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
#[cfg(any(not(target_os = "android"), test))]
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
            LlamaCppAssetKind::ServerVulkan => vec!["ggml.dll".into(), "ggml-vulkan.dll".into()],
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

#[cfg(any(not(target_os = "android"), test))]
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

#[cfg(any(not(target_os = "android"), test))]
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

#[cfg(any(not(target_os = "android"), test))]
fn cuda_supports_compute_capability(
    cuda_version: (u16, u16),
    compute_capability: (u16, u16),
) -> bool {
    cuda_version.0 < 13 || compute_capability >= TURING_COMPUTE_CAPABILITY
}

#[cfg(any(not(target_os = "android"), test))]
fn format_version(version: (u16, u16)) -> String {
    format!("{}.{}", version.0, version.1)
}

#[cfg(any(not(target_os = "android"), test))]
fn minimum_cuda_for_compute_capability(capability: (u16, u16)) -> (u16, u16) {
    if capability.0 >= 10 {
        BLACKWELL_MINIMUM_CUDA
    } else {
        (0, 0)
    }
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
