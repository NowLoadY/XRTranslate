mod translation;
use screen_text::CaptureEvent;
use serde_json::{Value, json};
use session_coordinator::{TranslationEvent, TranslationSessionOwner, TranslationTask};
use xrtranslate_client_core::services::{Capabilities, select_text_languages};
pub use xrtranslate_client_core::{screen_text, session_coordinator};
use xrtranslate_plugin::{Context, Environment, Plugin};

#[derive(Default)]
struct App {
    translation: translation::OcrPlugin,
    services: Option<Capabilities>,
    owner: Option<TranslationSessionOwner>,
    start: Option<u64>,
    revision: u64,
    published: Value,
}
impl App {
    fn stop(&mut self, context: &mut Context) {
        if let Some(owner) = self.owner.take() {
            context.call("translation.stop", json!(owner));
        }
        self.start = None;
    }
    fn publish(&mut self, context: &mut Context) {
        let state = json!({"screen_text":{"revision":self.revision,"view":self.translation.state}});
        if state != self.published {
            self.published = state.clone();
            context.event("state", state);
        }
    }
    fn recognized(&mut self, text: String, context: &mut Context) -> Result<(), String> {
        let binding = self.translation.recognized(text.clone());
        let Some(binding) = binding else {
            if self.translation.state.source.is_empty() {
                self.stop(context);
            }
            return Ok(());
        };
        self.stop(context);
        let services = self
            .services
            .as_ref()
            .ok_or("Translation service is not ready")?;
        let languages = select_text_languages(
            &text,
            &services.source_language,
            &services.target_language,
            services.languages,
            services.text.additional_target.as_deref(),
            services.text.asr_only,
        )?;
        self.owner = Some(TranslationSessionOwner::Plugin(binding.owner.clone()));
        self.start = Some(context.call(
            "translation.start",
            json!(TranslationTask::text(text, languages, Some(binding))),
        ));
        Ok(())
    }
}
impl Plugin for App {
    fn initialize(&mut self, _: Environment, context: &mut Context) -> Result<(), String> {
        self.publish(context);
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Unknown screen operation".into())
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        let result = (|| -> Result<(), String> {
            match name {
                "host.services" => {
                    self.services = Some(serde_json::from_value(value).map_err(|e| e.to_string())?)
                }
                "screen.clear" => {
                    self.revision = value.as_u64().ok_or("Invalid screen revision")?;
                    self.stop(context);
                    self.translation.clear();
                }
                "screen.text" => {
                    let event: CaptureEvent =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    if event.revision() < self.revision {
                        return Ok(());
                    }
                    self.revision = event.revision();
                    match event {
                        CaptureEvent::Busy(_) => self.translation.set_recognizing(true),
                        CaptureEvent::Idle(_) => self.translation.set_recognizing(false),
                        CaptureEvent::Text(_, text) => self.recognized(text, context)?,
                        CaptureEvent::Failed { error, fatal, .. } => {
                            if fatal {
                                self.stop(context);
                                self.translation.failed(error);
                            } else {
                                self.translation.capture_failed(error);
                            }
                        }
                    }
                }
                "translation.event" => {
                    let owner: TranslationSessionOwner =
                        serde_json::from_value(value["owner"].clone())
                            .map_err(|e| e.to_string())?;
                    let event: TranslationEvent = serde_json::from_value(value["event"].clone())
                        .map_err(|e| e.to_string())?;
                    if let TranslationEvent::Finished { outcome, .. } = event {
                        if let Some(operation) = owner.operation_id() {
                            self.translation.finished(operation, outcome);
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.translation.failed(error);
        }
        self.publish(context);
    }
    fn reply(&mut self, id: u64, result: xrtranslate_plugin::Result, context: &mut Context) {
        if self.start == Some(id) {
            self.start = None;
            if let Err(error) = result {
                self.translation.failed(error);
            }
            self.publish(context);
        }
    }
}
fn main() {
    if let Err(error) = xrtranslate_plugin::serve(App::default()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
