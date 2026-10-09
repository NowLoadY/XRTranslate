//! Background native-model installation for the desktop client.
//!
//! The installer intentionally owns its own worker thread and Tokio runtime:
//! model downloads and installation can take minutes and must never
//! run in eframe's UI thread.

use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    thread,
};
use xrtranslate_assets::{
    DownloadProgress, ModelAssetId, ModelAssetsConfig, ModelCapability, ModelHardwareRequirements,
    ModelLevel, NativeModelInstaller, ResolvedModelAssets, clear_model_staging, manifest_for,
    manifests_for_capability, remove_model_asset, tier_default_manifest,
};
use xrtranslate_config::AppConfig;
use xrtranslate_download::{DownloadCancellation, DownloadSource};

#[derive(Clone, Debug)]
pub enum NativeModelTaskState {
    Idle,
    Discovering,
    Detected,
    Installing {
        asset_id: ModelAssetId,
        relative_path: Option<String>,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Installed,
    Failed(String),
}

/// A model package exposed by the provider objects selected in `config.json`.
#[derive(Clone, Debug)]
pub struct NativeModelPackage {
    pub id: ModelAssetId,
    pub label: &'static str,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub provider: &'static str,
    pub capability: ModelCapability,
    pub level: ModelLevel,
    pub languages: &'static [&'static str],
    pub hardware: ModelHardwareRequirements,
}

/// Read-only projection of a serial model-install batch for the download UI.
/// The shared asset installer still owns each package transaction; this host
/// snapshot only explains queue order and aggregate progress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelBatchSnapshot {
    pub current_asset_id: Option<ModelAssetId>,
    pub current_relative_path: Option<String>,
    pub completed_packages: usize,
    pub total_packages: usize,
    pub queued_packages: Vec<ModelAssetId>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub failed_asset_id: Option<ModelAssetId>,
}

impl NativeModelTaskState {
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        matches!(self, Self::Discovering | Self::Installing { .. })
    }
}

#[derive(Clone, Copy, Debug)]
enum NativeModelTask {
    Discover,
    Install(ModelAssetId),
}

#[derive(Debug)]
enum NativeModelTaskEvent {
    Progress(DownloadProgress),
    Finished(NativeModelTaskResult),
}

#[derive(Debug)]
enum NativeModelTaskResult {
    Detected(Vec<ModelAssetId>),
    Installed { asset_id: ModelAssetId },
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
struct ModelInstallBatch {
    project_root: PathBuf,
    package_ids: Vec<ModelAssetId>,
    queued: VecDeque<ModelAssetId>,
    completed: Vec<ModelAssetId>,
    completed_bytes: u64,
    total_bytes: u64,
    failed_asset_id: Option<ModelAssetId>,
}

/// Coordinates one native model worker and a serial, de-duplicated install
/// queue. Results are polled by the UI, while filesystem checks and
/// network transfer stay on the worker.
pub struct NativeModelTaskManager {
    state: NativeModelTaskState,
    events: Option<Receiver<NativeModelTaskEvent>>,
    proxy_url: Option<String>,
    use_mirror: bool,
    rediscover_after_current: bool,
    cancellation: Option<DownloadCancellation>,
    active_task: Option<(PathBuf, NativeModelTask)>,
    restart_after_source_switch: bool,
    source_switch_cleanup: Vec<ModelAssetId>,
    install_batch: Option<ModelInstallBatch>,
    known_ready: HashSet<ModelAssetId>,
}

impl Default for NativeModelTaskManager {
    fn default() -> Self {
        Self {
            state: NativeModelTaskState::Idle,
            events: None,
            proxy_url: None,
            use_mirror: false,
            rediscover_after_current: false,
            cancellation: None,
            active_task: None,
            restart_after_source_switch: false,
            source_switch_cleanup: Vec::new(),
            install_batch: None,
            known_ready: HashSet::new(),
        }
    }
}

impl NativeModelTaskManager {
    pub fn set_proxy_url(&mut self, proxy_url: &str) {
        self.proxy_url = (!proxy_url.trim().is_empty()).then(|| proxy_url.trim().to_owned());
    }

    /// Switches the global model source. An active transfer is cooperatively
    /// stopped, its package staging is removed after the worker releases it,
    /// and the same package is restarted through the newly selected source.
    pub fn switch_download_source(
        &mut self,
        project_root: PathBuf,
        use_mirror: bool,
    ) -> Result<(), String> {
        if self.use_mirror == use_mirror {
            return Ok(());
        }
        self.use_mirror = use_mirror;
        if matches!(self.active_task, Some((_, NativeModelTask::Install(_)))) && self.is_busy() {
            self.restart_after_source_switch = true;
            self.source_switch_cleanup = self.install_batch.as_ref().map_or_else(
                || {
                    self.active_task
                        .iter()
                        .filter_map(|(_, task)| match task {
                            NativeModelTask::Install(id) => Some(*id),
                            NativeModelTask::Discover => None,
                        })
                        .collect()
                },
                |batch| batch.package_ids.clone(),
            );
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            }
            return Ok(());
        }
        let ids = catalog_model_packages(&project_root)?
            .into_iter()
            .map(|package| package.id)
            .collect::<Vec<_>>();
        clear_model_staging_for(&project_root, &ids)
    }

    #[must_use]
    pub const fn use_mirror(&self) -> bool {
        self.use_mirror
    }
    #[must_use]
    pub fn state(&self) -> &NativeModelTaskState {
        &self.state
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.state.is_busy()
    }

    /// Adds packages to the current serial batch without losing rapid clicks.
    /// Duplicate, active, completed, and already-present package ids are
    /// ignored. A failed batch is rebuilt in caller order so retrying "all"
    /// resumes at the failed package before the remaining packages.
    pub fn enqueue_many(
        &mut self,
        project_root: PathBuf,
        asset_ids: impl IntoIterator<Item = ModelAssetId>,
    ) -> Result<(), String> {
        if matches!(self.active_task, Some((_, NativeModelTask::Discover))) && self.is_busy() {
            return Err("Wait for model discovery before starting downloads.".into());
        }
        if let Some((active_root, NativeModelTask::Install(_))) = &self.active_task
            && *active_root != project_root
        {
            return Err("The active model batch belongs to a different project root.".into());
        }

        let requested = asset_ids
            .into_iter()
            .filter(|id| !self.known_ready.contains(id))
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Ok(());
        }

        let restart_failed_batch =
            !self.is_busy() && matches!(self.state, NativeModelTaskState::Failed(_));
        if restart_failed_batch {
            self.install_batch = None;
        }
        let batch = self.install_batch.get_or_insert_with(|| ModelInstallBatch {
            project_root: project_root.clone(),
            package_ids: Vec::new(),
            queued: VecDeque::new(),
            completed: Vec::new(),
            completed_bytes: 0,
            total_bytes: 0,
            failed_asset_id: None,
        });
        if batch.project_root != project_root {
            return Err("The queued model batch belongs to a different project root.".into());
        }
        for id in requested {
            if batch.package_ids.contains(&id) {
                continue;
            }
            batch.package_ids.push(id);
            batch.queued.push_back(id);
            batch.total_bytes = batch.total_bytes.saturating_add(model_download_bytes(id));
        }
        batch.failed_asset_id = None;

        if !self.is_busy() {
            self.start_next_queued()?;
        }
        Ok(())
    }

    pub fn delete(
        &mut self,
        project_root: &std::path::Path,
        asset_id: ModelAssetId,
    ) -> Result<(), String> {
        if self.is_busy() {
            return Err("Wait for the current model task before deleting a model resource.".into());
        }
        let assets = load_assets(project_root)?;
        remove_model_asset(&assets, asset_id).map_err(|error| error.to_string())?;
        self.state = NativeModelTaskState::Idle;
        self.events = None;
        self.active_task = None;
        self.known_ready.remove(&asset_id);
        self.install_batch = None;
        Ok(())
    }

    /// Starts a one-time, background presence scan for the configured model
    /// packages, checking required files and their sizes.
    pub fn discover_existing(&mut self, project_root: PathBuf) -> Result<(), String> {
        self.start(project_root, NativeModelTask::Discover)
    }

    pub fn invalidate_discovery(&mut self) {
        if self.is_busy() {
            self.rediscover_after_current = true;
        } else {
            self.state = NativeModelTaskState::Idle;
            self.events = None;
        }
    }

    #[must_use]
    pub fn needs_discovery(&self) -> bool {
        matches!(
            self.state,
            NativeModelTaskState::Idle | NativeModelTaskState::Installed
        )
    }

    /// Whether the latest background scan or install found all required files.
    #[must_use]
    pub fn is_model_ready(&self, asset_id: ModelAssetId) -> bool {
        self.known_ready.contains(&asset_id)
    }

    #[must_use]
    pub fn batch_snapshot(&self) -> Option<NativeModelBatchSnapshot> {
        let batch = self.install_batch.as_ref()?;
        let (current_asset_id, current_relative_path, current_downloaded) = match &self.state {
            NativeModelTaskState::Installing {
                asset_id,
                relative_path,
                downloaded_bytes,
                ..
            } => (Some(*asset_id), relative_path.clone(), *downloaded_bytes),
            _ => (batch.failed_asset_id, None, 0),
        };
        Some(NativeModelBatchSnapshot {
            current_asset_id,
            current_relative_path,
            completed_packages: batch.completed.len(),
            total_packages: batch.package_ids.len(),
            queued_packages: batch.queued.iter().copied().collect(),
            downloaded_bytes: batch.completed_bytes.saturating_add(current_downloaded),
            total_bytes: batch.total_bytes,
            failed_asset_id: batch.failed_asset_id,
        })
    }

    /// Applies completed worker events. Call this once every UI frame.
    pub fn poll(&mut self) {
        let Some(events) = self.events.clone() else {
            return;
        };

        let mut finished = None;
        loop {
            match events.try_recv() {
                Ok(NativeModelTaskEvent::Progress(progress)) => {
                    self.state = NativeModelTaskState::Installing {
                        asset_id: progress.asset_id,
                        relative_path: Some(progress.relative_path.to_owned()),
                        downloaded_bytes: progress.downloaded_bytes,
                        total_bytes: progress.total_bytes,
                    };
                }
                Ok(NativeModelTaskEvent::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state = NativeModelTaskState::Failed(
                        "The native model worker stopped before reporting a result.".into(),
                    );
                    if let Some((_, NativeModelTask::Install(id))) = self.active_task {
                        if let Some(batch) = &mut self.install_batch {
                            batch.failed_asset_id = Some(id);
                        }
                    }
                    finished = Some(NativeModelTaskResult::Failed(
                        "The native model worker stopped before reporting a result.".into(),
                    ));
                    break;
                }
            }
        }
        let Some(result) = finished else {
            return;
        };

        self.events = None;
        self.cancellation = None;
        let active_task = self.active_task.take();
        match result {
            NativeModelTaskResult::Detected(ready) => {
                self.known_ready = ready.into_iter().collect();
                self.state = NativeModelTaskState::Detected;
            }
            NativeModelTaskResult::Installed { asset_id } => {
                self.known_ready.insert(asset_id);
                if let Some(batch) = &mut self.install_batch {
                    if !batch.completed.contains(&asset_id) {
                        batch.completed.push(asset_id);
                        batch.completed_bytes = batch
                            .completed_bytes
                            .saturating_add(model_download_bytes(asset_id));
                    }
                    batch.failed_asset_id = None;
                }
                self.state = NativeModelTaskState::Installed;
                if let Err(error) = self.start_next_queued() {
                    self.state = NativeModelTaskState::Failed(error);
                }
            }
            NativeModelTaskResult::Cancelled if self.restart_after_source_switch => {
                self.restart_after_source_switch = false;
                let Some((active_root, _)) = active_task.as_ref() else {
                    self.state = NativeModelTaskState::Failed(
                        "The cancelled model source switch lost its project root.".into(),
                    );
                    return;
                };
                if let Err(error) = clear_model_staging_for(
                    active_root,
                    &std::mem::take(&mut self.source_switch_cleanup),
                ) {
                    self.state = NativeModelTaskState::Failed(error);
                    return;
                }
                if let Some((project_root, task)) = active_task
                    && let Err(error) = self.start(project_root, task)
                {
                    self.state = NativeModelTaskState::Failed(error);
                }
                return;
            }
            NativeModelTaskResult::Cancelled => {
                self.state = NativeModelTaskState::Idle;
            }
            NativeModelTaskResult::Failed(error) => {
                if let Some((_, NativeModelTask::Install(id))) = active_task
                    && let Some(batch) = &mut self.install_batch
                {
                    batch.failed_asset_id = Some(id);
                }
                self.state = NativeModelTaskState::Failed(error);
            }
        }
        if self.rediscover_after_current && !self.is_busy() {
            self.rediscover_after_current = false;
            self.state = NativeModelTaskState::Idle;
        }
    }

    fn start_next_queued(&mut self) -> Result<(), String> {
        let next = self
            .install_batch
            .as_mut()
            .and_then(|batch| batch.queued.pop_front());
        let Some(asset_id) = next else {
            return Ok(());
        };
        let project_root = self
            .install_batch
            .as_ref()
            .expect("queued package has a batch")
            .project_root
            .clone();
        self.start(project_root, NativeModelTask::Install(asset_id))
    }

    fn start(&mut self, project_root: PathBuf, task: NativeModelTask) -> Result<(), String> {
        if self.is_busy() {
            return Err("A native model task is already running.".into());
        }

        let (event_tx, event_rx) = unbounded();
        let proxy_url = self.proxy_url.clone();
        let source = DownloadSource::from_mirror_enabled(self.use_mirror);
        let cancellation =
            matches!(task, NativeModelTask::Install(_)).then(DownloadCancellation::default);
        let worker_cancellation = cancellation.clone();
        let worker_root = project_root.clone();
        thread::Builder::new()
            .name("native-model-installer".into())
            .spawn(move || {
                run_task(
                    worker_root,
                    task,
                    event_tx,
                    proxy_url,
                    source,
                    worker_cancellation,
                )
            })
            .map_err(|error| format!("Cannot start native model worker: {error}"))?;
        self.state = match task {
            NativeModelTask::Discover => NativeModelTaskState::Discovering,
            NativeModelTask::Install(asset_id) => NativeModelTaskState::Installing {
                asset_id,
                relative_path: None,
                downloaded_bytes: 0,
                total_bytes: 0,
            },
        };
        self.events = Some(event_rx);
        self.cancellation = cancellation;
        self.active_task = Some((project_root, task));
        Ok(())
    }
}

fn run_task(
    project_root: PathBuf,
    task: NativeModelTask,
    event_tx: crossbeam_channel::Sender<NativeModelTaskEvent>,
    proxy_url: Option<String>,
    source: DownloadSource,
    cancellation: Option<DownloadCancellation>,
) {
    let result = match task {
        NativeModelTask::Discover => discover_models(project_root),
        NativeModelTask::Install(asset_id) => install_model(
            project_root,
            asset_id,
            &event_tx,
            proxy_url.as_deref(),
            source,
            cancellation.expect("install tasks have a cancellation token"),
        ),
    };
    let _ = event_tx.send(NativeModelTaskEvent::Finished(result));
}

fn discover_models(project_root: PathBuf) -> NativeModelTaskResult {
    match load_assets(&project_root) {
        Ok(assets) => {
            let ready = assets
                .installed_assets()
                .map(|asset| asset.manifest().id)
                .collect::<Vec<_>>();
            NativeModelTaskResult::Detected(ready)
        }
        Err(error) => NativeModelTaskResult::Failed(error),
    }
}

fn install_model(
    project_root: PathBuf,
    asset_id: ModelAssetId,
    event_tx: &crossbeam_channel::Sender<NativeModelTaskEvent>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
) -> NativeModelTaskResult {
    let cancellation_observer = cancellation.clone();
    let assets = match load_assets(&project_root) {
        Ok(assets) => assets,
        Err(error) => return NativeModelTaskResult::Failed(error),
    };
    let installer = match NativeModelInstaller::with_download_source_and_cancellation(
        assets,
        proxy_url,
        source,
        cancellation,
    ) {
        Ok(installer) => installer,
        Err(error) => return NativeModelTaskResult::Failed(error.to_string()),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return NativeModelTaskResult::Failed(format!(
                "Cannot initialize model installer runtime: {error}"
            ));
        }
    };
    let progress_tx = event_tx.clone();
    let result = runtime.block_on(installer.install(asset_id, move |progress| {
        let _ = progress_tx.send(NativeModelTaskEvent::Progress(progress));
    }));

    match result {
        Ok(_) => NativeModelTaskResult::Installed { asset_id },
        Err(error) if error.is_cancelled() || cancellation_observer.is_cancelled() => {
            let cleanup = load_assets(&project_root).and_then(|assets| {
                clear_model_staging(&assets, asset_id).map_err(|error| error.to_string())
            });
            match cleanup {
                Ok(()) => NativeModelTaskResult::Cancelled,
                Err(error) => NativeModelTaskResult::Failed(error),
            }
        }
        Err(error) => NativeModelTaskResult::Failed(error.to_string()),
    }
}

pub fn model_asset_is_present(
    project_root: &std::path::Path,
    asset_id: ModelAssetId,
) -> Result<bool, String> {
    let assets = load_assets(project_root)?;
    Ok(assets.asset(asset_id).check().is_empty())
}

fn load_assets(project_root: &std::path::Path) -> Result<ResolvedModelAssets, String> {
    let config = load_config(project_root)?;
    configured_assets(&config, project_root)
}

pub fn configured_model_packages(
    project_root: &std::path::Path,
) -> Result<Vec<NativeModelPackage>, String> {
    let config = load_config(project_root)?;
    let assets = configured_assets(&config, project_root)?;
    config
        .active_native_model_assets()
        .into_iter()
        .map(|key| {
            let id = ModelAssetId::from_config_key(&key).ok_or_else(|| {
                format!("Unknown model_asset in the active provider configuration: {key}")
            })?;
            let manifest = assets.asset(id).manifest();
            Ok(package_from_manifest(manifest))
        })
        .collect()
}

/// Enumerates the complete manifest catalogue using the configured directory
/// overrides. UI resource management can therefore expose installed packages
/// that are no longer selected by a provider without naming model families.
pub fn catalog_model_packages(
    project_root: &std::path::Path,
) -> Result<Vec<NativeModelPackage>, String> {
    let assets = load_assets(project_root)?;
    Ok(assets
        .catalog_assets()
        .map(|asset| package_from_manifest(asset.manifest()))
        .collect())
}

fn package_from_manifest(manifest: &xrtranslate_assets::ModelAssetManifest) -> NativeModelPackage {
    NativeModelPackage {
        id: manifest.id,
        label: manifest.label,
        provider: manifest.provider,
        capability: manifest.capability,
        level: manifest.level,
        languages: manifest.languages,
        hardware: manifest.hardware,
        download_bytes: manifest.download_bytes(),
        installed_bytes: manifest.installed_bytes(),
    }
}

fn model_download_bytes(id: ModelAssetId) -> u64 {
    manifest_for(id).download_bytes()
}

fn clear_model_staging_for(
    project_root: &std::path::Path,
    ids: &[ModelAssetId],
) -> Result<(), String> {
    let assets = load_assets(project_root)?;
    for id in ids.iter().copied() {
        clear_model_staging(&assets, id).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn model_packages_for_provider(
    provider: &str,
    capability: ModelCapability,
) -> Vec<NativeModelPackage> {
    manifests_for_capability(capability)
        .filter(|manifest| manifest.provider == provider)
        .map(package_from_manifest)
        .collect()
}

/// Select the default model for the best tier supported by the reported GPU.
/// The tier default is explicit, so adding another choice does not change it.
pub fn default_model_for_vram(
    provider: &str,
    capability: ModelCapability,
    memory_bytes: u64,
) -> Option<ModelAssetId> {
    [ModelLevel::Normal, ModelLevel::Small]
        .into_iter()
        .filter_map(|level| tier_default_manifest(provider, capability, level))
        .find(|manifest| memory_bytes >= manifest.hardware.minimum_memory_bytes)
        .or_else(|| {
            manifests_for_capability(capability)
                .filter(|manifest| {
                    manifest.provider == provider
                        && memory_bytes >= manifest.hardware.minimum_memory_bytes
                })
                .min_by_key(|manifest| manifest.estimated_vram_bytes)
        })
        .map(|manifest| manifest.id)
}

pub fn set_model_asset(
    project_root: &std::path::Path,
    capability: ModelCapability,
    asset_id: ModelAssetId,
) -> Result<(), String> {
    if capability.allows_multiple_assets() {
        return Err(
            "Composable model packages must be selected through the provider asset list.".into(),
        );
    }
    let path = project_root.join("config.json");
    let mut document = xrtranslate_config::load_user_config_document(&path, project_root)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let section_name = match capability {
        ModelCapability::Asr => "asr",
        ModelCapability::Translation => "translation",
        ModelCapability::Tts => "tts",
        ModelCapability::Ocr => "ocr",
    };
    let section = document
        .get_mut(section_name)
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("Missing {section_name} configuration."))?;
    let provider = section
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Missing {section_name}.provider."))?
        .to_owned();
    let manifest = manifest_for(asset_id);
    if manifest.provider != provider || manifest.capability != capability {
        return Err(format!(
            "Model {} does not belong to provider {provider} for {capability:?}.",
            asset_id
        ));
    }
    let provider_config = section
        .get_mut("providers")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|providers| providers.get_mut(&provider))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("Missing {section_name}.providers.{provider}."))?;
    provider_config.insert(
        "model_asset".into(),
        serde_json::Value::String(manifest.id.as_str().into()),
    );
    if let Some(runtime) = manifest.runtime {
        provider_config.insert(
            "transport".into(),
            serde_json::Value::String(runtime.transport().into()),
        );
    }
    if !capability.allows_multiple_assets() && provider_config.contains_key("model_assets") {
        provider_config.insert(
            "model_assets".into(),
            serde_json::Value::Array(vec![serde_json::Value::String(manifest.id.as_str().into())]),
        );
    }
    xrtranslate_config::AppConfig::from_value(document.clone())
        .map_err(|error| format!("Invalid configuration: {error}"))?;
    xrtranslate_config::save_user_config_document(&path, project_root, &document)
}

fn configured_assets(
    config: &AppConfig,
    project_root: &std::path::Path,
) -> Result<ResolvedModelAssets, String> {
    let mut assets = ModelAssetsConfig::with_directory_overrides(
        config.model_manager.models_directory.clone(),
        config.model_manager.qwen3_asr_gguf_directory.clone(),
        config.model_manager.hunyuan_mt_gguf_directory.clone(),
    );
    for key in config.active_native_model_assets() {
        let id = ModelAssetId::from_config_key(&key).ok_or_else(|| {
            format!("Unknown model_asset in active provider configuration: {key}")
        })?;
        assets.select_asset(id);
    }
    Ok(assets.resolve_selected(project_root))
}

fn load_config(project_root: &std::path::Path) -> Result<AppConfig, String> {
    let config_path = project_root.join("config.json");
    AppConfig::from_path_with_user_config(&config_path, project_root)
        .map_err(|error| format!("Cannot read {}: {error}", config_path.display()))
}
