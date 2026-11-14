//! Shared background installation for built-in and plugin model resources.
//!
//! The installer intentionally owns its own worker thread and Tokio runtime:
//! model downloads and installation can take minutes and must never
//! run in eframe's UI thread.

use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    thread,
};
use xrtranslate_assets::{
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelHardwareRequirements, ModelLevel,
    ModelPackage, ResolvedModelAssets, manifest_for, manifests_for_capability,
    tier_default_manifest,
};
use xrtranslate_config::AppConfig;
use xrtranslate_download::{DownloadCancellation, DownloadClient, DownloadSource};

/// A host-owned resource or a model declared by an independently built plugin.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ResourceId {
    BuiltIn(ModelAssetId),
    Plugin { plugin: String, model: String },
}
impl From<ModelAssetId> for ResourceId {
    fn from(id: ModelAssetId) -> Self {
        Self::BuiltIn(id)
    }
}
impl From<&ResourceId> for ResourceId {
    fn from(id: &ResourceId) -> Self {
        id.clone()
    }
}
impl ResourceId {
    fn download_bytes(&self, plugins: &HashMap<ResourceId, RegisteredModel>) -> u64 {
        match self {
            Self::BuiltIn(id) => model_download_bytes(*id),
            Self::Plugin { .. } => plugins
                .get(self)
                .map_or(0, |model| model.package.download_bytes()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredModel {
    pub package: ModelPackage,
    pub directory: PathBuf,
}

#[derive(Debug)]
struct ResourceProgress {
    asset_id: ResourceId,
    relative_path: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}

#[derive(Clone, Debug)]
pub enum NativeModelTaskState {
    Idle,
    Discovering,
    Detected,
    Installing {
        asset_id: ResourceId,
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
    pub current_asset_id: Option<ResourceId>,
    pub current_relative_path: Option<String>,
    pub completed_packages: usize,
    pub total_packages: usize,
    pub queued_packages: Vec<ResourceId>,
    pub paused_packages: Vec<ResourceId>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub failed_asset_id: Option<ResourceId>,
}

impl NativeModelTaskState {
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        matches!(self, Self::Discovering | Self::Installing { .. })
    }
}

#[derive(Clone, Debug)]
enum NativeModelTask {
    Discover,
    Install(ResourceId),
    Import(ResourceId, PathBuf),
}

#[derive(Debug)]
enum NativeModelTaskEvent {
    Progress(ResourceProgress),
    Finished(NativeModelTaskResult),
}

#[derive(Debug)]
enum NativeModelTaskResult {
    Detected(Vec<ResourceId>),
    Installed { asset_id: ResourceId },
    Cancelled,
    SourceFailed(String),
    Failed(String),
}

#[derive(Debug)]
struct ModelInstallBatch {
    project_root: PathBuf,
    package_ids: Vec<ResourceId>,
    queued: VecDeque<ResourceId>,
    completed: Vec<ResourceId>,
    completed_bytes: u64,
    total_bytes: u64,
    failed_asset_id: Option<ResourceId>,
}

/// Coordinates one model worker and a serial, de-duplicated install
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
    paused: HashSet<ResourceId>,
    download_progress: HashMap<ResourceId, (u64, u64)>,
    install_batch: Option<ModelInstallBatch>,
    known_ready: HashSet<ResourceId>,
    plugin_models: HashMap<ResourceId, RegisteredModel>,
    changed_resources: Vec<ResourceId>,
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
            paused: HashSet::new(),
            download_progress: HashMap::new(),
            install_batch: None,
            known_ready: HashSet::new(),
            plugin_models: HashMap::new(),
            changed_resources: Vec::new(),
        }
    }
}

impl NativeModelTaskManager {
    pub fn set_plugin_models(&mut self, models: HashMap<ResourceId, RegisteredModel>) {
        if self.plugin_models != models {
            // Retire pending work if a plugin is removed or changes its resource.
            let changed: Vec<_> = self
                .plugin_models
                .iter()
                .filter(|(id, model)| models.get(*id) != Some(*model))
                .map(|(id, _)| id.clone())
                .collect();
            for id in changed {
                let _ = self.pause(&id);
            }
            self.known_ready
                .retain(|id| !matches!(id, ResourceId::Plugin { .. }));
            self.plugin_models = models;
            self.invalidate_discovery();
        }
    }

    pub fn take_changed_resources(&mut self) -> Vec<ResourceId> {
        std::mem::take(&mut self.changed_resources)
    }

    pub fn set_proxy_url(&mut self, proxy_url: &str) {
        self.proxy_url = (!proxy_url.trim().is_empty()).then(|| proxy_url.trim().to_owned());
    }

    /// Switches the global model source. An active transfer is cooperatively
    /// stopped and resumed through the newly selected source. Both routes
    /// address the same immutable package, so downloaded bytes are retained.
    pub fn switch_download_source(
        &mut self,
        _project_root: PathBuf,
        use_mirror: bool,
    ) -> Result<(), String> {
        if self.use_mirror == use_mirror {
            return Ok(());
        }
        self.use_mirror = use_mirror;
        if let Some((_, NativeModelTask::Install(id))) = self.active_task.as_ref()
            && self.is_busy()
            && !self.is_model_paused(id)
        {
            self.restart_after_source_switch = true;
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            }
            return Ok(());
        }
        Ok(())
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

    pub fn is_importing(&self, id: impl Into<ResourceId>) -> bool {
        let id = id.into();
        matches!(&self.active_task, Some((_, NativeModelTask::Import(current, _))) if *current == id)
    }

    pub fn import(
        &mut self,
        project_root: PathBuf,
        id: impl Into<ResourceId>,
        source: PathBuf,
    ) -> Result<(), String> {
        self.start(project_root, NativeModelTask::Import(id.into(), source))
    }

    pub fn cancel_import(&self) {
        if matches!(self.active_task, Some((_, NativeModelTask::Import(..))))
            && let Some(cancellation) = &self.cancellation
        {
            cancellation.cancel();
        }
    }

    #[must_use]
    pub fn is_model_paused(&self, asset_id: impl Into<ResourceId>) -> bool {
        let asset_id = asset_id.into();
        self.paused.contains(&asset_id)
    }

    #[must_use]
    pub fn is_model_queued(&self, asset_id: impl Into<ResourceId>) -> bool {
        let asset_id = asset_id.into();
        self.install_batch
            .as_ref()
            .is_some_and(|batch| batch.queued.contains(&asset_id))
    }

    #[must_use]
    pub fn model_download_progress(&self, asset_id: impl Into<ResourceId>) -> Option<(u64, u64)> {
        let asset_id = asset_id.into();
        if let NativeModelTaskState::Installing {
            asset_id: current,
            downloaded_bytes,
            total_bytes,
            ..
        } = &self.state
            && *current == asset_id
        {
            return Some((*downloaded_bytes, *total_bytes));
        }
        self.download_progress.get(&asset_id).copied()
    }

    /// Pauses just this package, preserving its partial download. Other
    /// queued packages continue after the current worker releases its files.
    pub fn pause(&mut self, asset_id: impl Into<ResourceId>) -> Result<(), String> {
        let asset_id = asset_id.into();
        if self.is_model_paused(&asset_id) || self.known_ready.contains(&asset_id) {
            return Ok(());
        }
        let active = matches!(
            &self.active_task,
            Some((_, NativeModelTask::Install(current))) if *current == asset_id
        );
        if !active && !self.is_model_queued(&asset_id) {
            return Err("The model is not downloading or queued.".into());
        }
        let progress = self
            .model_download_progress(&asset_id)
            .unwrap_or((0, asset_id.download_bytes(&self.plugin_models)));
        self.download_progress.insert(asset_id.clone(), progress);
        self.paused.insert(asset_id.clone());
        if let Some(batch) = &mut self.install_batch {
            batch.queued.retain(|id| *id != asset_id);
        }
        if active {
            self.restart_after_source_switch = false;
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            }
        }
        Ok(())
    }

    /// Adds packages to the current serial batch without losing rapid clicks.
    /// Duplicate, active, completed, and already-present package ids are
    /// ignored. A failed batch is rebuilt in caller order so retrying "all"
    /// resumes at the failed package before the remaining packages.
    pub fn enqueue_many(
        &mut self,
        project_root: PathBuf,
        asset_ids: impl IntoIterator<Item = impl Into<ResourceId>>,
    ) -> Result<(), String> {
        if let Some((active_root, _)) = &self.active_task
            && *active_root != project_root
        {
            return Err("The active model batch belongs to a different project root.".into());
        }

        let requested = asset_ids
            .into_iter()
            .map(Into::into)
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
            let resuming = self.paused.remove(&id);
            if batch.package_ids.contains(&id) {
                if resuming && !batch.queued.contains(&id) {
                    batch.queued.push_back(id);
                }
                continue;
            }
            batch.package_ids.push(id.clone());
            batch.queued.push_back(id.clone());
            batch.total_bytes = batch
                .total_bytes
                .saturating_add(id.download_bytes(&self.plugin_models));
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
        asset_id: impl Into<ResourceId>,
    ) -> Result<(), String> {
        let asset_id = asset_id.into();
        if self.is_busy() {
            return Err("Wait for the current model task before deleting a model resource.".into());
        }
        let model = resolve_model(project_root, &asset_id, &self.plugin_models)?;
        model
            .package
            .remove(&model.directory)
            .map_err(|error| error.to_string())?;
        self.state = NativeModelTaskState::Idle;
        self.events = None;
        self.active_task = None;
        self.known_ready.remove(&asset_id);
        self.paused.remove(&asset_id);
        self.download_progress.remove(&asset_id);
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
    pub fn is_model_ready(&self, asset_id: impl Into<ResourceId>) -> bool {
        let asset_id = asset_id.into();
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
            } => (
                Some(asset_id.clone()),
                relative_path.clone(),
                *downloaded_bytes,
            ),
            _ => (batch.failed_asset_id.clone(), None, 0),
        };
        Some(NativeModelBatchSnapshot {
            current_asset_id: current_asset_id.clone(),
            current_relative_path,
            completed_packages: batch.completed.len(),
            total_packages: batch.package_ids.len(),
            queued_packages: batch.queued.iter().cloned().collect(),
            paused_packages: batch
                .package_ids
                .iter()
                .filter(|id| self.is_model_paused(*id))
                .cloned()
                .collect(),
            downloaded_bytes: batch
                .completed_bytes
                .saturating_add(current_downloaded)
                .saturating_add(
                    batch
                        .package_ids
                        .iter()
                        .filter(|id| Some(*id) != current_asset_id.as_ref())
                        .filter(|id| !batch.completed.contains(id))
                        .filter_map(|id| self.download_progress.get(id))
                        .map(|(downloaded, _)| *downloaded)
                        .sum::<u64>(),
                ),
            total_bytes: batch.total_bytes,
            failed_asset_id: batch.failed_asset_id.clone(),
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
                    self.download_progress.insert(
                        progress.asset_id.clone(),
                        (progress.downloaded_bytes, progress.total_bytes),
                    );
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
                    if let Some((_, NativeModelTask::Install(id))) = &self.active_task {
                        if let Some(batch) = &mut self.install_batch {
                            batch.failed_asset_id = Some(id.clone());
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
        // A failure may already be queued when the user pauses or switches
        // source. Its delayed delivery must not override that newer intent.
        let result = if matches!(
            result,
            NativeModelTaskResult::Failed(_) | NativeModelTaskResult::SourceFailed(_)
        ) && (self.restart_after_source_switch
            || matches!(&active_task, Some((_, NativeModelTask::Install(id))) if self.is_model_paused(id)))
        {
            NativeModelTaskResult::Cancelled
        } else {
            result
        };
        match result {
            NativeModelTaskResult::Detected(ready) => {
                self.known_ready = ready.into_iter().collect();
                self.paused.retain(|id| !self.known_ready.contains(id));
                self.download_progress
                    .retain(|id, _| !self.known_ready.contains(id));
                self.state = NativeModelTaskState::Detected;
                if let Err(error) = self.start_next_queued() {
                    self.state = NativeModelTaskState::Failed(error);
                }
            }
            NativeModelTaskResult::Installed { asset_id } => {
                self.restart_after_source_switch = false;
                self.known_ready.insert(asset_id.clone());
                self.changed_resources.push(asset_id.clone());
                self.paused.remove(&asset_id);
                self.download_progress.remove(&asset_id);
                if let Some(batch) = &mut self.install_batch {
                    if batch.package_ids.contains(&asset_id) && !batch.completed.contains(&asset_id)
                    {
                        batch.completed.push(asset_id.clone());
                        batch.completed_bytes = batch
                            .completed_bytes
                            .saturating_add(asset_id.download_bytes(&self.plugin_models));
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
                self.state = NativeModelTaskState::Detected;
                if let Some((project_root, task)) = active_task
                    && let Err(error) = self.start(project_root, task)
                {
                    self.state = NativeModelTaskState::Failed(error);
                }
                return;
            }
            NativeModelTaskResult::Cancelled => {
                if let Some((_, NativeModelTask::Import(id, _))) = &active_task {
                    self.download_progress.remove(id);
                }
                self.state = NativeModelTaskState::Detected;
                if let Err(error) = self.start_next_queued() {
                    self.state = NativeModelTaskState::Failed(error);
                }
            }
            NativeModelTaskResult::SourceFailed(_) if !self.use_mirror => {
                self.state = NativeModelTaskState::Detected;
                if let Some((project_root, task)) = active_task {
                    let result = self
                        .switch_download_source(project_root.clone(), true)
                        .and_then(|()| self.start(project_root, task));
                    if let Err(error) = result {
                        self.state = NativeModelTaskState::Failed(error);
                    }
                }
            }
            NativeModelTaskResult::Failed(error) | NativeModelTaskResult::SourceFailed(error) => {
                if let Some((_, NativeModelTask::Import(id, _))) = &active_task {
                    self.download_progress.remove(id);
                }
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
        let Some(batch) = self.install_batch.as_mut() else {
            return Ok(());
        };
        while let Some(asset_id) = batch.queued.pop_front() {
            if self.known_ready.contains(&asset_id) {
                if !batch.completed.contains(&asset_id) {
                    batch.completed.push(asset_id.clone());
                    batch.completed_bytes = batch
                        .completed_bytes
                        .saturating_add(asset_id.download_bytes(&self.plugin_models));
                }
                continue;
            }
            let project_root = batch.project_root.clone();
            return self.start(project_root, NativeModelTask::Install(asset_id));
        }
        Ok(())
    }

    fn start(&mut self, project_root: PathBuf, task: NativeModelTask) -> Result<(), String> {
        if self.is_busy() {
            return Err("A native model task is already running.".into());
        }

        let (event_tx, event_rx) = unbounded();
        let proxy_url = self.proxy_url.clone();
        let source = DownloadSource::from_mirror_enabled(self.use_mirror);
        let cancellation =
            (!matches!(task, NativeModelTask::Discover)).then(DownloadCancellation::default);
        let worker_cancellation = cancellation.clone();
        let worker_root = project_root.clone();
        let plugin_models = self.plugin_models.clone();
        let worker_task = task.clone();
        thread::Builder::new()
            .name("native-model-installer".into())
            .spawn(move || {
                run_task(
                    worker_root,
                    worker_task,
                    plugin_models,
                    event_tx,
                    proxy_url,
                    source,
                    worker_cancellation,
                )
            })
            .map_err(|error| format!("Cannot start native model worker: {error}"))?;
        self.state = match &task {
            NativeModelTask::Discover => NativeModelTaskState::Discovering,
            NativeModelTask::Import(asset_id, _) => NativeModelTaskState::Installing {
                asset_id: asset_id.clone(),
                relative_path: None,
                downloaded_bytes: 0,
                total_bytes: 0,
            },
            NativeModelTask::Install(asset_id) => {
                let (downloaded_bytes, total_bytes) = self
                    .download_progress
                    .get(asset_id)
                    .copied()
                    .unwrap_or((0, asset_id.download_bytes(&self.plugin_models)));
                NativeModelTaskState::Installing {
                    asset_id: asset_id.clone(),
                    relative_path: None,
                    downloaded_bytes,
                    total_bytes,
                }
            }
        };
        self.events = Some(event_rx);
        self.cancellation = cancellation;
        self.active_task = Some((project_root, task));
        Ok(())
    }
}

fn resolve_model(
    project_root: &std::path::Path,
    id: &ResourceId,
    plugins: &HashMap<ResourceId, RegisteredModel>,
) -> Result<RegisteredModel, String> {
    match id {
        ResourceId::BuiltIn(id) => {
            let assets = load_assets(project_root)?;
            let asset = assets.asset(*id);
            Ok(RegisteredModel {
                package: asset.manifest().into(),
                directory: asset.directory().to_path_buf(),
            })
        }
        ResourceId::Plugin { .. } => plugins
            .get(id)
            .cloned()
            .ok_or_else(|| "The plugin model is no longer available.".into()),
    }
}

fn run_task(
    project_root: PathBuf,
    task: NativeModelTask,
    plugins: HashMap<ResourceId, RegisteredModel>,
    event_tx: crossbeam_channel::Sender<NativeModelTaskEvent>,
    proxy_url: Option<String>,
    source: DownloadSource,
    cancellation: Option<DownloadCancellation>,
) {
    let result = match task {
        NativeModelTask::Discover => discover_models(project_root, &plugins),
        NativeModelTask::Import(asset_id, source) => {
            let cancellation = cancellation.expect("import tasks have a cancellation token");
            let result = resolve_model(&project_root, &asset_id, &plugins).and_then(|model| {
                model
                    .package
                    .import(
                        &source,
                        &model.directory,
                        &cancellation,
                        |path, done, total| {
                            let _ =
                                event_tx.send(NativeModelTaskEvent::Progress(ResourceProgress {
                                    asset_id: asset_id.clone(),
                                    relative_path: path.into(),
                                    downloaded_bytes: done,
                                    total_bytes: total,
                                }));
                        },
                    )
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(_) => NativeModelTaskResult::Installed { asset_id },
                Err(_) if cancellation.is_cancelled() => NativeModelTaskResult::Cancelled,
                Err(error) => NativeModelTaskResult::Failed(error),
            }
        }
        NativeModelTask::Install(asset_id) => install_model(
            resolve_model(&project_root, &asset_id, &plugins),
            asset_id,
            &event_tx,
            proxy_url.as_deref(),
            source,
            cancellation.expect("install tasks have a cancellation token"),
        ),
    };
    let _ = event_tx.send(NativeModelTaskEvent::Finished(result));
}

fn discover_models(
    project_root: PathBuf,
    plugins: &HashMap<ResourceId, RegisteredModel>,
) -> NativeModelTaskResult {
    match load_assets(&project_root) {
        Ok(assets) => {
            if let Err(error) = xrtranslate_assets::ensure_bundled_gtcrn(&assets) {
                return NativeModelTaskResult::Failed(error.to_string());
            }
            let mut ready = assets
                .installed_assets()
                .map(|asset| ResourceId::BuiltIn(asset.manifest().id))
                .collect::<Vec<_>>();
            ready.extend(
                plugins
                    .iter()
                    .filter(|(_, model)| model.package.check(&model.directory).unwrap_or(false))
                    .map(|(id, _)| id.clone()),
            );
            NativeModelTaskResult::Detected(ready)
        }
        Err(error) => NativeModelTaskResult::Failed(error),
    }
}

fn install_model(
    model: Result<RegisteredModel, String>,
    asset_id: ResourceId,
    event_tx: &crossbeam_channel::Sender<NativeModelTaskEvent>,
    proxy_url: Option<&str>,
    source: DownloadSource,
    cancellation: DownloadCancellation,
) -> NativeModelTaskResult {
    let model = match model {
        Ok(model) => model,
        Err(error) => return NativeModelTaskResult::Failed(error),
    };
    let client = match DownloadClient::with_proxy_source_and_cancellation(
        concat!("xrtranslate-assets/", env!("CARGO_PKG_VERSION")),
        proxy_url,
        source,
        cancellation.clone(),
    ) {
        Ok(client) => client,
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
    let result = runtime.block_on(model.package.install(
        &client,
        &model.directory,
        |path, done, total| {
            let _ = event_tx.send(NativeModelTaskEvent::Progress(ResourceProgress {
                asset_id: asset_id.clone(),
                relative_path: path.into(),
                downloaded_bytes: done,
                total_bytes: total,
            }));
        },
    ));
    match result {
        Ok(_) => NativeModelTaskResult::Installed { asset_id },
        Err(error) if error.is_cancelled() || cancellation.is_cancelled() => {
            NativeModelTaskResult::Cancelled
        }
        Err(error) if error.allows_source_fallback() => {
            NativeModelTaskResult::SourceFailed(error.to_string())
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
        ModelCapability::SourceSeparation | ModelCapability::NoiseSuppression => {
            return Err("Source separation is selected by the media task.".into());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn discovering() -> (
        NativeModelTaskManager,
        crossbeam_channel::Sender<NativeModelTaskEvent>,
        PathBuf,
    ) {
        let root = PathBuf::from("test-model-root");
        let (tx, rx) = unbounded();
        let manager = NativeModelTaskManager {
            state: NativeModelTaskState::Discovering,
            active_task: Some((root.clone(), NativeModelTask::Discover)),
            events: Some(rx),
            ..NativeModelTaskManager::default()
        };
        (manager, tx, root)
    }

    fn plugin_model(root: &std::path::Path) -> (ResourceId, RegisteredModel) {
        (
            ResourceId::Plugin {
                plugin: "test-plugin".into(),
                model: "test-model".into(),
            },
            RegisteredModel {
                package: ModelPackage {
                    id: "test-model".into(),
                    revision: "main".into(),
                    archive: None,
                    files: vec![xrtranslate_assets::PackageFile {
                        path: "model.onnx".into(),
                        url: "https://example.invalid/model.onnx".into(),
                        bytes: 4,
                    }],
                },
                directory: root.join("test-model"),
            },
        )
    }

    #[test]
    fn plugin_and_builtin_models_share_queue_and_pause_state() {
        let (mut manager, tx, root) = discovering();
        let (plugin, model) = plugin_model(&root);
        let builtin = ResourceId::BuiltIn(ModelAssetId::GtcrnOnnx);
        manager.plugin_models.insert(plugin.clone(), model);
        manager
            .enqueue_many(
                root.clone(),
                [plugin.clone(), builtin.clone(), plugin.clone()],
            )
            .unwrap();
        assert_eq!(
            manager.batch_snapshot().unwrap().queued_packages,
            [plugin.clone(), builtin.clone()]
        );
        manager.pause(&plugin).unwrap();
        assert_eq!(
            manager.batch_snapshot().unwrap().queued_packages,
            [builtin.clone()]
        );
        manager.switch_download_source(root.clone(), true).unwrap();
        assert!(manager.is_model_paused(&plugin));
        manager.enqueue_many(root, [&plugin]).unwrap();
        assert!(!manager.is_model_paused(&plugin));
        assert!(manager.use_mirror());
        assert_eq!(manager.batch_snapshot().unwrap().total_packages, 2);
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::Detected(vec![builtin, plugin.clone()]),
        ))
        .unwrap();
        manager.poll();
        assert!(manager.is_model_ready(&plugin));
        assert!(!manager.is_busy());
        let batch = manager.batch_snapshot().unwrap();
        assert_eq!(batch.completed_packages, 2);
        assert_eq!(batch.downloaded_bytes, batch.total_bytes);
    }

    #[test]
    fn plugin_install_reuses_staged_files_and_deletes_only_declared_files() {
        let root = tempfile::tempdir().unwrap();
        let (id, model) = plugin_model(root.path());
        let staging = root.path().join(".xrtranslate-staging/test-model-main");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("model.onnx"), b"test").unwrap();
        let mut manager = NativeModelTaskManager::default();
        manager.set_plugin_models(HashMap::from([(id.clone(), model.clone())]));
        manager.enqueue_many(root.path().into(), [&id]).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while manager.is_busy() && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(5));
            manager.poll();
        }
        assert!(manager.is_model_ready(&id), "{:?}", manager.state());
        assert_eq!(manager.take_changed_resources(), [id.clone()]);
        assert_eq!(
            std::fs::read(model.directory.join("model.onnx")).unwrap(),
            b"test"
        );
        std::fs::write(model.directory.join("keep.txt"), b"user data").unwrap();
        manager.delete(root.path(), &id).unwrap();
        assert!(!model.directory.join("model.onnx").exists());
        assert!(model.directory.join("keep.txt").exists());
        assert!(!manager.is_model_ready(&id));
    }

    #[test]
    fn local_import_updates_readiness_and_skips_future_downloads() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("received");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("model.onnx"), b"test").unwrap();
        let (id, model) = plugin_model(root.path());
        let mut manager = NativeModelTaskManager::default();
        manager.set_plugin_models(HashMap::from([(id.clone(), model.clone())]));
        manager
            .import(root.path().into(), &id, source.clone())
            .unwrap();
        assert!(manager.is_importing(&id));
        // Changing the network source must not restart a local copy.
        manager
            .switch_download_source(root.path().into(), true)
            .unwrap();
        assert!(!manager.restart_after_source_switch);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while manager.is_busy() && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(5));
            manager.poll();
        }
        assert!(manager.is_model_ready(&id), "{:?}", manager.state());
        assert!(!manager.is_importing(&id));
        assert_eq!(manager.take_changed_resources(), [id.clone()]);
        assert!(model.package.check(&model.directory).unwrap());
        assert!(source.join("model.onnx").exists());
        manager.enqueue_many(root.path().into(), [&id]).unwrap();
        assert!(!manager.is_busy());
        assert!(!manager.is_model_queued(&id));
    }

    #[test]
    fn failed_local_import_stays_unready_and_can_retry() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("received");
        std::fs::create_dir(&source).unwrap();
        let (id, model) = plugin_model(root.path());
        let mut manager = NativeModelTaskManager::default();
        manager.set_plugin_models(HashMap::from([(id.clone(), model.clone())]));
        for valid in [false, true] {
            if valid {
                std::fs::write(source.join("model.onnx"), b"test").unwrap();
            }
            manager
                .import(root.path().into(), &id, source.clone())
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while manager.is_busy() && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(5));
                manager.poll();
            }
            assert_eq!(manager.is_model_ready(&id), valid, "{:?}", manager.state());
            assert_eq!(model.directory.exists(), valid);
            if !valid {
                assert!(matches!(manager.state(), NativeModelTaskState::Failed(_)));
                assert!(!manager.use_mirror());
                assert!(manager.take_changed_resources().is_empty());
            }
        }
    }

    #[test]
    fn discovery_accepts_download_intent_and_skips_already_installed_models() {
        let (mut manager, tx, root) = discovering();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.enqueue_many(root, [id, id]).unwrap();
        assert!(manager.is_model_queued(id));
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::Detected(vec![id.into()]),
        ))
        .unwrap();
        manager.poll();
        assert!(manager.is_model_ready(id));
        assert!(!manager.is_model_queued(id));
        assert!(!manager.is_busy());
        let batch = manager.batch_snapshot().unwrap();
        assert_eq!(batch.completed_packages, 1);
        assert_eq!(batch.total_packages, 1);
        assert_eq!(batch.downloaded_bytes, batch.total_bytes);
    }

    #[test]
    fn queued_model_can_pause_and_resume_during_discovery() {
        let (mut manager, _tx, root) = discovering();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.enqueue_many(root.clone(), [id]).unwrap();
        manager.pause(id).unwrap();
        assert!(manager.is_model_paused(id));
        assert!(!manager.is_model_queued(id));
        assert_eq!(
            manager.model_download_progress(id),
            Some((0, model_download_bytes(id)))
        );
        manager.enqueue_many(root, [id]).unwrap();
        assert!(!manager.is_model_paused(id));
        assert!(manager.is_model_queued(id));
        assert_eq!(manager.batch_snapshot().unwrap().total_packages, 1);
    }

    #[test]
    fn paused_worker_retains_final_progress_without_requeueing() {
        let (mut manager, tx, root) = discovering();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.enqueue_many(root.clone(), [id]).unwrap();
        manager.install_batch.as_mut().unwrap().queued.clear();
        manager.active_task = Some((root, NativeModelTask::Install(id.into())));
        manager.state = NativeModelTaskState::Installing {
            asset_id: id.into(),
            relative_path: None,
            downloaded_bytes: 10,
            total_bytes: 100,
        };
        let cancellation = DownloadCancellation::default();
        manager.cancellation = Some(cancellation.clone());
        manager.pause(id).unwrap();
        assert!(cancellation.is_cancelled());
        tx.send(NativeModelTaskEvent::Progress(ResourceProgress {
            asset_id: id.into(),
            relative_path: "model.onnx".into(),
            downloaded_bytes: 20,
            total_bytes: 100,
        }))
        .unwrap();
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::Cancelled,
        ))
        .unwrap();
        manager.poll();
        assert!(manager.is_model_paused(id));
        assert!(!manager.is_model_queued(id));
        assert!(!manager.is_busy());
        assert_eq!(manager.model_download_progress(id), Some((20, 100)));
        assert_eq!(manager.batch_snapshot().unwrap().downloaded_bytes, 20);
        manager
            .switch_download_source(PathBuf::from("test-model-root"), true)
            .unwrap();
        assert_eq!(manager.model_download_progress(id), Some((20, 100)));
    }

    #[test]
    fn source_failure_selects_the_visible_mirror_only_once() {
        let (mut manager, tx, _) = discovering();
        // A fresh directory has no config, so restarting the worker below
        // exercises source selection without performing a network request.
        let root = tempfile::tempdir().unwrap();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.active_task = Some((
            root.path().to_path_buf(),
            NativeModelTask::Install(id.into()),
        ));
        manager.state = NativeModelTaskState::Installing {
            asset_id: id.into(),
            relative_path: None,
            downloaded_bytes: 10,
            total_bytes: 100,
        };
        manager.download_progress.insert(id.into(), (10, 100));
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::SourceFailed("official unavailable".into()),
        ))
        .unwrap();
        manager.poll();
        assert!(manager.use_mirror());
        assert_eq!(manager.model_download_progress(id), Some((10, 100)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while manager.is_busy() && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(5));
            manager.poll();
        }
        assert!(!manager.is_busy());
        let (tx, rx) = unbounded();
        manager.events = Some(rx);
        manager.active_task = Some((
            root.path().to_path_buf(),
            NativeModelTask::Install(id.into()),
        ));
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::SourceFailed("mirror unavailable".into()),
        ))
        .unwrap();
        manager.poll();
        assert!(
            matches!(manager.state(), NativeModelTaskState::Failed(error) if error == "mirror unavailable")
        );
        assert!(manager.use_mirror());
        manager.paused.insert(id.into());
        manager
            .switch_download_source(root.path().to_path_buf(), false)
            .unwrap();
        assert!(!manager.use_mirror());
        assert!(manager.is_model_paused(id));
        assert_eq!(manager.model_download_progress(id), Some((10, 100)));
    }

    #[test]
    fn a_failure_already_in_flight_cannot_restart_a_paused_download() {
        let (mut manager, tx, root) = discovering();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.active_task = Some((root, NativeModelTask::Install(id.into())));
        manager.paused.insert(id.into());
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::SourceFailed("official unavailable".into()),
        ))
        .unwrap();
        manager.poll();
        assert!(manager.is_model_paused(id));
        assert!(!manager.is_busy());
        assert!(!manager.use_mirror());
    }

    #[test]
    fn a_late_mirror_failure_cannot_override_a_manual_switch_to_official() {
        let (mut manager, tx, _) = discovering();
        let root = tempfile::tempdir().unwrap();
        let id = ModelAssetId::UvrMdxnet9482Onnx;
        manager.use_mirror = true;
        manager.active_task = Some((
            root.path().to_path_buf(),
            NativeModelTask::Install(id.into()),
        ));
        manager.state = NativeModelTaskState::Installing {
            asset_id: id.into(),
            relative_path: None,
            downloaded_bytes: 10,
            total_bytes: 100,
        };
        tx.send(NativeModelTaskEvent::Finished(
            NativeModelTaskResult::SourceFailed("old mirror failure".into()),
        ))
        .unwrap();
        manager
            .switch_download_source(root.path().to_path_buf(), false)
            .unwrap();
        manager.poll();
        assert!(!manager.use_mirror());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while manager.is_busy() && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(5));
            manager.poll();
        }
        assert!(!manager.is_busy());
        assert!(!manager.use_mirror());
    }
}
