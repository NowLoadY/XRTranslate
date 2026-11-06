//! Installed plugins contribute views and operations; this adapter supplies host services.
mod models;
mod services;
mod ui;
use crate::{
    i18n::UiLanguage,
    plugins::{PluginId, PluginPreferences},
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
            } => Message::Event {
                name: "translation.committed".into(),
                value: json!({"stream":stream_id,"turn":turn_id,"segment":segment_index,"source":source,"translation":translated,"audio_source":audio_source,"speaker":speaker,"is_typing":is_typing,"additional":additional_translations.iter().map(|item|item.translated_text.as_str()).collect::<Vec<_>>()}),
            },
            HostOutputEvent::Clear => Message::Event {
                name: "translation.clear".into(),
                value: Value::Null,
            },
            HostOutputEvent::StreamCancelled(id) => Message::Event {
                name: "translation.cancelled".into(),
                value: json!({"stream":id}),
            },
            _ => return,
        };
        let mut queue = self.0.lock().unwrap();
        if queue.len() == 64 {
            queue.pop_front();
            log::warn!("Plugin output queue is full; oldest event discarded");
        }
        queue.push_back(message);
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
    directory: PathBuf,
    manifest: Manifest,
    settings: Settings,
    process: Option<Process>,
    error: Option<String>,
    page: ui::PageState,
}
struct Inference {
    plugin: String,
    id: u64,
    reply: crate::ask::Reply,
    cancel: Arc<AtomicBool>,
}
pub(crate) struct Host {
    root: PathBuf,
    directory: PathBuf,
    next_scan: std::time::Instant,
    models_root: PathBuf,
    resources: Option<(String, BTreeMap<String, PathBuf>)>,
    capabilities: Vec<String>,
    context: BTreeMap<String, Value>,
    listeners: BTreeMap<String, Vec<Box<dyn Fn(&Value)>>>,
    installed: BTreeMap<String, Installed>,
    pub output: Output,
    inference: Vec<Inference>,
    pub error: Option<String>,
    models: models::Downloads,
}
impl Host {
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
        let mut host = Self {
            root: runtime.runtime_root().join("plugins"),
            directory: root.join("plugins"),
            next_scan: std::time::Instant::now(),
            models_root,
            resources: None,
            capabilities: Vec::new(),
            context: BTreeMap::new(),
            listeners: BTreeMap::new(),
            installed: BTreeMap::new(),
            output: Output::default(),
            inference: Vec::new(),
            error: None,
            models: models::Downloads::default(),
        };
        host.refresh();
        host
    }
    pub fn refresh(&mut self) {
        if std::time::Instant::now() < self.next_scan {
            return;
        }
        self.next_scan = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let entries = match xrtranslate_plugin::directory::discover(&self.directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let mut present = std::collections::HashSet::new();
        let mut errors = Vec::new();
        for (directory, manifest) in entries {
            let manifest = match manifest {
                Ok(manifest) => manifest,
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
            let modified = std::fs::metadata(directory.join(&manifest.executable))
                .ok()
                .and_then(|m| m.modified().ok());
            if self.installed.get(&id).is_some_and(|entry| {
                entry.directory == directory
                    && entry.manifest == manifest
                    && entry.modified == modified
            }) {
                continue;
            }
            self.stop(&id);
            self.installed.remove(&id);
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
            self.models.cancel(&id);
            self.installed.remove(&id);
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
        let id = PluginId::parse(&manifest.id).ok_or("Invalid plugin ID")?;
        if crate::plugins::PluginRegistry::builtin()
            .descriptor(id)
            .is_some()
            || self.installed.contains_key(&manifest.id)
        {
            return Err(format!("Plugin ID already registered: {}", manifest.id));
        }
        let settings = std::fs::read(self.data(&manifest.id).join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        self.installed.insert(
            manifest.id.clone(),
            Installed {
                modified,
                error: xrtranslate_plugin::directory::require_host(
                    &manifest,
                    env!("CARGO_PKG_VERSION"),
                )
                .err(),
                directory,
                manifest,
                settings,
                process: None,
                page: ui::PageState::default(),
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
            || self
                .installed
                .values()
                .any(|p| p.settings.enabled && p.process.is_none() && p.error.is_none())
    }
    pub fn available(&self, id: &str) -> bool {
        self.installed
            .get(id)
            .is_some_and(|p| p.settings.enabled && p.process.is_some())
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
        entry.error =
            xrtranslate_plugin::directory::require_host(&entry.manifest, env!("CARGO_PKG_VERSION"))
                .err();
        self.save(id)
    }
    fn stop(&mut self, id: &str) {
        self.inference.retain(|request| {
            if request.plugin != id {
                return true;
            }
            request.cancel.store(true, Ordering::Release);
            false
        });
        if let Some(entry) = self.installed.get_mut(id) {
            entry.process.take();
        }
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        preferences: &mut PluginPreferences,
        language: UiLanguage,
        runtimes: BTreeMap<String, PathBuf>,
        capabilities: Option<Vec<String>>,
    ) -> Vec<(String, Message)> {
        if let Some(capabilities) = capabilities {
            self.capabilities = capabilities;
        }
        let capabilities = self.capabilities.clone();
        if let Some(id) = self.models.poll() {
            self.stop(&id);
        }
        let resources = (locale(language).to_owned(), runtimes.clone());
        if self
            .resources
            .as_ref()
            .is_some_and(|previous| previous != &resources)
        {
            for id in self.installed.keys().cloned().collect::<Vec<_>>() {
                self.stop(&id);
                let entry = self.installed.get_mut(&id).unwrap();
                entry.error = xrtranslate_plugin::directory::require_host(
                    &entry.manifest,
                    env!("CARGO_PKG_VERSION"),
                )
                .err();
            }
        }
        self.resources = Some(resources);
        let events: Vec<_> = self.output.0.lock().unwrap().drain(..).collect();
        let mut calls = Vec::new();
        let language = locale(language);
        let base = self.root.clone();
        let models_root = self.models_root.clone();
        for (id, entry) in &mut self.installed {
            if !entry.settings.enabled {
                continue;
            }
            if entry.process.is_none() && entry.error.is_none() {
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
                let repaint = ctx.clone();
                match Process::start(&entry.directory, &entry.manifest, environment, move || {
                    repaint.request_repaint()
                }) {
                    Ok(process) => {
                        for (name, value) in &self.context {
                            if entry.manifest.subscriptions.contains(name) {
                                let _ = process.send(Message::Event {
                                    name: name.clone(),
                                    value: value.clone(),
                                });
                            }
                        }
                        entry.process = Some(process);
                    }
                    Err(error) => entry.error = Some(error),
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
                if let Some(error) = process.error() {
                    log::warn!("Plugin {id}: {error}");
                    entry.error = Some(error);
                    entry.process.take();
                }
            }
        }
        preferences.installed_pages = self
            .installed
            .iter()
            .filter(|(_, p)| p.settings.enabled && p.manifest.page)
            .map(|(id, p)| {
                (
                    PluginId::parse(id).unwrap(),
                    p.manifest.title.get(language).into(),
                )
            })
            .collect();
        if self.models.busy() || !self.inference.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut ready = Vec::new();
        for request in &self.inference {
            match request.reply.try_recv() {
                Ok(value) => ready.push((
                    request.plugin.clone(),
                    request.id,
                    value.map(Value::String).map_err(|e| e.error),
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
    pub fn subscribe(&mut self, topic: &str, listener: impl Fn(&Value) + 'static) {
        self.listeners
            .entry(topic.into())
            .or_default()
            .push(Box::new(listener));
    }
    pub fn publish(&self, name: &str, value: Value) {
        if let Some(listeners) = self.listeners.get(name) {
            for listener in listeners {
                listener(&value);
            }
        }
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
impl Drop for Host {
    fn drop(&mut self) {
        for request in &self.inference {
            request.cancel.store(true, Ordering::Release);
        }
    }
}
fn locale(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::Chinese => "zh",
        UiLanguage::Japanese => "ja",
        UiLanguage::Korean => "ko",
        UiLanguage::Russian => "ru",
        UiLanguage::English => "en",
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
                json!({"directory":self.directory,"plugins":self.installed.iter().map(|(id,p)| json!({"id":id,"title":p.manifest.title,"version":p.manifest.version,"enabled":p.settings.enabled,"running":p.process.is_some(),"error":p.error})).collect::<Vec<_>>(),"error":self.error}),
            ),
            "plugins.enable" => {
                self.set_enabled(
                    string("id")?,
                    args["enabled"].as_bool().ok_or("Missing enabled")?,
                )?;
                Ok(json!({"updated":true}))
            }
            "plugins.operation" => {
                let plugin = string("plugin")?;
                let id = args["id"].as_u64().ok_or("Missing operation id")?;
                let process = self
                    .installed
                    .get_mut(plugin)
                    .and_then(|p| p.process.as_mut())
                    .ok_or("Plugin is stopped")?;
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
    json!({"pending":{"name":"plugins.operation","arguments":{"plugin":plugin,"id":id}}})
}
