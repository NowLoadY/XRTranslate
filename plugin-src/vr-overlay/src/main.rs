mod overlay;
use eframe::egui;
use overlay::{VrOverlayPageContext, VrOverlayPlugin, VrOverlaySettings, VrOverlayUiAction};
use serde_json::{Value, json};
use std::time::Duration;
pub use xrtranslate_client_core::{
    compact_speaker_label, i18n, recognition as client_settings, session_coordinator, ui,
    voice_interaction,
};
use xrtranslate_client_core::{
    companion::{Command, SurfaceStatus, VoiceMode},
    session_coordinator::{HostOutputEvent, HostOutputSubscriber},
};
use xrtranslate_plugin::{Context, Environment, Plugin, surface};

#[derive(Default)]
struct App {
    overlay: Option<VrOverlayPlugin>,
    language: i18n::UiLanguage,
    state: Value,
}
impl App {
    fn settings_changed(&mut self, context: &mut Context) {
        let overlay = self.overlay.as_mut().unwrap();
        overlay.sync_settings();
        context.event("config", json!(overlay.draft()));
    }
    fn command(&mut self, command: Command, context: &mut Context) {
        let overlay = self.overlay.as_mut().unwrap();
        match command {
            Command::ShowSubtitles | Command::HideSubtitles => {
                overlay.draft_mut().captions_enabled = command == Command::ShowSubtitles;
                self.settings_changed(context);
            }
            Command::ComeHere => overlay.manager().visit_avatar(),
            Command::ReturnHome => overlay.manager().return_avatar(),
        }
    }
}
impl Plugin for App {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> Result<(), String> {
        self.language =
            i18n::UiLanguage::from_locale_str(&environment.language).unwrap_or_default();
        let config = if environment.config.is_null() {
            environment
                .data_directory
                .ancestors()
                .nth(3)
                .and_then(|runtime| std::fs::read(runtime.join("rust-client-settings.json")).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|settings| settings.get("vr_overlay_settings").cloned())
                .unwrap_or(Value::Null)
        } else {
            environment.config
        };
        let settings: VrOverlaySettings = serde_json::from_value(config).unwrap_or_default();
        if let Some(overlay) = &mut self.overlay {
            *overlay.draft_mut() = settings;
            overlay.sync_settings();
        } else {
            self.overlay = Some(VrOverlayPlugin::new(
                settings,
                environment.data_directory.join("input"),
            ));
        }
        context.event("config", json!(self.overlay.as_ref().unwrap().draft()));
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Use the overlay page's UI controls".into())
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        if self.overlay.is_none() {
            return;
        }
        let result = (|| -> Result<(), serde_json::Error> {
            match name {
                "translation.output" => {
                    let event: HostOutputEvent<'static> = serde_json::from_value(value)?;
                    self.overlay
                        .as_ref()
                        .unwrap()
                        .handle()
                        .on_host_output(event);
                }
                "companion.presentation" => self
                    .overlay
                    .as_ref()
                    .unwrap()
                    .manager()
                    .present_companion(serde_json::from_value(value)?),
                "companion.command" => self.command(serde_json::from_value(value)?, context),
                "voice.state" => self
                    .overlay
                    .as_ref()
                    .unwrap()
                    .manager()
                    .set_voice_snapshot(serde_json::from_value(value)?),
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("Overlay event {name}: {error}");
        }
    }
    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        let overlay = self.overlay.as_ref()?;
        let status = overlay.manager().status();
        let settings = overlay.draft();
        let requested = settings.enabled && settings.interaction_enabled;
        let state = json!({
            "companion": SurfaceStatus {
                available: status.avatar_available, present: status.avatar_present,
                attention: status.avatar_attention, greeting: status.avatar_greeting,
            },
            "voice": VoiceMode {
                requested, active: requested && status.steamvr_connected && status.interaction_available,
                review: requested && settings.review_before_send,
            },
        });
        if state != self.state {
            self.state = state.clone();
            context.event("state", state);
        }
        for action in overlay.manager().take_voice_actions() {
            context.event("voice.action", json!(action));
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn configure_ui(&mut self, context: &egui::Context, options: &Value) {
        ui::configure_surface(context, options);
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let status = overlay.manager().status();
        let actions = overlay::ui::render(
            overlay.draft_mut(),
            ui,
            VrOverlayPageContext {
                language: self.language,
                status: &status,
            },
        );
        for action in actions {
            match action {
                VrOverlayUiAction::SettingsChanged => self.settings_changed(context),
                VrOverlayUiAction::ClearSubtitles => {
                    self.overlay.as_ref().unwrap().handle().clear()
                }
                VrOverlayUiAction::ConnectSteamVr => self.overlay.as_ref().unwrap().connect(),
                VrOverlayUiAction::DisconnectSteamVr => self.overlay.as_ref().unwrap().disconnect(),
                VrOverlayUiAction::VisitAvatar => self.command(Command::ComeHere, context),
            }
        }
        ui::forward_notices(ui.ctx(), context);
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    #[cfg(debug_assertions)]
    if let Some(directory) = std::env::args()
        .skip_while(|arg| arg != "--vr-avatar-render")
        .nth(1)
    {
        if let Err(error) = overlay::render_avatar_preview(std::path::Path::new(&directory)) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = xrtranslate_client_core::ui::surface::serve(App::default()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
