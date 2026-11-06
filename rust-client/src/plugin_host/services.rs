use super::*;
impl crate::XRTranslateApp {
    pub(crate) fn render_installed_page(&mut self, ui: &mut egui::Ui, id: PluginId) {
        use crate::ui::{
            components::text_composer::{ComposerContext, TextAction},
            pages::translation,
        };
        for (capability, enabled) in self.plugin_host.page(ui, id, self.ui_language) {
            match capability.as_str() {
                "audio.capture" => {
                    translation::render_capture_control(
                        self,
                        ui,
                        crate::client_settings::CaptureSource::Microphone,
                    );
                    translation::render_capture_control(
                        self,
                        ui,
                        crate::client_settings::CaptureSource::SystemAudio,
                    );
                }
                "text.compose" => {
                    let capabilities = self.language_capabilities().for_text();
                    let action = self.plugin_host.composer(id).render(
                        ui,
                        id.as_str(),
                        (&mut self.source_lang, &mut self.target_lang),
                        ComposerContext {
                            language: self.ui_language,
                            capabilities,
                            preparing: self.text_translation.preparing_host(),
                            translation_enabled: enabled,
                            allow_direct: true,
                            direct_enabled: enabled,
                            show_languages: true,
                        },
                    );
                    let accepted = match action {
                        Some(TextAction::Translate {
                            text,
                            source_lang,
                            target_lang,
                        }) => self.submit_text_translation(
                            &text,
                            Some(source_lang),
                            Some(target_lang),
                            None,
                        ),
                        Some(TextAction::Direct(text)) => self
                            .plugin_host
                            .ui_action(id, json!({"id":"text.compose","value":text}))
                            .is_ok(),
                        None => false,
                    };
                    if accepted {
                        self.plugin_host.composer(id).text.clear();
                    }
                }
                _ => {
                    crate::ui::components::error_notice(
                        ui,
                        self.ui_language,
                        &format!("Unavailable UI capability: {capability}"),
                    );
                }
            }
        }
    }
    pub(crate) fn runtime_requirements(&self) -> xrtranslate_config::RuntimeRequirements {
        let mut requirements = self.service_config.runtime_requirements();
        for entry in self
            .plugin_host
            .installed
            .values()
            .filter(|entry| entry.settings.enabled)
        {
            if let Some(model) = entry
                .settings
                .model
                .as_ref()
                .and_then(|id| entry.manifest.models.iter().find(|m| &m.id == id))
            {
                match model.runtime.as_str() {
                    "onnx-cpu" => requirements.onnx_cpu = true,
                    "onnx-cuda" => requirements.onnx_cuda = true,
                    "llama.cpp" => requirements.llama_cpp = true,
                    _ => {}
                }
            }
        }
        requirements
    }
    pub(crate) fn poll_installed_plugins(&mut self, ctx: &egui::Context) {
        self.plugin_host.refresh();
        let visible = match self.navigation.page {
            crate::ui::Page::Plugin(id)
                if !self.first_run && ctx.input(|i| i.viewport().visible().unwrap_or(true)) =>
            {
                Some(id.as_str().to_owned())
            }
            _ => None,
        };
        if visible != self.plugin_host.visible {
            if let Some(previous) = self.plugin_host.visible.take() {
                self.plugin_host
                    .send_event(&previous, "ui.visibility", json!(false));
            }
            if let Some(current) = &visible {
                self.plugin_host
                    .send_event(current, "ui.visibility", json!(true));
            }
            self.plugin_host.visible = visible;
        }
        for plugin in std::mem::take(&mut self.plugin_host.stopped) {
            let owners = self
                .audio_tasks
                .iter()
                .map(|task| task.owner.clone())
                .chain(
                    self.pending_translations
                        .iter()
                        .map(crate::session_coordinator::TranslationTask::owner),
                )
                .chain(
                    self.text_translation
                        .scopes()
                        .map(|scope| scope.owner.clone()),
                )
                .filter(|owner| owner.is_plugin(&plugin))
                .collect::<Vec<_>>();
            for owner in owners {
                self.stop_task_owner(&owner);
            }
        }
        let events = self
            .plugin_host
            .sessions
            .0
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        for (plugin, value) in events {
            self.plugin_host
                .send_event(&plugin, "translation.event", value);
        }
        for (plugin, id, started) in std::mem::take(&mut self.plugin_host.preparing) {
            use crate::backend::BackendStatus;
            let result = match self.backend_manager.status(&self.server_url) {
                BackendStatus::Ready => Some(Ok(json!({"server":self.server_url}))),
                BackendStatus::Failed(error) => Some(Err(error)),
                BackendStatus::Starting(_) if started.elapsed().as_secs() >= 180 => {
                    Some(Err("Model startup timed out".into()))
                }
                _ => None,
            };
            if let Some(result) = result {
                self.plugin_host.reply(&plugin, id, result)
            } else {
                self.plugin_host.preparing.push((plugin, id, started));
            }
        }
        if self.plugin_host.context_updated.elapsed().as_millis() >= 250 {
            self.refresh_plugin_context();
        }
        let mut runtimes = BTreeMap::new();
        let core = self
            .backend_manager
            .runtime_layout()
            .onnx_cpu_core_library();
        if core.is_file() {
            runtimes.insert("onnx".into(), core);
        }
        let capabilities = if self
            .plugin_host
            .needs_environment(self.ui_language, &runtimes)
        {
            let mut names: Vec<_> = self
                .director_tools()
                .into_iter()
                .map(|tool| tool.name)
                .collect();
            names.extend(
                [
                    "bookmarks.query",
                    "audio.capture",
                    "text.compose",
                    "ui",
                    "translation.start",
                    "translation.stop",
                    "translation.pause",
                    "translation.resume",
                    "translation.finish",
                    "models.preprocessing",
                    "backend.prepare",
                    "playback",
                ]
                .map(str::to_owned),
            );
            if cfg!(windows) {
                names.push("platform.windows".into());
            }
            if self.service_config.agent_available() || self.service_config.supports_ask() {
                names.push("inference.request".into());
            }
            Some(names)
        } else {
            None
        };
        let calls = self.plugin_host.poll(
            ctx,
            &mut self.plugin_preferences,
            self.ui_language,
            runtimes,
            capabilities,
        );
        for (plugin, message) in calls {
            match message {
                Message::Call {
                    id,
                    name,
                    arguments,
                } if name == "bookmarks.query" => match serde_json::from_value(arguments) {
                    Ok(action) => self.plugin_host.bookmarks.push((
                        plugin,
                        id,
                        self.history_archive.vocabulary(action, ctx),
                    )),
                    Err(error) => self.plugin_host.reply(&plugin, id, Err(error.to_string())),
                },
                Message::Call {
                    id,
                    name,
                    arguments,
                } if name == "inference.request" => {
                    let request = match serde_json::from_value(arguments) {
                        Ok(request) => request,
                        Err(error) => {
                            self.plugin_host.reply(&plugin, id, Err(error.to_string()));
                            continue;
                        }
                    };
                    let cancel = Arc::new(AtomicBool::new(false));
                    let reply = self
                        .ask
                        .request_with_cancel(request, Some(cancel.clone()), ctx);
                    self.plugin_host.inference(plugin, id, reply, cancel);
                }
                Message::Call {
                    id,
                    name,
                    arguments,
                } if matches!(
                    name.as_str(),
                    "translation.start"
                        | "translation.stop"
                        | "translation.pause"
                        | "translation.resume"
                        | "translation.finish"
                        | "models.preprocessing"
                        | "backend.prepare"
                ) =>
                {
                    let result = self.plugin_service(&plugin, &name, arguments, ctx);
                    if name == "backend.prepare" && matches!(&result,Ok(value) if value.is_null()) {
                        self.plugin_host
                            .preparing
                            .push((plugin, id, std::time::Instant::now()));
                    } else {
                        self.plugin_host.reply(&plugin, id, result);
                    }
                }
                Message::Call {
                    id,
                    name,
                    arguments,
                } => {
                    let result =
                        self.call_director_tool(&ui_director::ToolCall { name, arguments });
                    self.plugin_host.operation_reply(plugin, id, result);
                }
                Message::Cancel { id } => {
                    self.plugin_host.cancel_operations(&plugin, Some(id));
                    self.plugin_host
                        .preparing
                        .retain(|(p, i, _)| p != &plugin || *i != id);
                }
                Message::Event { name, value } if name == "playback" => {
                    if let Some(id) = value["id"].as_u64() {
                        let result = serde_json::from_value(value["command"].clone())
                            .map_err(|e| e.to_string())
                            .and_then(|command| {
                                self.plugin_host.playback.command(&plugin, id, command)
                            });
                        if let Err(error) = result {
                            log::warn!("Plugin {plugin} playback {id}: {error}");
                            self.plugin_host.send_event(
                                &plugin,
                                "playback.error",
                                json!({"id":id,"error":error}),
                            );
                        }
                    }
                }
                Message::Event { name, value } if name == "state" => {
                    if let Some(entry) = self.plugin_host.installed.get_mut(&plugin) {
                        entry.state = value.clone();
                    }
                    self.plugin_host
                        .publish("plugin.state", json!({"plugin":plugin,"state":value}));
                }
                Message::Event { name, value } if name == "notice" => {
                    if let Some(message) = value["message"].as_str() {
                        self.notifications
                            .error(message, value["details"].as_str().unwrap_or(""));
                    }
                }
                Message::Event { name, value } if name == "message" => {
                    if let Some(message) = value.as_str() {
                        self.companion_inbox.post_text(&plugin, message.to_owned());
                    }
                }
                Message::Event { name, value } if name == "ui.frame" => {
                    if let Some(entry) = self.plugin_host.installed.get_mut(&plugin) {
                        match serde_json::from_value(value) {
                            Ok(frame) => entry.page.surface.accept(ctx, frame),
                            Err(error) => entry.error = Some(error.to_string()),
                        }
                    }
                }
                Message::Event { name, value } if name == "config" => {
                    if let Some(entry) = self.plugin_host.installed.get_mut(&plugin) {
                        entry.settings.config = value;
                    }
                    if let Err(error) = self.plugin_host.save(&plugin) {
                        log::warn!("Plugin settings: {error}");
                    }
                }
                Message::Event { name, value } => self.plugin_host.publish(&name, value),
                _ => {}
            }
        }
        for (plugin, value) in self.plugin_host.playback.poll() {
            self.plugin_host
                .send_event(&plugin, "playback.state", value);
        }
        self.poll_plugin_operations();
        for (plugin, request) in ui_director::egui::driver().take_surface_requests() {
            let result = self
                .plugin_host
                .installed
                .get_mut(&plugin)
                .and_then(|entry| entry.process.as_mut())
                .ok_or("Plugin unavailable".to_owned())
                .and_then(|process| {
                    process.request(
                        "ui.command",
                        serde_json::to_value(&request.command).unwrap(),
                    )
                });
            match result {
                Ok(id) => self
                    .plugin_host
                    .surface_requests
                    .push((plugin, id, request)),
                Err(error) => {
                    let _ = request.responder.send(ui_director::Response::err(error));
                }
            }
        }
        for (plugin, id, request) in std::mem::take(&mut self.plugin_host.surface_requests) {
            let process = self
                .plugin_host
                .installed
                .get_mut(&plugin)
                .and_then(|entry| entry.process.as_mut());
            let result = if request.cancelled.load(Ordering::Acquire)
                || request.expires <= std::time::Instant::now()
            {
                if let Some(process) = process {
                    process.cancel(id);
                }
                Some(Err("UI action cancelled or expired".into()))
            } else {
                process.map_or(Some(Err("Plugin unavailable".into())), |process| {
                    process.result(id)
                })
            };
            if let Some(result) = result {
                let response = match result {
                    Ok(value) => ui_director::Response::ok("UI action completed", Some(value)),
                    Err(error) => ui_director::Response::err(error),
                };
                let _ = request.responder.send(response);
            } else {
                self.plugin_host
                    .surface_requests
                    .push((plugin, id, request));
            }
        }
    }

    pub(crate) fn refresh_plugin_context(&mut self) {
        self.plugin_host.set_context(
            "bookmarks.revision",
            json!(self.history_archive.vocabulary_revision()),
        );
        let services = xrtranslate_client_core::services::Capabilities {
            source_language: self.source_lang.clone(),
            target_language: self.target_lang.clone(),
            capture: self.capture_source,
            profiles: crate::CaptureSource::Both
                .routes()
                .iter()
                .map(|source| (*source, self.recognition_settings(*source).clone()))
                .collect(),
            microphone_enabled: self.microphone_enabled,
            system_audio_enabled: self.system_audio_enabled,
            language: self.ui_language,
            languages: self.language_capabilities(),
            preprocessing: self.audio_preprocessing_models(),
            default_preprocessing: self.service_config.default_audio_preprocessing(),
            download_proxy: self.download_proxy_url.clone(),
            ask: self.service_config.supports_ask(),
            agent: self.service_config.agent_available()
                && self.plugin_host.enabled(PluginId::AGENT),
            tts: self.service_config.tts_is_configured(),
        };
        self.plugin_host
            .set_context("host.services", json!(services));
        let tools = self.director_tools();
        self.plugin_host.set_context(
            "host.context",
            json!({
                "language": self.ui_language.display_name(),
                "location": ui_director::egui::driver().location(),
                "inference": {
                    "agent": self.service_config.agent_available(),
                    "ask": self.service_config.supports_ask(),
                    "input_budget": self.service_config.agent_input_budget(),
                },
                "tools": tools,
            }),
        );
        self.plugin_host.context_updated = std::time::Instant::now();
    }
}

impl crate::XRTranslateApp {
    fn plugin_service(
        &mut self,
        plugin: &str,
        name: &str,
        arguments: Value,
        ctx: &egui::Context,
    ) -> xrtranslate_plugin::Result {
        use crate::session_coordinator::{TranslationSessionOwner, TranslationTask};
        match name {
            "translation.start" => {
                let task: TranslationTask =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                if !task.owner().is_plugin(plugin) {
                    return Err("Translation owner must be the calling plugin".into());
                }
                let owner = task.owner();
                self.start_translation_task(task, Some(ctx.clone()));
                Ok(json!(owner))
            }
            "translation.pause" | "translation.resume" | "translation.finish" => {
                let owner: TranslationSessionOwner =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                if !owner.is_plugin(plugin) {
                    return Err("Translation owner must be the calling plugin".into());
                }
                self.control_plugin_audio(&owner, name)?;
                Ok(Value::Null)
            }
            "translation.stop" => {
                let owner: TranslationSessionOwner =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                if !owner.is_plugin(plugin) {
                    return Err("Translation owner must be the calling plugin".into());
                }
                self.stop_task_owner(&owner);
                Ok(Value::Null)
            }
            "models.preprocessing" => {
                self.apply_audio_preprocessing_action(
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?,
                );
                Ok(Value::Null)
            }
            "backend.prepare" => {
                use crate::backend::BackendStart;
                match self.backend_manager.prepare(&self.server_url)? {
                    BackendStart::Ready => Ok(json!({"server":self.server_url})),
                    BackendStart::Starting(_) => Ok(Value::Null),
                }
            }
            _ => Err("Unknown host service".into()),
        }
    }
}

impl crate::XRTranslateApp {
    fn control_plugin_audio(
        &mut self,
        owner: &crate::session_coordinator::TranslationSessionOwner,
        operation: &str,
    ) -> Result<(), String> {
        let index = self
            .audio_tasks
            .iter()
            .position(|task| &task.owner == owner)
            .ok_or("Audio task is no longer active")?;
        let mut task = self.audio_tasks.remove(index);
        let result = (|| -> Result<(), String> {
            task.paused = operation != "translation.resume";
            for channel in &mut task.channels {
                match operation {
                    "translation.resume" => {
                        if channel.audio_tx.is_some() {
                            self.start_task_capture(channel)?;
                            channel.capturing = true;
                        }
                        channel.session.resume();
                    }
                    "translation.finish" => {
                        self.audio_system.stop_capture_group(channel.scope.id());
                        channel.capturing = false;
                        channel.audio_tx.take();
                        channel.session.finish();
                    }
                    _ => {
                        channel.session.pause();
                        self.audio_system.stop_capture_group(channel.scope.id());
                        channel.capturing = false;
                    }
                }
            }
            if operation == "translation.finish" {
                if let Some(import) = &task.import {
                    import.stop();
                }
            }
            if let Some(recording) = &task.recording {
                recording.checkpoint().map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if result.is_err() {
            task.paused = true;
            for channel in &mut task.channels {
                channel.session.pause();
                self.audio_system.stop_capture_group(channel.scope.id());
                channel.capturing = false;
            }
        }
        self.audio_tasks.insert(index, task);
        self.release_unused_inputs();
        result
    }
}
