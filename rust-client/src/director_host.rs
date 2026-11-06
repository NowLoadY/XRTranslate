//! Application composition for Director providers. Business operations stay in plugins.
use crate::XRTranslateApp;
use ui_director::Response;
use ui_director::egui::driver::Command;
use ui_director::{Tool, ToolCall, ToolResult};

impl XRTranslateApp {
    pub(crate) fn agent_active(&self) -> bool {
        self.plugin_host.state("agent")["running"] == true
    }
    pub(crate) fn submit_agent_instruction(
        &mut self,
        instruction: String,
    ) -> Result<(), &'static str> {
        if !self.agent_active() {
            self.prepare_agent()?;
        }
        self.refresh_plugin_context();
        self.plugin_host
            .invoke(ToolCall {
                name: "agent.message".into(),
                arguments: serde_json::json!({"message":instruction}),
            })
            .map_err(|error| {
                log::warn!("Avatar Agent instruction failed: {error}");
                "Could not send the instruction. Please try again."
            })
    }
    pub(crate) fn prepare_agent(&mut self) -> Result<(), &'static str> {
        if !self.plugin_host.available("agent") {
            return Err("Enable Agent in Settings");
        }
        if !self.service_config.agent_available() {
            return Err("Please choose a model that supports Agent on the Welcome Page.");
        }
        if self.agent_active() {
            return Err("An agent is already running");
        }
        self.refresh_plugin_context();
        Ok(())
    }

    pub(crate) fn director_tools(&mut self) -> Vec<Tool> {
        crate::plugins::tool_providers(self)
            .into_iter()
            .flat_map(|provider| provider.tools())
            .collect()
    }

    pub(crate) fn call_director_tool(&mut self, call: &ToolCall) -> ToolResult {
        let result = ui_director::dispatch(&mut crate::plugins::tool_providers(self), call);
        if let Err(error) = &result {
            log::warn!("Director tool {} failed: {error}", call.name);
        }
        result
    }

    pub(crate) fn poll_director(&mut self) {
        let requests = ui_director::egui::driver().take_host_requests();
        if requests.is_empty() {
            return;
        }
        self.refresh_plugin_context();
        for request in requests {
            if request.expires <= std::time::Instant::now() {
                let _ = request
                    .responder
                    .send(Response::err("Request expired before execution"));
                continue;
            }
            let result = match request.command {
                Command::Call(call) => self.call_director_tool(&call),
                _ => Err("Unsupported operation".into()),
            };
            let response = match result {
                Ok(value) => Response::ok("Completed", Some(value)),
                Err(error) => Response::err(error),
            };
            let _ = request.responder.send(response);
        }
    }

    pub(crate) fn stop_agent(&mut self, _reason: &str) {
        let _ = self.plugin_host.invoke(ToolCall {
            name: "agent.stop".into(),
            arguments: serde_json::json!({}),
        });
    }
}
