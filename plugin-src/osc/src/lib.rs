mod osc;
use eframe::egui;
pub use osc::OscGame;
use osc::{OscPageContext, OscPlugin, OscUiAction, runtime::OscSettings};
use serde_json::{Value, json};
use std::time::Duration;
pub use xrtranslate_client_core::{
    compact_speaker_label, feature_access, i18n, recognition as client_settings,
    session_coordinator, text_output, ui,
};
use xrtranslate_client_core::{
    services::{Capabilities, TextAction},
    session_coordinator::HostOutputSubscriber,
};
use xrtranslate_plugin::{Context, Environment, Plugin, Request, surface};

struct App {
    game: OscGame,
    output: Option<OscPlugin>,
    metrics: text_output::TextOutputHandle,
    services: Option<Capabilities>,
    submit: Option<Request>,
    publication: Option<(u64, osc::runtime::VoicePublishReply)>,
    state: Value,
    lyrics_available: bool,
}
impl App {
    fn id(&self) -> &'static str {
        if self.game == OscGame::VRChat {
            "osc"
        } else {
            "chilloutvr_osc"
        }
    }
    fn apply(&mut self, context: &mut Context) -> Result<(), String> {
        let output = self.output.as_mut().unwrap();
        output.draft().validate()?;
        output.apply_draft()?;
        context.event("config", json!(output.draft()));
        Ok(())
    }
    fn action(&mut self, action: OscUiAction, context: &mut Context) -> Result<(), String> {
        let host_action = match action {
            OscUiAction::ClearHostHistory => Some(TextAction::Clear),
            OscUiAction::SetMicrophoneSyncEnabled(enabled) => {
                Some(TextAction::MicrophoneSync { enabled })
            }
            OscUiAction::SetLanguageRoute {
                source_lang,
                target_lang,
                asr_only,
            } => Some(TextAction::Language {
                source: source_lang,
                target: target_lang,
                asr_only,
            }),
            OscUiAction::TranslateInput { text } => {
                if self.submit.is_none() {
                    self.submit = Some(
                        context
                            .client()
                            .request("text.action", json!(TextAction::Submit { text })),
                    );
                }
                None
            }
            OscUiAction::DirectInput(text) => {
                let output = self.output.as_mut().unwrap();
                if output.applied().enabled {
                    output.send_manual_message(&text);
                    output.draft_input_mut().clear();
                }
                None
            }
            OscUiAction::SetSpeakerNumberVisible(enabled) => {
                self.output
                    .as_mut()
                    .unwrap()
                    .draft_mut()
                    .show_speaker_number = enabled
                    && feature_access::is_available(feature_access::Feature::SpeakerNumbers);
                self.apply(context)?;
                None
            }
            OscUiAction::SetOutputEnabled(enabled) => {
                self.output.as_mut().unwrap().draft_mut().enabled = enabled;
                self.apply(context)?;
                None
            }
            OscUiAction::ApplySettings => {
                self.apply(context)?;
                None
            }
        };
        if let Some(action) = host_action {
            context.call("text.action", json!(action));
        }
        Ok(())
    }
    fn notice(context: &mut Context, error: String) {
        context.event(
            "notice",
            json!({"message":"Could not apply OSC settings.","details":error}),
        );
    }
    fn receipt(&self, operation: u64) -> Value {
        json!({"pending":{"name":format!("{}.voice", self.id()),"arguments":{"action":"result","operation":operation}}})
    }
}
impl Plugin for App {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> Result<(), String> {
        if self.output.is_some() {
            return Ok(());
        }
        let config = if environment.config.is_null() {
            environment
                .data_directory
                .ancestors()
                .nth(3)
                .and_then(|root| std::fs::read(root.join("rust-client-settings.json")).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|settings| settings.get(&format!("{}_settings", self.id())).cloned())
                .unwrap_or(Value::Null)
        } else {
            environment.config
        };
        let mut settings: OscSettings = serde_json::from_value(config).unwrap_or_else(|_| {
            if self.game == OscGame::VRChat {
                OscSettings::from_project_config()
            } else {
                OscSettings {
                    enabled: false,
                    ..OscSettings::for_game(self.game)
                }
            }
        });
        settings.game = self.game;
        settings.enabled &= feature_access::is_available(feature_access::Feature::OscChatbox);
        settings.show_speaker_number &=
            feature_access::is_available(feature_access::Feature::SpeakerNumbers);
        settings.history_ttl_seconds = settings.history_ttl_seconds.clamp(10.0, 20.0);
        self.output = Some(OscPlugin::new(
            self.game,
            settings,
            self.metrics.clone(),
            context.client(),
        ));
        self.apply(context)
    }
    fn call(
        &mut self,
        name: &str,
        arguments: Value,
        _: &mut Context,
    ) -> xrtranslate_plugin::Result {
        if name != format!("{}.voice", self.id()) {
            return Err("Unknown output operation".into());
        }
        let output = self.output.as_ref().ok_or("Output unavailable")?;
        match arguments["action"].as_str() {
            Some("prepare") => Ok(json!(
                output.prepare_voice_text(arguments["text"].as_str().ok_or("Missing text")?)?
            )),
            Some("publish") => {
                if self.publication.is_some() {
                    return Err("Another voice publication is pending".into());
                }
                let operation = arguments["operation"].as_u64().ok_or("Missing operation")?;
                let text =
                    output.prepare_voice_text(arguments["text"].as_str().ok_or("Missing text")?)?;
                self.publication = Some((operation, output.publish_voice(operation, &text)));
                Ok(self.receipt(operation))
            }
            Some("result") => {
                let operation = arguments["operation"].as_u64().ok_or("Missing operation")?;
                let (id, reply) = self
                    .publication
                    .as_ref()
                    .filter(|(id, _)| *id == operation)
                    .ok_or("Unknown publication")?;
                match reply.try_recv() {
                    Ok(result) => {
                        self.publication = None;
                        result.map(|_| Value::Null)
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => Ok(self.receipt(*id)),
                    Err(_) => {
                        self.publication = None;
                        Err("OSC sender disconnected".into())
                    }
                }
            }
            _ => Err("Unknown voice operation".into()),
        }
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        let result = (|| -> Result<(), String> {
            let id = self.id();
            let output = self.output.as_mut().ok_or("Output unavailable")?;
            match name {
                "host.services" => {
                    self.services = Some(serde_json::from_value(value).map_err(|e| e.to_string())?)
                }
                "system.metrics" => self
                    .metrics
                    .update(serde_json::from_value(value).map_err(|e| e.to_string())?),
                "osc.receiver" if self.game == OscGame::VRChat => {
                    output.receiver =
                        Some(serde_json::from_value(value).map_err(|e| e.to_string())?)
                }
                "translation.output" => output
                    .publisher()
                    .on_host_output(serde_json::from_value(value).map_err(|e| e.to_string())?),
                "voice.review" if self.game == OscGame::VRChat => {
                    output.set_microphone_review(value == true)
                }
                "lyrics.text" if value["output"] == id => output.publisher().set_lyrics(
                    serde_json::from_value(value["text"].clone()).map_err(|e| e.to_string())?,
                ),
                "plugin.state" if value["plugin"] == "lyrics" => {
                    self.lyrics_available = !value["state"].is_null();
                    if !self.lyrics_available {
                        output.publisher().set_lyrics(Default::default());
                    }
                }
                "udp.revoked" => {
                    output.disable_output()?;
                    context.event("config", json!(output.draft()));
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            Self::notice(context, error);
        }
    }
    fn reply(&mut self, _: u64, result: xrtranslate_plugin::Result, context: &mut Context) {
        if let Err(error) = result {
            Self::notice(context, error);
        }
    }
    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        let id = self.id();
        let output = self.output.as_mut()?;
        if let Some(result) = self.submit.as_ref().and_then(Request::poll) {
            self.submit = None;
            match result {
                Ok(_) => output.draft_input_mut().clear(),
                Err(error) => Self::notice(context, error),
            }
        }
        if let Some(error) = output.manager().take_error()
            && output.applied().enabled
        {
            Self::notice(context, error);
        }
        let settings = output.applied();
        let state = json!({
            "osc_receiver": (self.game == OscGame::VRChat).then(|| xrtranslate_client_core::osc::ReceiveOptions {enabled:settings.enabled, ip:settings.ip.clone(), send_port:settings.send_port, listen_port:settings.listen_port}),
            "voice_output": (self.game == OscGame::VRChat && settings.enabled).then_some("osc.voice"),
            "lyrics": settings.lyric_budget().map(|budget| json!({"output":id,"budget":budget,"offset_ms":settings.lyric_offset_ms()})),
        });
        if state != self.state {
            self.state = state.clone();
            context.event("state", state);
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn configure_ui(&mut self, ctx: &egui::Context, options: &Value) {
        ui::configure_surface(ctx, options);
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        let (Some(output), Some(services)) = (&mut self.output, &self.services) else {
            return;
        };
        let actions = output.render_page(
            ui,
            OscPageContext {
                language: services.language,
                preparing_text: services.text.preparing || self.submit.is_some(),
                microphone_sync_enabled: services.text.microphone_sync,
                lyrics_available: self.lyrics_available,
                languages: services.languages,
                source_lang: &services.source_language,
                target_lang: &services.target_language,
                additional_target_lang: services.text.additional_target.as_deref(),
                asr_only: services.text.asr_only,
            },
        );
        for action in actions {
            if let Err(error) = self.action(action, context) {
                Self::notice(context, error);
            }
        }
        ui::forward_notices(ui.ctx(), context);
    }
}
pub fn run(game: OscGame) {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let app = App {
        game,
        output: None,
        metrics: Default::default(),
        services: None,
        submit: None,
        publication: None,
        state: Value::Null,
        lyrics_available: false,
    };
    if let Err(error) = surface::serve(app) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
