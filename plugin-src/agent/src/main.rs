mod agent;
mod imagegen;
mod skills;

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

struct Operation {
    id: u64,
    call: ToolCall,
    image: Option<imagegen::Output>,
}

#[derive(Default)]
struct Agent {
    core: Option<AgentPlugin>,
    session: String,
    inference: Option<Inference>,
    operation: Option<Operation>,
    image_directory: std::path::PathBuf,
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
            self.image_directory = std::env::temp_dir()
                .join("XRTranslate")
                .join("generated_images");
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
        if name == "plugin.state"
            && let Some(notice) = value["state"].get("document_notice")
        {
            self.core.as_mut().unwrap().document_changed(notice.clone());
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
        core.image_generation = value["inference"]["image_generation"] == true;
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
            let _ = request.reply.send(inference_result(result));
        } else if self
            .operation
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            let operation = self.operation.take().unwrap();
            let core = self.core.as_mut().unwrap();
            if let Some(image) = operation.image {
                let saved = inference_result(result)
                    .map_err(|error| error.error)
                    .and_then(|png| image.save(&png, core.image_input && core.native_tools));
                match saved {
                    Ok((receipt, preview)) => {
                        core.observe_with_image(&operation.call, Ok(receipt), preview)
                    }
                    Err(error) => core.observe(&operation.call, Err(error)),
                }
            } else {
                core.observe(&operation.call, result);
            }
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
            if let Some(operation) = self.operation.take() {
                context.cancel(operation.id);
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
                    if call.name == imagegen::TOOL {
                        match imagegen::prepare(&call, &self.image_directory) {
                            Ok((request, output)) => {
                                let id = context.call(
                                    "inference.image",
                                    serde_json::to_value(request).unwrap(),
                                );
                                self.operation = Some(Operation {
                                    id,
                                    call,
                                    image: Some(output),
                                });
                            }
                            Err(error) => core.observe(&call, Err(error)),
                        }
                    } else {
                        let id = context.call(&call.name, call.arguments.clone());
                        self.operation = Some(Operation {
                            id,
                            call,
                            image: None,
                        });
                    }
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
        if status["operation"].is_null()
            && let Some(operation) = &self.operation
        {
            status["operation"] = json!(operation.call);
        }
        if status != self.status {
            context.event("state", status.clone());
            self.status = status;
        }
    }
}

fn inference_result(result: Result) -> std::result::Result<String, AskError> {
    let mut value = result.map_err(AskError::from)?;
    if let Some(error) = value.get_mut("error") {
        return Err(serde_json::from_value(error.take())
            .unwrap_or_else(|_| "Invalid inference error".into()));
    }
    match value["text"].take() {
        Value::String(text) => Ok(text),
        _ => Err("Inference returned no result".into()),
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(error) = xrtranslate_plugin::serve(Agent::default()) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
