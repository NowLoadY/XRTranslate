mod agent;

use agent::{Action, AgentPlugin};
use crossbeam_channel::Sender;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ui_director::{ToolCall, ToolProvider};
use xrtranslate_plugin::{Context, Environment, Plugin, Result};
use xrtranslate_protocol::{AskError, AskRequest, AskTarget};

struct Inference {
    id: u64,
    reply: Sender<std::result::Result<String, AskError>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Agent {
    core: Option<AgentPlugin>,
    session: String,
    inference: Option<Inference>,
    operation: Option<(u64, ToolCall)>,
    status: Value,
}

impl Plugin for Agent {
    fn initialize(
        &mut self,
        environment: Environment,
        _: &mut Context,
    ) -> std::result::Result<(), String> {
        // Environment refreshes never replace the application's conversation.
        if self.core.is_none() {
            self.session = uuid::Uuid::new_v4().to_string();
            self.core = Some(AgentPlugin::load(environment.data_directory));
        }
        Ok(())
    }

    fn call(&mut self, name: &str, arguments: Value, context: &mut Context) -> Result {
        let result = self.core.as_mut().unwrap().call(&ToolCall {
            name: name.into(),
            arguments,
        });
        self.publish(context);
        result
    }

    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        if name == "plugin.state" && value["plugin"] == "media" {
            self.core
                .as_mut()
                .unwrap()
                .document_changed(value["state"]["editing_notice"].clone());
            return;
        }
        if name != "host.context" {
            return;
        }
        let core = self.core.as_mut().unwrap();
        let native_tools = value["inference"]["native_tools"] == true;
        if core.native_tools != native_tools && core.active() {
            core.stop("The Agent model connection changed");
        }
        core.native_tools = native_tools;
        core.image_input = value["inference"]["image_input"] == true;
        core.configure(
            value["inference"]["agent"].as_bool().unwrap_or(false),
            value["inference"]["input_budget"].as_u64().unwrap_or(0) as usize,
            serde_json::from_value(value["tools"].clone()).unwrap_or_default(),
            value["language"].as_str().unwrap_or("English"),
        );
        if let Ok(location) = serde_json::from_value(value["location"].clone()) {
            core.set_location(location);
        }
    }

    fn reply(&mut self, id: u64, result: Result, context: &mut Context) {
        if self
            .inference
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            let request = self.inference.take().unwrap();
            let result = result.map_err(AskError::from).and_then(|value| {
                if let Some(error) = value.get("error") {
                    Err(serde_json::from_value(error.clone())
                        .unwrap_or_else(|_| "Invalid inference error".into()))
                } else {
                    value["text"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "Inference returned no text".into())
                }
            });
            let _ = request.reply.send(result);
        } else if self
            .operation
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
        {
            let (_, call) = self.operation.take().unwrap();
            self.core.as_mut().unwrap().observe(&call, result);
        }
        self.publish(context);
    }

    fn tick(
        &mut self,
        _: Duration,
        context: &mut Context,
    ) -> Option<Vec<xrtranslate_plugin::Widget>> {
        let core = self.core.as_mut().unwrap();
        if !core.active() {
            if let Some((id, _)) = self.operation.take() {
                context.cancel(id);
            }
        }
        if self
            .inference
            .as_ref()
            .is_some_and(|request| request.cancel.load(Ordering::Acquire))
        {
            context.cancel(self.inference.take().unwrap().id);
        }
        if self.operation.is_none() {
            let cancel = core.cancellation().unwrap_or_default();
            let mut pending = None;
            let action = core.poll(|messages| {
                let (reply, receiver) = crossbeam_channel::bounded(1);
                pending = Some((reply, messages));
                receiver
            });
            if let Some((reply, messages)) = pending {
                let request = match core.native_turn(&self.session) {
                    Some(agent_turn) => AskRequest::AgentTurn {
                        messages,
                        agent_turn,
                    },
                    None => AskRequest::Messages {
                        messages,
                        target: AskTarget::Agent,
                        response_schema: Some(AgentPlugin::response_schema()),
                    },
                };
                let id = context.call("inference.request", serde_json::to_value(request).unwrap());
                self.inference = Some(Inference { id, reply, cancel });
            }
            match action {
                Some(Action::Call(call)) => {
                    let id = context.call(&call.name, call.arguments.clone());
                    self.operation = Some((id, call));
                }
                Some(Action::Finished) | None => {}
            }
        }
        for call in core.take_cleanup() {
            context.call(&call.name, call.arguments);
        }
        self.publish(context);
        None
    }
}

impl Agent {
    fn publish(&mut self, context: &mut Context) {
        let core = self.core.as_mut().unwrap();
        if let Some(message) = core.take_message() {
            context.event("message", json!(message));
        }
        let mut status = core.status();
        if let Some((_, call)) = &self.operation {
            status["operation"] = json!(call);
        }
        if status != self.status {
            context.event("state", status.clone());
            self.status = status;
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(error) = xrtranslate_plugin::serve(Agent::default()) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
