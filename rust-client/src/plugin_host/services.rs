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
            names.extend(["audio.capture".into(), "text.compose".into(), "ui".into()]);
            if cfg!(windows) {
                names.push("platform.windows".into());
            }
            if self.service_config.agent_available() || self.service_config.supports_ask() {
                names.push("text.ask".into());
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
                } if name == "text.ask" => {
                    let target = if self.service_config.agent_available() {
                        Some(xrtranslate_protocol::AskTarget::Agent)
                    } else if self.service_config.supports_ask() {
                        Some(xrtranslate_protocol::AskTarget::Translation)
                    } else {
                        None
                    };
                    let Some(target) = target else {
                        self.plugin_host.reply(
                            &plugin,
                            id,
                            Err("Choose a model that supports text instructions".into()),
                        );
                        continue;
                    };
                    let messages = match serde_json::from_value(arguments["messages"].clone()) {
                        Ok(messages) => messages,
                        Err(error) => {
                            self.plugin_host.reply(&plugin, id, Err(error.to_string()));
                            continue;
                        }
                    };
                    let cancel = Arc::new(AtomicBool::new(false));
                    let reply = self.ask.request_with_cancel(
                        xrtranslate_protocol::AskRequest::Messages {
                            messages,
                            target,
                            response_schema: None,
                        },
                        Some(cancel.clone()),
                        ctx,
                    );
                    self.plugin_host.inference(plugin, id, reply, cancel);
                }
                Message::Call {
                    id,
                    name,
                    arguments,
                } => {
                    let result =
                        self.call_director_tool(&ui_director::ToolCall { name, arguments });
                    self.plugin_host.reply(&plugin, id, result);
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
    }
}
