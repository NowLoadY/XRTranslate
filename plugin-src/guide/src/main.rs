use serde::Deserialize;
use serde_json::{Value, json};
use xrt_guide::{Narrator, Planner};
use xrtranslate_plugin::{Context, Environment, Plugin, Result, guide::State};

#[derive(Default)]
struct Guide {
    planner: Planner,
    published: Option<State>,
    narrator: Narrator,
}

impl Guide {
    fn publish(&mut self, context: &mut Context) {
        self.narrator.sync(&self.planner.state);
        if self.published.as_ref() != Some(&self.planner.state) {
            context.event("state", json!(self.planner.state));
            self.published = Some(self.planner.state.clone());
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    page: Option<String>,
    topic: Option<String>,
}

impl Plugin for Guide {
    fn tick(
        &mut self,
        _: std::time::Duration,
        _: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        self.narrator.sync(&self.planner.state);
        None
    }

    fn initialize(
        &mut self,
        _: Environment,
        context: &mut Context,
    ) -> std::result::Result<(), String> {
        self.publish(context);
        Ok(())
    }

    fn call(&mut self, name: &str, arguments: Value, context: &mut Context) -> Result {
        if matches!(name, "guide.start" | "guide.select") {
            let selection: Selection =
                serde_json::from_value(arguments).map_err(|error| error.to_string())?;
            if name == "guide.start" {
                if selection.topic.is_some() {
                    return Err("Use guide.select to select a topic".into());
                }
                self.planner.start(selection.page.as_deref())?;
            } else {
                self.planner.select(
                    selection.page.as_deref().ok_or("Supply a page")?,
                    selection.topic.as_deref(),
                )?;
            }
        } else {
            if arguments
                .as_object()
                .is_none_or(|object| !object.is_empty())
            {
                return Err("This operation takes no arguments".into());
            }
            match name {
                "guide.catalog" => return Ok(json!({"chapters": self.planner.catalog()?})),
                "guide.status" => return Ok(json!(self.planner.state)),
                "guide.next" => self.planner.advance(true)?,
                "guide.previous" => self.planner.advance(false)?,
                "guide.stop" => self.planner.stop(),
                _ => return Err("Unknown guide operation".into()),
            }
        }
        self.publish(context);
        Ok(json!(self.planner.state))
    }

    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        match name {
            "guide.context" => match serde_json::from_value(value) {
                Ok(context) => self.planner.update(context),
                Err(_) => self.planner.stop(),
            },
            "guide.presented" => {
                if let Some(revision) = value["revision"].as_u64() {
                    self.planner.presented(revision);
                }
            }
            _ => return,
        }
        self.publish(context);
    }
}

fn main() {
    if let Err(error) = xrtranslate_plugin::serve(Guide::default()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
