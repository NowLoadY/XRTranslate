mod archive;
mod page;
use eframe::egui;
use serde_json::Value;
use std::time::Duration;
use xrtranslate_client_core::services::{Capabilities, Reply};
pub use xrtranslate_client_core::{i18n, language_label, ui};
use xrtranslate_plugin::{Client, Context, Environment, Plugin, surface};

struct Inference {
    client: Client,
    requests: xrtranslate_client_core::services::Inference,
}
impl Inference {
    fn request(&mut self, prompt: String, _: &egui::Context) -> Reply {
        self.requests.request(&self.client, prompt)
    }
}
#[derive(Default)]
struct App {
    page: page::Page,
    archive: Option<archive::Archive>,
    inference: Option<Inference>,
    capabilities: Option<Capabilities>,
    ui: egui::Context,
    hint: String,
}
impl Plugin for App {
    fn initialize(&mut self, _: Environment, context: &mut Context) -> Result<(), String> {
        #[cfg(target_os = "android")]
        xrtranslate_client_core::file_dialog::install_client(context.client());
        if self.archive.is_none() {
            self.archive = Some(archive::Archive::new(context.client()));
            self.inference = Some(Inference {
                client: context.client(),
                requests: Default::default(),
            });
        }
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Use the vocabulary page's UI controls".into())
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        match name {
            "host.services" => self.capabilities = serde_json::from_value(value).ok(),
            "bookmarks.revision" => {
                if let Some(archive) = &mut self.archive {
                    archive.revision = value.as_u64().unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        if let (Some(archive), Some(inference), Some(cap)) =
            (&self.archive, &mut self.inference, &self.capabilities)
        {
            archive.poll();
            inference.requests.poll();
            let notices = ui::notifications::Notifications::default();
            self.page
                .poll_examples(archive, &notices, cap.language, &self.ui);
            for (message, details) in notices.take_pending() {
                context.event(
                    "notice",
                    serde_json::json!({"message":message,"details":details}),
                );
            }
            let hint = i18n::tr(cap.language, self.page.hint());
            if hint != self.hint {
                self.hint = hint.into();
                context.event("state", serde_json::json!({"hint":hint}));
            }
        }
        None
    }
}
impl surface::UiPlugin for App {
    fn configure_ui(&mut self, ctx: &egui::Context, options: &Value) {
        ui::configure_surface(ctx, options);
        self.ui = ctx.clone();
    }
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context) {
        if let (Some(archive), Some(inference), Some(cap)) =
            (&self.archive, &mut self.inference, &self.capabilities)
        {
            self.page.show(
                ui,
                archive,
                cap.language,
                cap.languages.for_text(),
                inference,
            );
        } else {
            ui.spinner();
        }
        ui::forward_notices(ui.ctx(), context);
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();
    if let Err(error) = xrtranslate_client_core::ui::surface::serve(App::default()) {
        eprintln!("Vocabulary: {error}");
        std::process::exit(1);
    }
}
