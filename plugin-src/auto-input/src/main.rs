mod focused_input;
mod runtime;
use serde_json::{Value, json};
use std::time::Duration;
use xrtranslate_plugin::{Context, Environment, Plugin, Result, Text, Widget};

#[derive(Default)]
struct AutoInput {
    runtime: Option<runtime::AutoInputPlugin>,
    error: Option<String>,
    view: Vec<Widget>,
}
impl Plugin for AutoInput {
    fn initialize(
        &mut self,
        environment: Environment,
        _: &mut Context,
    ) -> std::result::Result<(), String> {
        if !cfg!(windows) {
            return Err("Automatic input is available on Windows.".into());
        }
        // Installing a plugin must not start typing into another application.
        self.runtime = Some(runtime::AutoInputPlugin::new(environment.host_executable));
        Ok(())
    }
    fn call(&mut self, name: &str, _: Value, _: &mut Context) -> Result {
        if name == "auto_input.status" {
            Ok(json!({"enabled":self.runtime.as_ref().unwrap().enabled()}))
        } else {
            Err("Unknown operation".into())
        }
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        let runtime = self.runtime.as_mut().unwrap();
        if name == "ui" && value["id"] == "active" {
            runtime.set_enabled(value["value"] == true);
            self.error = None;
        } else if let Err(error) = runtime.publisher().event(name, value) {
            eprintln!("Invalid translation event: {error}");
        }
    }
    fn tick(&mut self, _: Duration, context: &mut Context) -> Option<Vec<Widget>> {
        let runtime = self.runtime.as_ref().unwrap();
        if let Some(error) = runtime.take_error() {
            eprintln!("Automatic input: {error}");
            self.error = Some(error);
        }
        let active = runtime.enabled();
        let mut view = vec![Widget::Toggle {
            id: "active".into(),
            label: Text(
                [
                    ("en".into(), "Insert translations".into()),
                    ("zh".into(), "输入译文".into()),
                ]
                .into(),
            ),
            value: active,
            enabled: true,
        }];
        if let Some(error) = &self.error {
            view.push(Widget::Error {
                message: error.clone(),
            });
        }
        if view == self.view {
            None
        } else {
            context.event("config", json!({"active":active}));
            self.view = view.clone();
            Some(view)
        }
    }
}
fn main() {
    if let Err(error) = xrtranslate_plugin::serve(AutoInput::default()) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
