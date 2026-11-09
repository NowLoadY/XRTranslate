mod lyrics;
mod strings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::mpsc, time::Duration};
use xrtranslate_plugin::{Context, Environment, Plugin, Result, Widget};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct LyricsText {
    title: String,
    lyric: String,
}
#[derive(Default)]
struct Lyrics {
    engine: Option<lyrics::LyricsPlugin>,
    output: Option<mpsc::Receiver<(String, LyricsText)>>,
    view: Vec<Widget>,
    requests: std::collections::BTreeMap<String, serde_json::Value>,
}
impl Plugin for Lyrics {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> std::result::Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.engine = Some(lyrics::LyricsPlugin::new(
            environment.data_directory.join("cache"),
            move |id, text| {
                let _ = tx.send((id, text));
            },
        ));
        self.output = Some(rx);
        context.event("state", json!({"available":true}));
        Ok(())
    }
    fn call(&mut self, name: &str, _: Value, _: &mut Context) -> Result {
        match name {
            "lyrics.status" => {
                Ok(json!({"status":self.engine.as_ref().unwrap().status().message_key()}))
            }
            _ => Err("Unknown operation".into()),
        }
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        let engine = self.engine.as_mut().unwrap();
        match name {
            "plugin.state" => {
                if let Some(plugin) = value["plugin"].as_str() {
                    if value["state"]["lyrics"].is_object() {
                        self.requests
                            .insert(plugin.into(), value["state"]["lyrics"].clone());
                    } else {
                        self.requests.remove(plugin);
                    }
                    if let Ok(requests) =
                        serde_json::from_value(json!(self.requests.values().collect::<Vec<_>>()))
                    {
                        engine.configure(requests);
                    }
                }
            }
            "ui" if value["id"] == "clear_cache" => engine.clear_cache(),
            _ => {}
        }
    }
    fn tick(&mut self, _: Duration, context: &mut Context) -> Option<Vec<Widget>> {
        for (output, text) in self.output.as_ref().unwrap().try_iter() {
            context.event("lyrics.text", json!({"output":output,"text":text}));
        }
        let engine = self.engine.as_ref().unwrap();
        let view = vec![
            Widget::Label {
                text: strings::text(engine.status().message_key()),
            },
            Widget::Button {
                id: "clear_cache".into(),
                label: strings::text("Clear lyrics cache"),
                enabled: !engine.cache_is_clearing(),
                primary: false,
            },
        ];
        if self.view == view {
            None
        } else {
            self.view = view.clone();
            Some(view)
        }
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

fn run() -> std::result::Result<(), String> {
    // WinRT factories are cached for the process lifetime. Keep the MTA alive
    // across lyric worker restarts so its cached interfaces cannot be unloaded.
    #[cfg(windows)]
    let _apartment = lyrics::Apartment::new()?;
    xrtranslate_plugin::serve(Lyrics::default())
}
