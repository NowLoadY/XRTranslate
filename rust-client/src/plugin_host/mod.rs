//! Installed plugins contribute views and operations; this adapter supplies host services.
mod datagram;
mod io;
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

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    enabled: bool,
    model: Option<String>,
    config: Value,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            config: Value::Null,
        }
    }
}
struct Installed {
    modified: Option<std::time::SystemTime>,
    icon: Option<(String, Arc<[u8]>)>,
    directory: PathBuf,
    executable: PathBuf,
    manifest: Manifest,
    settings: Settings,
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
type Scan = Result<
    Vec<(
        PathBuf,
        Result<(Manifest, Option<std::time::SystemTime>), String>,
    )>,
    String,
>;

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
    navigation_changed: bool,
    models_root: PathBuf,
    resources: Option<(String, BTreeMap<String, PathBuf>)>,
    capabilities: Vec<String>,
    context: BTreeMap<String, Value>,
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
            navigation_changed: true,
            io: io::Worker::new(),
            models_root,
            resources: None,
            capabilities: Vec::new(),
            context: BTreeMap::new(),
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
        if self.scan.is_none() && std::time::Instant::now() >= self.next_scan {
            let directory = self.directory.clone();
            let packaged_directory = self.packaged_directory.clone();
            let executables = self.executables.clone();
            self.scan = Some(self.io.submit(
                move || {
                    let mut entries = xrtranslate_plugin::directory::discover(
                        &directory,
                        executables.as_deref(),
                    )?;
                    if let Some(directory) = packaged_directory {
                        let user_ids: std::collections::HashSet<_> = entries
                            .iter()
                            .filter_map(|(_, manifest)| {
                                manifest.as_ref().ok().map(|manifest| manifest.id.clone())
                            })
                            .collect();
                        entries.extend(
                            xrtranslate_plugin::directory::discover(&directory, None)?
                                .into_iter()
                                .filter(|(_, manifest)| {
                                    manifest
                                        .as_ref()
                                        .map_or(true, |manifest| !user_ids.contains(&manifest.id))
                                }),
                        );
                    }
                    Ok(entries
                        .into_iter()
                        .map(|(directory, manifest)| {
                            let entry = manifest.map(|manifest| {
                                let modified = std::fs::metadata(
                                    executables
                                        .as_deref()
                                        .unwrap_or(&directory)
                                        .join(&manifest.executable),
                                )
                                .ok()
                                .and_then(|m| m.modified().ok());
                                (manifest, modified)
                            });
                            (directory, entry)
                        })
                        .collect())
                },
                Some(ctx.clone()),
            ));
        }
        let Some(result) = self.scan.as_ref().and_then(|scan| scan.try_recv().ok()) else {
            return;
        };
        self.scan = None;
        self.next_scan = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let entries = match result {
            Ok(entries) => entries,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let mut present = std::collections::HashSet::new();
        let mut errors = Vec::new();
        for (directory, manifest) in entries {
            let (manifest, modified) = match manifest {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!("{}: {error}", directory.display()));
                    continue;
                }
            };
            let id = manifest.id.clone();
            if !present.insert(id.clone()) {
                errors.push(format!("Duplicate plugin ID: {id}"));
                continue;
            }
            if self.installed.get(&id).is_some_and(|entry| {
                entry.directory == directory
                    && entry.manifest == manifest
                    && entry.modified == modified
            }) {
                continue;
            }
            self.stop(&id);
            self.installed.remove(&id);
            self.navigation_changed = true;
            self.context.remove("host.context");
            if let Err(error) = self.register(directory, manifest, modified) {
                errors.push(error);
            }
        }
        for id in self
            .installed
            .keys()
            .filter(|id| !present.contains(*id))
            .cloned()
            .collect::<Vec<_>>()
        {
            self.stop(&id);
            self.installed.remove(&id);
            self.navigation_changed = true;
            self.context.remove("host.context");
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
    fn register(
        &mut self,
        directory: PathBuf,
        manifest: Manifest,
        modified: Option<std::time::SystemTime>,
    ) -> Result<(), String> {
        PluginId::parse(&manifest.id).ok_or("Invalid plugin ID")?;
        if self.installed.contains_key(&manifest.id) {
            return Err(format!("Plugin ID already registered: {}", manifest.id));
        }
        let settings = std::fs::read(self.data(&manifest.id).join("settings.json"))
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
                .and_then(|value| legacy_enablement(&value, &manifest.id))
                .unwrap_or(true);
                Settings {
                    enabled,
                    ..Default::default()
                }
            });
        self.installed.insert(
            manifest.id.clone(),
            Installed {
                modified,
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
        let path = self.data(id);
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let staged = path.join("settings.json.tmp");
        std::fs::write(
            &staged,
            serde_json::to_vec_pretty(&entry.settings).map_err(|e| e.to_string())?,
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
                    && p.process.is_none()
                    && p.starting.is_none()
                    && p.error.is_none()
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
                let environment = Environment {
                    host_executable: std::env::current_exe().unwrap_or_default(),
                    language: language.into(),
                    data_directory: base.join("data").join(id),
                    model_directory: entry
                        .settings
                        .model
                        .as_ref()
                        .and_then(|selected| {
                            entry.manifest.models.iter().find(|m| &m.id == selected)
                        })
                        .map(|model| models_root.join(&model.directory)),
                    runtimes: runtimes.clone(),
                    config: entry.settings.config.clone(),
                    capabilities: capabilities.clone(),
                };
                if let Some(process) = &entry.process {
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
                "Enable or disable an installed plugin.",
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
                json!({"directory":self.directory,"plugins":self.installed.iter().map(|(id,p)| json!({"id":id,"title":p.manifest.title,"enabled":p.settings.enabled,"running":p.process.is_some(),"error":p.error})).collect::<Vec<_>>(),"error":self.error}),
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
