//! Application composition for Director providers. Business operations stay in plugins.
use crate::{XRTranslateApp, plugins::PluginId};
use ui_director::Response;
use ui_director::egui::driver::Command;
use ui_director::{Tool, ToolCall, ToolResult};

impl XRTranslateApp {
    pub(crate) fn submit_agent_instruction(
        &mut self,
        instruction: String,
    ) -> Result<(), &'static str> {
        if !self.agent.active() {
            self.prepare_agent()?;
        }
        self.agent.message(instruction).map_err(|error| {
            log::warn!("Avatar Agent instruction failed: {error}");
            "Could not send the instruction. Please try again."
        })
    }

    /// UI goal entry and plugin-scoped tasks share model/tool preparation.
    pub(crate) fn prepare_agent(&mut self) -> Result<(), &'static str> {
        if !self.plugin_enabled(PluginId::AGENT) {
            return Err("Enable Agent in Settings");
        }
        if !self.service_config.agent_available() {
            return Err("Please choose a model that supports Agent on the Welcome Page.");
        }
        if self.agent.active() {
            return Err("An agent is already running");
        }
        let tools = self.director_tools();
        self.agent.configure(
            self.service_config.agent_available(),
            self.service_config.agent_input_budget(),
            tools,
            self.ui_language.display_name(),
        );
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
        let tools = self.director_tools();
        self.agent.configure(
            self.service_config.agent_available(),
            self.service_config.agent_input_budget(),
            tools.clone(),
            self.ui_language.display_name(),
        );
        for request in requests {
            if request.expires <= std::time::Instant::now() {
                let _ = request
                    .responder
                    .send(Response::err("Request expired before execution"));
                continue;
            }
            let result = match request.command {
                Command::Call(call) => self
                    .agent
                    .allow_external(&call, &tools)
                    .and_then(|()| self.call_director_tool(&call)),
                _ => Err("Unsupported operation".into()),
            };
            let response = match result {
                Ok(value) => Response::ok("Completed", Some(value)),
                Err(error) => Response::err(error),
            };
            let _ = request.responder.send(response);
        }
    }

    pub(crate) fn stop_agent(&mut self, reason: &str) {
        self.agent.stop(reason);
        self.cleanup_agent_operations();
    }

    fn cleanup_agent_operations(&mut self) {
        for call in self.agent.take_cleanup() {
            let _ = self.call_director_tool(&call);
        }
    }

    pub(crate) fn poll_agent(&mut self, ctx: &eframe::egui::Context) {
        if !self.plugin_enabled(PluginId::AGENT) || !self.service_config.agent_available() {
            self.stop_agent("Required capability is unavailable");
            return;
        }
        if self.agent.active() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let cancel = self.agent.cancellation();
        match self.agent.poll(|messages| {
            self.ask.request_with_cancel(
                xrtranslate_protocol::AskRequest::Messages {
                    messages,
                    target: xrtranslate_protocol::AskTarget::Agent,
                    response_schema: Some(crate::plugins::agent::AgentPlugin::response_schema()),
                },
                cancel.clone(),
                ctx,
            )
        }) {
            Some(crate::plugins::agent::Action::Call(call)) => {
                let result = self.call_director_tool(&call);
                self.agent.observe(&call, result);
            }
            Some(crate::plugins::agent::Action::Finished) | None => {}
        }
        if let Some(message) = self.agent.take_message() {
            self.companion_inbox
                .post_text(PluginId::AGENT.as_str(), message);
        }
        self.cleanup_agent_operations();
    }
}
