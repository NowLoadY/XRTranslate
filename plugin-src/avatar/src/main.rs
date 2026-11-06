mod avatar;
mod selection;
use avatar::{AvatarPlugin, HostAction, HostResult, OperationId};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
pub use xrtranslate_client_core::{
    i18n, osc as osc_transport, recognition, session_coordinator, text_output, ui,
};
use xrtranslate_client_core::{
    services::{Capabilities, LiveState},
    session_coordinator::{
        HostOutputSubscriber, TranslationInput, TranslationSessionOwner, TranslationTask,
    },
};
use xrtranslate_plugin::{Context, Environment, Plugin, Request, surface};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    enabled: bool,
    connector: avatar::connectors::Settings,
}
#[derive(Default)]
struct App {
    avatar: AvatarPlugin,
    enabled: bool,
    initialized: bool,
    transport: Option<osc_transport::OscTransportHandle>,
    services: Option<Capabilities>,
    session: Option<(OperationId, TranslationSessionOwner)>,
    start: Option<(OperationId, Request)>,
}
impl App {
    fn save(&self, context: &mut Context) {
        context.event(
            "config",
            json!(Config {
                enabled: self.enabled,
                connector: self.avatar.settings.clone()
            }),
        );
    }
    fn start(&mut self, operation: OperationId, context: &mut Context) -> Result<(), String> {
        if self.session.is_some() {
            return Err("The previous translation is still stopping".into());
        }
        let services = self
            .services
            .as_ref()
            .ok_or("Translation service is not ready")?;
        let languages = services.languages.select_with_options(
            &services.source_language,
            &services.target_language,
            services.text.additional_target.as_deref(),
            services.text.asr_only,
        )?;
        let binding = avatar::session_binding(operation);
        let owner = TranslationSessionOwner::Plugin(binding.owner.clone());
        let task = TranslationTask {
            languages,
            context: avatar::translation_context(operation),
            text_workload: None,
            plugin: Some(binding),
            input: TranslationInput::ExclusiveLive(services.capture),
            profiles: services.profiles.clone(),
        };
        self.session = Some((operation, owner));
        self.start = Some((
            operation,
            context.client().request("translation.start", json!(task)),
        ));
        Ok(())
    }
}
impl Plugin for App {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        let config = if environment.config.is_null() {
            environment.data_directory.ancestors().nth(3)
                .and_then(|root|std::fs::read(root.join("rust-client-settings.json")).ok())
                .and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok())
                .map(|value|json!({"enabled":value["game_plugins"]["enabled"],"connector":value["avatar_connector"]}))
                .unwrap_or(Value::Null)
        } else {
            environment.config
        };
        let config: Config = serde_json::from_value(config).unwrap_or_default();
        self.enabled = config.enabled;
        self.avatar = AvatarPlugin::new(config.connector);
        self.transport = Some(osc_transport::OscTransportHandle::new(context.client()));
        self.initialized = true;
        self.save(context);
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Unknown game operation".into())
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        let result = (|| -> Result<(), String> {
            match name {
                "host.services" => {
                    self.services = Some(serde_json::from_value(value).map_err(|e| e.to_string())?)
                }
                "translation.output" => self
                    .avatar
                    .output
                    .on_host_output(serde_json::from_value(value).map_err(|e| e.to_string())?),
                "osc.control" => self.transport.as_ref().unwrap().observe(value)?,
                "translation.state" => {
                    let owner: TranslationSessionOwner =
                        serde_json::from_value(value["owner"].clone())
                            .map_err(|e| e.to_string())?;
                    let state: LiveState = serde_json::from_value(value["state"].clone())
                        .map_err(|e| e.to_string())?;
                    if let Some((operation, _)) = self
                        .session
                        .as_ref()
                        .filter(|(_, current)| *current == owner)
                    {
                        let operation = *operation;
                        let result = match state {
                            LiveState::Running => HostResult::Started,
                            LiveState::Stopped => HostResult::Stopped,
                            LiveState::Failed(error) => HostResult::Failed(error),
                        };
                        if !matches!(result, HostResult::Started) {
                            self.avatar.request_host_stop(operation);
                            self.session = None;
                            self.start = None;
                        }
                        self.avatar.report_host_result(operation, result);
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            context.event("notice", json!({"message":error}));
        }
    }
    fn reply(&mut self, _: u64, result: xrtranslate_plugin::Result, context: &mut Context) {
        if let Err(error) = result {
            context.event("notice", json!({"message":error}));
        }
    }
    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        if let Some((operation, result)) = self
            .start
            .as_ref()
            .and_then(|(op, request)| request.poll().map(|r| (*op, r)))
        {
            self.start = None;
            if let Err(error) = result {
                self.session = None;
                self.avatar
                    .report_host_result(operation, HostResult::Failed(error));
            }
        }
        self.avatar.poll(self.enabled, self.transport.as_ref()?);
        for action in self.avatar.take_host_actions() {
            match action {
                HostAction::Start(operation) => {
                    if let Err(error) = self.start(operation, context) {
                        self.avatar
                            .report_host_result(operation, HostResult::Failed(error));
                    }
                }
                HostAction::Cancel(operation) => {
                    if let Some((_, owner)) = self
                        .session
                        .as_ref()
                        .filter(|(current, _)| *current == operation)
                    {
                        context.call("translation.stop", json!(owner));
                    } else {
                        self.avatar
                            .report_host_result(operation, HostResult::Stopped);
                    }
                }
            }
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn configure_ui(&mut self, ctx: &egui::Context, options: &Value) {
        ui::configure_surface(ctx, options);
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        let Some(language) = self.services.as_ref().map(|s| s.language) else {
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("game_plugins_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(i18n::tr(language, "Game Plugins"))
                            .size(22.0)
                            .color(ui::theme::text_strong())
                            .strong(),
                    );
                    ui.add_space(8.0);
                    changed |= selection::render_selection(
                        ui,
                        &mut self.avatar.settings.connector,
                        language,
                    );
                    changed |= ui::components::toggle_with_label(
                        ui,
                        &mut self.enabled,
                        i18n::tr(language, "Enable"),
                    )
                    .changed();
                });
                ui.add_space(16.0);
                changed |= self.avatar.render_content(ui, language, self.enabled);
                if changed {
                    self.save(context);
                }
                ui.add_space(16.0);
                ui::components::showcase(ui, language, &ui::previews::game_plugins());
                ui.add_space(8.0);
            });
        ui::forward_notices(ui.ctx(), context);
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(error) = surface::serve(App::default()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
