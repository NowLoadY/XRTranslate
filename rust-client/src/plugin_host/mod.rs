//! Installed plugins contribute views and operations; this adapter supplies host services.
mod datagram;
#[cfg(not(target_os = "android"))]
mod import;
mod io;
mod model_tools;
mod models;
mod operations;
mod osc;
pub(crate) use operations::Call;
mod playback;
mod services;
mod ui;
use crate::{
    i18n::UiLanguage,
    plugins::{PluginId, PluginPages},
    session_coordinator::{HostOutputEvent, HostOutputSubscriber},
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use xrtranslate_plugin::{Environment, Manifest, Message, host::Process};

#[derive(Clone, Default)]
pub(crate) struct Output(Arc<Mutex<VecDeque<Message>>>);
impl HostOutputSubscriber for Output {
    fn on_host_output(&self, event: HostOutputEvent<'_>) {
        let output = Message::Event {
            name: "translation.output".into(),
            value: json!(event),
        };
        let message = match event {
            HostOutputEvent::CommittedTranslation {
                stream_id,
                turn_id,
                segment_index,
                source,
                translated,
                audio_source,
                speaker,
                is_typing,
                additional_translations,
                ..
            } => Some(Message::Event {
                name: "translation.committed".into(),
                value: json!({"stream":stream_id,"turn":turn_id,"segment":segment_index,"source":source,"translation":translated,"audio_source":audio_source,"speaker":speaker,"is_typing":is_typing,"additional":additional_translations.iter().map(|item|item.translated_text.as_str()).collect::<Vec<_>>()}),
            }),
            HostOutputEvent::Clear => Some(Message::Event {
                name: "translation.clear".into(),
                value: Value::Null,
            }),
            HostOutputEvent::StreamCancelled(id) => Some(Message::Event {
                name: "translation.cancelled".into(),
                value: json!({"stream":id}),
            }),
            _ => None,
        };
        let mut queue = self.0.lock().unwrap();
        for message in std::iter::once(output).chain(message) {
            if queue.len() == 128 {
                queue.pop_front();
                log::warn!("Plugin output queue is full; oldest event discarded");
            }
            queue.push_back(message);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    enabled: bool,
    /// Legacy single-model selection, migrated into `models` on load.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    models: std::collections::BTreeSet<String>,
    config: Value,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            package: None,
            model: None,
            models: std::collections::BTreeSet::new(),
            config: Value::Null,
        }
    }
}
impl Settings {
    fn normalize(&mut self) {
        if let Some(legacy) = self.model.take()
            && !legacy.is_empty()
        {
            self.models.insert(legacy);
        }
    }

    fn has_model(&self, id: &str) -> bool {
        self.models.contains(id) || self.model.as_deref() == Some(id)
    }

    fn selected_models<'a>(&self, manifest: &'a Manifest) -> Vec<&'a xrtranslate_plugin::Model> {
        manifest
            .models
            .iter()
            .filter(|model| self.has_model(&model.id))
            .collect()
    }

    fn set_model_selected(&mut self, manifest: &Manifest, model_id: &str, selected: bool) -> bool {
        self.normalize();
        let Some(target) = manifest.models.iter().find(|m| m.id == model_id) else {
            return false;
        };
        let mut changed = false;
        if selected {
            if let Some(group) = target.group.as_deref() {
                for sibling in manifest
                    .models
                    .iter()
                    .filter(|m| m.id != model_id && m.group.as_deref() == Some(group))
                {
                    if self.models.remove(&sibling.id) {
                        changed = true;
                    }
                }
            }
            if self.models.insert(model_id.to_owned()) {
                changed = true;
            }
        } else if self.models.remove(model_id) {
            changed = true;
        }
        changed
    }
}
struct Installed {
    modified: Option<std::time::SystemTime>,
    directory_modified: Option<std::time::SystemTime>,
    icon: Option<(String, Arc<[u8]>)>,
    directory: PathBuf,
    executable: PathBuf,
    manifest: Manifest,
    settings: Settings,
    environment_dirty: bool,
    process: Option<Process>,
    starting: Option<std::sync::mpsc::Receiver<Result<Process, String>>>,
    error: Option<String>,
    page: ui::PageState,
    state: Value,
}
struct Inference {
    plugin: String,
    id: u64,
    reply: crate::ask::Reply,
    cancel: Arc<AtomicBool>,
}
use xrtranslate_plugin::directory::{Catalog, Package};

type Scan = Result<Catalog, String>;

pub(crate) struct Host {
    datagrams: datagram::Outputs,
    osc: osc::Control,
    #[cfg(target_os = "android")]
    documents: Vec<(
        String,
        u64,
        xrtranslate_client_core::file_dialog::FileRequest,
    )>,
    bookmarks: Vec<(String, u64, xrtranslate_client_core::bookmarks::Reply)>,
    stopped: Vec<String>,
    visible: Option<String>,
    preparing: Vec<(String, u64, std::time::Instant)>,
    pub sessions: Sessions,
    pub playback: playback::Players,
    root: PathBuf,
    directory: PathBuf,
    executables: Option<PathBuf>,
    packaged_directory: Option<PathBuf>,
    next_scan: std::time::Instant,
    scan: Option<std::sync::mpsc::Receiver<Scan>>,
    #[cfg(not(target_os = "android"))]
    import: import::Import,
    navigation_changed: bool,
    models_root: PathBuf,
    resources: Option<(String, BTreeMap<String, PathBuf>)>,
    capabilities: Vec<String>,
    context: BTreeMap<String, Value>,
    catalog: Catalog,
    installed: BTreeMap<String, Installed>,
    pub output: Output,
    inference: Vec<Inference>,
    operations: Vec<operations::Pending>,
    cleanup: Vec<ui_director::ToolCall>,
    commands: Vec<(String, u64)>,
    surface_requests: Vec<(String, u64, ui_director::egui::driver::Request)>,
    context_updated: std::time::Instant,
    pub error: Option<String>,
    io: io::Worker,
}
impl Host {
    pub fn send_event(&self, plugin: &str, name: &str, value: Value) {
        if let Some(process) = self.installed.get(plugin).and_then(|p| p.process.as_ref()) {
            if let Err(error) = process.send(Message::Event {
                name: name.into(),
                value,
            }) {
                log::warn!("Plugin {plugin}, event {name}: {error}");
            }
        }
    }

    pub fn load(root: PathBuf) -> Self {
        let config = xrtranslate_config::AppConfig::from_path_with_user_config(
            &root.join("config.json"),
            &root,
        )
        .ok();
        let runtime = config
            .as_ref()
            .map(|c| c.runtime_layout(&root))
            .unwrap_or_else(|| xrtranslate_config::RuntimeLayout::for_project_root(&root));
        let models_root = xrtranslate_assets::resolve_from_project_root(
            &root,
            config
                .as_ref()
                .and_then(|c| c.model_manager.models_directory.as_deref())
                .unwrap_or(Path::new("models")),
        );
        Self {
            datagrams: Default::default(),
            osc: Default::default(),
            #[cfg(target_os = "android")]
            documents: Vec::new(),
            bookmarks: Vec::new(),
            stopped: Vec::new(),
            visible: None,
            preparing: Vec::new(),
            sessions: Sessions::default(),
            playback: playback::Players::default(),
            root: runtime.runtime_root().join("plugins"),
            directory: xrtranslate_config::data_root(&root).join("plugins"),
            packaged_directory: (xrtranslate_config::data_root(&root) != root)
                .then(|| root.join("plugins")),
            #[cfg(target_os = "android")]
            executables: Some(crate::android::native_library_directory()),
            #[cfg(not(target_os = "android"))]
            executables: None,
            next_scan: std::time::Instant::now(),
            scan: None,
            #[cfg(not(target_os = "android"))]
            import: Default::default(),
            navigation_changed: true,
            io: io::Worker::new(),
            models_root,
            resources: None,
            capabilities: Vec::new(),
            context: BTreeMap::new(),
            catalog: Catalog::default(),
            installed: BTreeMap::new(),
            output: Output::default(),
            inference: Vec::new(),
            operations: Vec::new(),
            cleanup: Vec::new(),
            commands: Vec::new(),
            surface_requests: Vec::new(),
            context_updated: std::time::Instant::now(),
            error: None,
        }
    }
    pub fn refresh(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_os = "android"))]
        if self.poll_import(ctx) {
            return;
        }
        if self.scan.is_none() && std::time::Instant::now() >= self.next_scan {
            let directory = self.directory.clone();
            let packaged_directory = self.packaged_directory.clone();
            let executables = self.executables.clone();
            self.scan = Some(self.io.submit(
                move || {
                    xrtranslate_plugin::directory::discover_packages(
                        &directory,
                        packaged_directory.as_deref(),
                        executables.as_deref(),
                    )
                },
                Some(ctx.clone()),
            ));
        }
        let Some(result) = self.scan.as_ref().and_then(|scan| scan.try_recv().ok()) else {
            return;
        };
        self.scan = None;
        self.next_scan = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut catalog = match result {
            Ok(catalog) => catalog,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let mut errors: Vec<_> = std::mem::take(&mut catalog.issues)
            .into_iter()
            .map(|(path, error)| format!("{}: {error}", path.display()))
            .collect();
        self.catalog = catalog;
        let providers: std::collections::BTreeSet<_> = self
            .catalog
            .providers
            .keys()
            .chain(self.installed.keys())
            .cloned()
            .collect();
        for provider in providers {
            if let Err(error) = self.sync_package(&provider) {
                errors.push(error);
            }
        }
        let error = (!errors.is_empty()).then(|| errors.join("\n"));
        if error != self.error
            && let Some(error) = &error
        {
            log::warn!("Plugin discovery: {error}");
        }
        self.error = error;
    }
    fn data(&self, id: &str) -> PathBuf {
        self.root.join("data").join(id)
    }
    fn load_settings(&self, provider: &str) -> Settings {
        let mut settings: Settings = std::fs::read(self.data(provider).join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| {
                let enabled = std::fs::read(
                    self.root
                        .parent()
                        .unwrap()
                        .join("rust-client-settings.json"),
                )
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| legacy_enablement(&value, provider))
                .unwrap_or(true);
                Settings {
                    enabled,
                    ..Default::default()
                }
            });
        settings.normalize();
        settings
    }

    fn sync_package(&mut self, id: &str) -> Result<(), String> {
        let active = self.installed.get(id);
        let stored;
        let settings = match active {
            Some(entry) => &entry.settings,
            None => {
                stored = self.load_settings(id);
                &stored
            }
        };
        let selected = self.catalog.selected(id, settings.package.as_deref());
        if let (Some(active), Some(package)) = (active, selected)
            && active.directory == package.directory
            && active.manifest == package.manifest
            && active.modified == package.modified
            && active.directory_modified == package.directory_modified
        {
            return Ok(());
        }
        let replacement = selected.map(|p| (p.clone(), settings.clone()));
        if active.is_some() {
            self.stop(id);
            self.installed.remove(id);
        }
        self.navigation_changed = true;
        self.context.remove("host.context");
        if let Some((package, settings)) = replacement {
            self.register(package, settings)?;
        }
        Ok(())
    }

    fn select_package(&mut self, id: &str, choice: &str) -> Result<(), String> {
        if !self
            .catalog
            .providers
            .get(id)
            .is_some_and(|packages| packages.iter().any(|p| p.manifest.id == choice))
        {
            return Err("Plugin edition is not installed".into());
        }
        let mut settings = self
            .installed
            .get(id)
            .ok_or("Plugin is not installed")?
            .settings
            .clone();
        settings.package = Some(choice.to_owned());
        self.write_settings(id, &settings)?;
        self.installed.get_mut(id).unwrap().settings = settings;
        self.sync_package(id)
    }

    fn register(&mut self, package: Package, settings: Settings) -> Result<(), String> {
        let Package {
            directory,
            manifest,
            modified,
            directory_modified,
        } = package;
        let provider = manifest.provider_id();
        PluginId::parse(provider).ok_or("Invalid plugin ID")?;
        self.installed.insert(
            provider.to_owned(),
            Installed {
                modified,
                directory_modified,
                icon: manifest.icon.as_ref().and_then(|icon| {
                    std::fs::read(directory.join(icon)).ok().map(|bytes| {
                        (
                            format!("bytes://plugins/{}/{}", manifest.id, icon),
                            bytes.into(),
                        )
                    })
                }),
                error: xrtranslate_plugin::directory::require_host(
                    &manifest,
                    env!("CARGO_PKG_VERSION"),
                )
                .err(),
                executable: xrtranslate_plugin::directory::executable(
                    &directory,
                    &manifest,
                    self.executables.as_deref(),
                )?,
                directory,
                manifest,
                settings,
                environment_dirty: false,
                process: None,
                starting: None,
                page: ui::PageState::default(),
                state: Value::Null,
            },
        );
        Ok(())
    }
    fn save(&self, id: &str) -> Result<(), String> {
        let entry = self.installed.get(id).ok_or("Plugin is not installed")?;
        self.write_settings(id, &entry.settings)
    }
    fn write_settings(&self, id: &str, settings: &Settings) -> Result<(), String> {
        let path = self.data(id);
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let staged = path.join("settings.json.tmp");
        std::fs::write(
            &staged,
            serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(staged, path.join("settings.json")).map_err(|e| e.to_string())
    }
    pub fn needs_environment(
        &self,
        language: UiLanguage,
        runtimes: &BTreeMap<String, PathBuf>,
    ) -> bool {
        self.resources
            .as_ref()
            .is_none_or(|(lang, paths)| lang != locale(language) || paths != runtimes)
            || self.installed.values().any(|p| {
                p.settings.enabled
                    && p.error.is_none()
                    && (p.environment_dirty || (p.process.is_none() && p.starting.is_none()))
            })
    }
    pub fn available(&self, id: &str) -> bool {
        self.installed
            .get(id)
            .is_some_and(|p| p.settings.enabled && p.process.is_some())
    }
    pub fn state(&self, id: &str) -> &Value {
        self.installed
            .get(id)
            .map_or(&Value::Null, |entry| &entry.state)
    }
    pub fn state_owner(&self, capability: &str) -> Option<&str> {
        self.installed
            .iter()
            .find(|(_, entry)| entry.process.is_some() && !entry.state[capability].is_null())
            .map(|(id, _)| id.as_str())
    }
    pub fn states(&self) -> impl Iterator<Item = &Value> {
        self.installed
            .values()
            .filter(|entry| entry.process.is_some())
            .map(|entry| &entry.state)
    }
    pub fn invoke(&mut self, call: ui_director::ToolCall) -> Result<(), String> {
        let (plugin, entry) = self
            .installed
            .iter_mut()
            .find(|(_, entry)| {
                entry
                    .manifest
                    .tools
                    .iter()
                    .any(|tool| tool.name == call.name)
            })
            .ok_or("Plugin operation is unavailable")?;
        let process = entry.process.as_mut().ok_or("Plugin is unavailable")?;
        let id = process.request(&call.name, call.arguments)?;
        self.commands.push((plugin.clone(), id));
        Ok(())
    }
    pub fn configuration(&self, id: PluginId) -> Option<&Value> {
        self.installed
            .get(id.as_str())
            .map(|entry| &entry.settings.config)
    }
    pub fn contains(&self, id: PluginId) -> bool {
        self.installed.contains_key(id.as_str())
    }
    pub fn enabled(&self, id: PluginId) -> bool {
        self.installed
            .get(id.as_str())
            .is_some_and(|p| p.settings.enabled)
    }
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        self.stop(id);
        let entry = self
            .installed
            .get_mut(id)
            .ok_or("Plugin is not installed")?;
        entry.settings.enabled = enabled;
        self.navigation_changed = true;
        self.context.remove("host.context");
        entry.error =
            xrtranslate_plugin::directory::require_host(&entry.manifest, env!("CARGO_PKG_VERSION"))
                .err();
        self.save(id)
    }
    fn stop(&mut self, id: &str) {
        #[cfg(target_os = "android")]
        self.documents.retain(|(plugin, _, _)| plugin != id);
        self.datagrams.remove(id);
        self.osc.cancel(id, None);
        self.bookmarks.retain(|(plugin, _, _)| plugin != id);
        self.playback.remove(id);
        self.stopped.push(id.to_owned());
        self.preparing.retain(|(plugin, _, _)| plugin != id);
        self.cancel_operations(id, None);
        if let Some(entry) = self.installed.get_mut(id) {
            let process = entry.process.take();
            let starting = entry.starting.take();
            self.io.submit(move || drop((process, starting)), None);
            entry.state = Value::Null;
            entry.page.surface = Default::default();
        }
        self.publish("plugin.state", json!({"plugin":id,"state":null}));
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        preferences: &mut PluginPages,
        language: UiLanguage,
        runtimes: BTreeMap<String, PathBuf>,
        capabilities: Option<Vec<String>>,
    ) -> Vec<(String, Message)> {
        if let Some(capabilities) = capabilities {
            self.capabilities = capabilities;
        }
        let capabilities = self.capabilities.clone();
        let resources = (locale(language).to_owned(), runtimes.clone());
        let resources_changed = self
            .resources
            .as_ref()
            .is_some_and(|previous| previous != &resources);
        let language_changed = self
            .resources
            .as_ref()
            .is_none_or(|(lang, _)| lang != &resources.0);
        self.resources = Some(resources);
        let events: Vec<_> = self.output.0.lock().unwrap().drain(..).collect();
        let mut calls = Vec::new();
        let mut failed = Vec::new();
        let mut new_subscribers = Vec::new();
        let language = locale(language);
        let base = self.root.clone();
        let models_root = self.models_root.clone();
        for (id, entry) in &mut self.installed {
            if !entry.settings.enabled {
                continue;
            }
            let mut started = false;
            if let Some(result) = entry
                .starting
                .as_ref()
                .and_then(|pending| pending.try_recv().ok())
            {
                entry.starting = None;
                match result {
                    Ok(process) => {
                        entry.process = Some(process);
                        started = true;
                    }
                    Err(error) => entry.error = Some(error),
                }
            }
            if resources_changed {
                entry.error = xrtranslate_plugin::directory::require_host(
                    &entry.manifest,
                    env!("CARGO_PKG_VERSION"),
                )
                .err();
            }
            if ((entry.process.is_none() && entry.starting.is_none())
                || resources_changed
                || entry.environment_dirty
                || started)
                && entry.error.is_none()
            {
                let missing: Vec<_> = entry
                    .manifest
                    .requires
                    .iter()
                    .filter(|name| !capabilities.contains(name) && !runtimes.contains_key(*name))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    entry.error = Some(format!(
                        "Required capability unavailable: {}",
                        missing.join(", ")
                    ));
                    continue;
                }
                let selected_models = entry.settings.selected_models(&entry.manifest);
                let model_directories: BTreeMap<String, PathBuf> = selected_models
                    .iter()
                    .map(|model| (model.id.clone(), models_root.join(&model.directory)))
                    .collect();
                let model_directory = selected_models
                    .first()
                    .map(|model| models_root.join(&model.directory));
                let environment = Environment {
                    host_executable: std::env::current_exe().unwrap_or_default(),
                    language: language.into(),
                    data_directory: base.join("data").join(&entry.manifest.id),
                    model_directory,
                    model_directories,
                    runtimes: runtimes.clone(),
                    config: entry.settings.config.clone(),
                    capabilities: capabilities.clone(),
                };
                if let Some(process) = &entry.process {
                    entry.environment_dirty = false;
                    if let Err(error) = process.initialize(environment) {
                        entry.error = Some(error);
                    }
                    if started {
                        for (name, value) in &self.context {
                            if entry.manifest.subscriptions.contains(name) {
                                let _ = process.send(Message::Event {
                                    name: name.clone(),
                                    value: value.clone(),
                                });
                            }
                        }
                        if entry
                            .manifest
                            .subscriptions
                            .iter()
                            .any(|name| name == "plugin.state")
                        {
                            new_subscribers.push(id.clone());
                        }
                        let _ = process.send(Message::Event {
                            name: "ui.visibility".into(),
                            value: json!(self.visible.as_ref() == Some(id)),
                        });
                    }
                } else if entry.starting.is_none() {
                    let executable = entry.executable.clone();
                    let manifest = entry.manifest.clone();
                    let repaint = ctx.clone();
                    entry.starting = Some(self.io.submit(
                        move || {
                            Process::spawn(
                                &executable,
                                &manifest,
                                &environment.host_executable,
                                move || repaint.request_repaint(),
                            )
                        },
                        Some(ctx.clone()),
                    ));
                }
            }
            if let Some(process) = &mut entry.process {
                for event in &events {
                    if let Message::Event { name, .. } = event
                        && entry.manifest.subscriptions.contains(name)
                    {
                        if let Err(error) = process.send(event.clone()) {
                            entry.error = Some(error);
                        }
                    }
                }
                calls.extend(
                    process
                        .events()
                        .into_iter()
                        .map(|event| (id.clone(), event)),
                );
                if let Some(mut error) = process.error() {
                    if let Some(status) = process.exit_status() {
                        error = format!("{error} ({status})");
                    }
                    log::warn!("Plugin {id}: {error}");
                    entry.error = Some(error);
                    failed.push(id.clone());
                }
            }
        }
        calls.retain(|(id, _)| !failed.contains(id));
        for id in failed {
            self.stop(&id);
        }
        for subscriber in new_subscribers {
            if let Some(process) = self
                .installed
                .get(&subscriber)
                .and_then(|entry| entry.process.as_ref())
            {
                for (id, entry) in &self.installed {
                    let _ = process.send(Message::Event {
                        name: "plugin.state".into(),
                        value: json!({"plugin":id,"state":entry.state}),
                    });
                }
            }
        }
        if self.navigation_changed || language_changed {
            preferences.installed_pages = self
                .installed
                .iter()
                .filter(|(_, p)| p.settings.enabled && p.manifest.page)
                .map(|(id, p)| crate::plugins::PluginPage {
                    id: PluginId::parse(id).unwrap(),
                    title: p.manifest.title.get(language).into(),
                    order: p.manifest.navigation_order,
                    group: p.manifest.navigation_group.clone(),
                    icon: p.icon.clone(),
                })
                .collect();
            self.navigation_changed = false;
        }
        if !self.inference.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut ready = Vec::new();
        self.bookmarks.retain(|(plugin, id, reply)| {
            match reply.try_recv() {
                Ok(result) => ready.push((
                    plugin.clone(),
                    *id,
                    result
                        .and_then(|result| serde_json::to_value(result).map_err(|e| e.to_string())),
                )),
                Err(crossbeam_channel::TryRecvError::Disconnected) => ready.push((
                    plugin.clone(),
                    *id,
                    Err("Bookmark request interrupted".into()),
                )),
                _ => return true,
            }
            false
        });
        for request in &self.inference {
            match request.reply.try_recv() {
                Ok(value) => ready.push((
                    request.plugin.clone(),
                    request.id,
                    Ok(match value {
                        Ok(text) => json!({"text": text}),
                        Err(error) => json!({"error": error}),
                    }),
                )),
                Err(crossbeam_channel::TryRecvError::Disconnected) => ready.push((
                    request.plugin.clone(),
                    request.id,
                    Err("Inference interrupted".into()),
                )),
                _ => {}
            }
        }
        for (plugin, id, result) in ready {
            self.inference
                .retain(|r| !(r.plugin == plugin && r.id == id));
            self.reply(&plugin, id, result);
        }
        self.commands.retain(|(plugin, id)| {
            let Some(process) = self
                .installed
                .get_mut(plugin)
                .and_then(|entry| entry.process.as_mut())
            else {
                return false;
            };
            match process.result(*id) {
                None => true,
                Some(result) => {
                    if let Err(error) = result {
                        calls.push((
                            plugin.clone(),
                            Message::Event {
                                name: "message".into(),
                                value: json!(error),
                            },
                        ));
                    }
                    false
                }
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        calls
    }
    pub fn reply(&self, plugin: &str, id: u64, result: xrtranslate_plugin::Result) {
        if let Some(process) = self.installed.get(plugin).and_then(|p| p.process.as_ref()) {
            if let Err(error) = process.send(Message::Reply { id, result }) {
                log::warn!("Plugin reply: {error}");
            }
        }
    }
    pub fn publish(&self, name: &str, value: Value) {
        for entry in self.installed.values() {
            if entry
                .manifest
                .subscriptions
                .iter()
                .any(|topic| topic == name)
                && let Some(process) = &entry.process
            {
                if let Err(error) = process.send(Message::Event {
                    name: name.into(),
                    value: value.clone(),
                }) {
                    log::warn!("Plugin event {name}: {error}");
                }
            }
        }
    }
    pub fn set_context(&mut self, topic: &str, value: Value) -> bool {
        if self.context.get(topic) != Some(&value) {
            self.publish(topic, value.clone());
            self.context.insert(topic.into(), value);
            return true;
        }
        false
    }
    pub fn inference(
        &mut self,
        plugin: String,
        id: u64,
        reply: crate::ask::Reply,
        cancel: Arc<AtomicBool>,
    ) {
        self.inference.push(Inference {
            plugin,
            id,
            reply,
            cancel,
        });
    }
}

fn legacy_enablement(settings: &Value, id: &str) -> Option<bool> {
    let enabled = settings
        .pointer("/plugin_preferences/enabled")
        .or_else(|| settings.pointer("/plugins/enabled"))?
        .as_object()?;
    enabled.get(id).and_then(Value::as_bool).or_else(|| {
        enabled.iter().find_map(|(name, enabled)| {
            (PluginId::parse(name) == PluginId::parse(id))
                .then(|| enabled.as_bool())
                .flatten()
        })
    })
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn keeps_legacy_enablement_including_renamed_plugins() {
        let settings = json!({"plugin_preferences":{"enabled":{"video_player":false,"osc":false,"external_plugin":true}}});
        assert_eq!(legacy_enablement(&settings, "media"), Some(false));
        assert_eq!(legacy_enablement(&settings, "osc"), Some(false));
        assert_eq!(legacy_enablement(&settings, "external_plugin"), Some(true));
        assert_eq!(legacy_enablement(&settings, "new_plugin"), None);
        assert_eq!(
            legacy_enablement(&json!({"plugins":{"enabled":{"media":false}}}), "media"),
            Some(false)
        );
    }
}
pub(crate) fn locale(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::Chinese => "zh",
        UiLanguage::Japanese => "ja",
        UiLanguage::Korean => "ko",
        UiLanguage::Russian => "ru",
        UiLanguage::English => "en",
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for request in &self.inference {
            request.cancel.store(true, Ordering::Release);
        }
        for entry in self.installed.values_mut() {
            let process = entry.process.take();
            let starting = entry.starting.take();
            self.io.submit(move || drop((process, starting)), None);
        }
    }
}

impl ui_director::ToolProvider for Host {
    fn tools(&self) -> Vec<ui_director::Tool> {
        use ui_director::Tool;
        let mut tools = vec![
            Tool::new(
                "plugins.list",
                "List installed plugins and their status.",
                json!({}),
                &[],
            )
            .read_only(),
            Tool::new(
                "plugins.enable",
                "Enable or disable the selected provider for a plugin ID.",
                json!({"id":{"type":"string"},"enabled":{"type":"boolean"}}),
                &["id", "enabled"],
            ),
            Tool::new(
                "plugins.operation",
                "Collect a pending plugin operation.",
                json!({"plugin":{"type":"string"},"id":{"type":"integer"}}),
                &["plugin", "id"],
            )
            .read_only(),
            Tool::new(
                "plugins.cancel",
                "Cancel a pending plugin request.",
                json!({"plugin":{"type":"string"},"id":{"type":"integer"}}),
                &["plugin", "id"],
            ),
        ];
        tools.extend(
            self.installed
                .values()
                .filter(|p| p.settings.enabled)
                .flat_map(|p| &p.manifest.tools)
                .map(|t| Tool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                    read_only: t.read_only,
                }),
        );
        tools
    }
    fn call(&mut self, call: &ui_director::ToolCall) -> ui_director::ToolResult {
        let args = &call.arguments;
        let string = |key: &str| args[key].as_str().ok_or_else(|| format!("Missing {key}"));
        match call.name.as_str() {
            "plugins.list" => Ok(
                json!({"directory":self.directory,"plugins":self.installed.iter().map(|(id,p)| json!({"id":id,"package_id":p.manifest.id,"title":p.manifest.title,"enabled":p.settings.enabled,"running":p.process.is_some(),"error":p.error})).collect::<Vec<_>>(),"error":self.error}),
            ),
            "plugins.enable" => {
                self.set_enabled(
                    string("id")?,
                    args["enabled"].as_bool().ok_or("Missing enabled")?,
                )?;
                Ok(json!({"updated":true}))
            }
            "plugins.operation" | "plugins.cancel" => {
                let plugin = string("plugin")?;
                let id = args["id"].as_u64().ok_or("Missing operation id")?;
                let process = self
                    .installed
                    .get_mut(plugin)
                    .and_then(|p| p.process.as_mut())
                    .ok_or("Plugin is stopped")?;
                if call.name == "plugins.cancel" {
                    process.cancel(id);
                    return Ok(json!({"cancelled":true}));
                }
                process
                    .result(id)
                    .unwrap_or_else(|| Ok(receipt(plugin, id)))
            }
            _ => {
                let (plugin, entry) = self
                    .installed
                    .iter_mut()
                    .find(|(_, p)| p.manifest.tools.iter().any(|t| t.name == call.name))
                    .ok_or("Unknown plugin operation")?;
                let process = entry.process.as_mut().ok_or("Plugin is unavailable")?;
                let id = process.request(&call.name, call.arguments.clone())?;
                Ok(receipt(plugin, id))
            }
        }
    }
}
fn receipt(plugin: &str, id: u64) -> Value {
    json!({
        "pending":{"name":"plugins.operation","arguments":{"plugin":plugin,"id":id}},
        "cancel":{"name":"plugins.cancel","arguments":{"plugin":plugin,"id":id}},
    })
}

#[cfg(test)]
mod edition_tests {
    use super::*;

    #[test]
    fn switching_editions_preserves_enablement_and_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("plugins");
        for (id, family) in [("sample", None), ("sample_plus", Some("sample"))] {
            let directory = plugins.join(id);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("plugin"), []).unwrap();
            std::fs::write(
                directory.join("plugin.json"),
                json!({
                    "id":id, "pro_for":family, "min_host_version":"0.4.0",
                    "title":{"en":id}, "executable":"plugin"
                })
                .to_string(),
            )
            .unwrap();
        }
        let load = || {
            let mut host = Host::load(root.path().to_owned());
            host.catalog =
                xrtranslate_plugin::directory::discover_packages(&plugins, None, None).unwrap();
            host.sync_package("sample").unwrap();
            host
        };
        let mut host = load();
        assert_eq!(host.installed["sample"].manifest.id, "sample");
        host.set_enabled("sample", false).unwrap();
        host.select_package("sample", "sample_plus").unwrap();
        drop(host);
        let mut host = load();
        assert_eq!(host.installed["sample"].manifest.id, "sample_plus");
        assert!(!host.installed["sample"].settings.enabled);
        assert!(host.select_package("sample", "missing").is_err());
        host.select_package("sample", "sample").unwrap();
        host.set_enabled("sample", true).unwrap();
        drop(host);
        let host = load();
        assert_eq!(host.installed["sample"].manifest.id, "sample");
        assert!(host.installed["sample"].settings.enabled);
    }

    #[test]
    fn multi_model_selection_migrates_legacy_and_respects_groups() {
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("plugins");
        let directory = plugins.join("vision");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("plugin"), []).unwrap();
        std::fs::write(
            directory.join("plugin.json"),
            json!({
                "id": "vision",
                "min_host_version": "0.4.0",
                "title": {"en": "Vision"},
                "executable": "plugin",
                "models": [
                    {
                        "id": "matting-small",
                        "label": "Matting Small",
                        "directory": "vision/small",
                        "revision": "1",
                        "runtime": "onnx",
                        "group": "matting",
                        "files": [{"path": "a.onnx", "url": "https://example.com/a.onnx", "bytes": 100}]
                    },
                    {
                        "id": "matting-large",
                        "label": "Matting Large",
                        "directory": "vision/large",
                        "revision": "1",
                        "runtime": "onnx",
                        "group": "matting",
                        "files": [{"path": "b.onnx", "url": "https://example.com/b.onnx", "bytes": 200}]
                    },
                    {
                        "id": "audio-sep",
                        "label": "Audio Sep",
                        "directory": "vision/sep",
                        "revision": "1",
                        "runtime": "onnx",
                        "files": [{"path": "c.onnx", "url": "https://example.com/c.onnx", "bytes": 300}]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        // Seed a legacy single-model settings.json file in the host's data path.
        let data_dir = Host::load(root.path().to_owned()).data("vision");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("settings.json"),
            json!({"enabled": true, "model": "matting-small"}).to_string(),
        )
        .unwrap();

        let load = || {
            let mut host = Host::load(root.path().to_owned());
            host.catalog =
                xrtranslate_plugin::directory::discover_packages(&plugins, None, None).unwrap();
            host.sync_package("vision").unwrap();
            host
        };

        let mut host = load();
        assert!(host.installed["vision"].settings.has_model("matting-small"));
        assert!(!host.installed["vision"].settings.has_model("audio-sep"));

        // Selecting an ungrouped model keeps the existing model selected (multi-select).
        assert!(host.set_model_selected("vision", "audio-sep", true).unwrap());
        assert!(host.installed["vision"].settings.has_model("matting-small"));
        assert!(host.installed["vision"].settings.has_model("audio-sep"));
        assert!(host.installed["vision"].environment_dirty);

        // Selecting another model in the same group swaps within the group while keeping ungrouped models.
        assert!(host.set_model_selected("vision", "matting-large", true).unwrap());
        assert!(!host.installed["vision"].settings.has_model("matting-small"));
        assert!(host.installed["vision"].settings.has_model("matting-large"));
        assert!(host.installed["vision"].settings.has_model("audio-sep"));

        drop(host);
        let host = load();
        assert!(!host.installed["vision"].settings.has_model("matting-small"));
        assert!(host.installed["vision"].settings.has_model("matting-large"));
        assert!(host.installed["vision"].settings.has_model("audio-sep"));
    }
}

#[derive(Clone, Default)]
pub(crate) struct Sessions(Arc<Mutex<VecDeque<(String, Value)>>>);
impl crate::session_coordinator::SessionEventSubscriber for Sessions {
    fn on_translation_event(
        &self,
        owner: &crate::session_coordinator::TranslationSessionOwner,
        event: &crate::session_coordinator::TranslationEvent,
    ) {
        if let Some(plugin) = owner.plugin() {
            self.0.lock().unwrap().push_back((
                plugin.plugin_id().to_owned(),
                json!({"owner":owner,"event":event}),
            ));
        }
    }
}
