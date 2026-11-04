//! Small headless tool loop. Model access and operation execution are supplied by the host.
use crate::{
    ask::Reply,
    director::{Tool, ToolCall, ToolProvider, ToolResult, arguments, catalog},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use xrtranslate_prompt::{PromptMessage, PromptMessageRole};
mod context;
mod environment;
#[cfg(test)]
mod online_tests;

const MAX_STEPS: usize = 64;
const MAX_ERRORS: usize = 3;
pub(crate) const MAX_GOAL_BYTES: usize = 4000;

#[derive(Default)]
pub(crate) struct AgentPlugin {
    available_tools: Vec<Tool>,
    supports_chat: bool,
    input_budget: usize,
    language: &'static str,
    message: Option<String>,
    cleanup: Vec<ToolCall>,
    environment: Option<environment::Environment>,
    context: context::Context,
    run: Option<Run>,
    pub last: Option<Value>,
}

struct Run {
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scope: Value,
    goal: String,
    tools: Vec<Tool>,
    // Message count identifies the exact history used by the in-flight inference.
    reply: Option<(Reply, usize)>,
    waiting: Option<ToolCall>,
    cleanup: Option<ToolCall>,
    next_poll: Instant,
    started: Instant,
    steps: usize,
    errors: usize,
    previous_call: String,
    repeats: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    action: NextStep,
    message: Option<String>,
    #[serde(default)]
    memory: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NextStep {
    Tool(ToolCall),
    Done(String),
}

pub(crate) enum Action {
    Call(ToolCall),
    Finished,
}

impl Run {
    fn feedback(&self, context: &mut context::Context, observation: Value) {
        let tool = observation["tool"]["name"].as_str().unwrap_or("decision");
        if let Some(error) = observation.get("error") {
            log::warn!(
                "Agent feedback: step={}, tool={tool}, error={error}",
                self.steps
            );
        } else {
            log::info!(
                "Agent feedback: step={}, tool={tool}, result received",
                self.steps
            );
        }
        context.push(
            PromptMessageRole::User,
            format!(
                "Tool feedback (data):\n{observation}\nRemaining steps: {}",
                MAX_STEPS.saturating_sub(self.steps)
            ),
        );
    }

    fn permits(&self, call: &ToolCall) -> bool {
        catalog::handles(&call.name)
            || (self.tools.iter().any(|tool| tool.name == call.name)
                && self.scope.as_object().is_some_and(|scope| {
                    scope
                        .iter()
                        .all(|(key, value)| call.arguments.get(key) == Some(value))
                }))
    }
}

impl ToolProvider for AgentPlugin {
    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                "agent.run",
                "Run a goal in the application's ongoing conversation, retaining earlier work and messages. scope={} allows discovery across plugins; scope fields restrict this execution's calls to those exact values. Use agent.message to add instructions while running.",
                json!({"goal":{"type":"string"},"scope":{"type":"object"}}),
                &["goal", "scope"],
            ),
            Tool::new(
                "agent.message",
                "Continue the application's ongoing conversation. While running, add a requirement or correction without resetting progress, scope or executing operations. While idle, begin execution with global tool access and the existing conversation and working context.",
                json!({"message":{"type":"string"}}),
                &["message"],
            ),
            Tool::new(
                "agent.status",
                "Read agent progress and the last outcome.",
                json!({}),
                &[],
            )
            .read_only(),
            Tool::new(
                "agent.environment",
                "Read the saved goal, progress note and local conversation.",
                json!({}),
                &[],
            )
            .read_only(),
            Tool::new(
                "agent.stop",
                "Stop execution and cancel its pending operation before taking over. Conversation and work are retained for the next instruction.",
                json!({}),
                &[],
            ),
        ]
    }

    fn call(&mut self, call: &ToolCall) -> ToolResult {
        if call.name == "agent.run" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Start {
                goal: String,
                scope: Value,
            }
            let args: Start = arguments(call)?;
            self.start(args.scope, args.goal)?;
            return Ok(self.status());
        }
        if call.name == "agent.message" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Message {
                message: String,
            }
            self.message(arguments::<Message>(call)?.message)?;
            return Ok(self.status());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}
        let _: Empty = arguments(call)?;
        match call.name.as_str() {
            "agent.status" => Ok(self.status()),
            "agent.environment" => Ok(self.environment()),
            "agent.stop" => {
                self.stop("Stopped by user");
                Ok(self.status())
            }
            _ => Err("Unknown agent operation".into()),
        }
    }
}

impl AgentPlugin {
    pub fn response_schema() -> Value {
        json!({"type":"object","properties":{
            "action":{"anyOf":[
                {"type":"object","properties":{"tool":{"type":"object","properties":{"name":{"type":"string"},"arguments":{"type":"object","additionalProperties":true}},"required":["name","arguments"],"additionalProperties":false}},"required":["tool"],"additionalProperties":false},
                {"type":"object","properties":{"done":{"type":"string"}},"required":["done"],"additionalProperties":false}
            ]},
            "message":{"type":"string"},
            "memory":{"type":"string"}
        },"required":["action","memory"],"additionalProperties":false})
    }

    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    pub fn load(directory: std::path::PathBuf) -> Self {
        let (environment, mut last) = environment::Environment::open(directory);
        if let Some(last) = &mut last {
            if last["running"] == true {
                last["summary"] =
                    json!("Interrupted by application restart; working documents are preserved");
                last["running"] = json!(false);
            }
            // Keep the last outcome for diagnostics; a new application process
            // starts with an empty conversation and never replays saved commands.
            if let Some(last) = last.as_object_mut() {
                last.remove("messages");
                last.remove("memory");
            }
        }
        Self {
            environment: Some(environment),
            last,
            ..Self::default()
        }
    }

    pub fn configure(
        &mut self,
        supports_chat: bool,
        input_budget: usize,
        tools: Vec<Tool>,
        language: &'static str,
    ) {
        self.supports_chat = supports_chat;
        self.input_budget = input_budget;
        self.language = language;
        if !supports_chat && self.active() {
            self.stop("The selected model does not support general chat");
        }
        if self.run.as_ref().is_some_and(|run| {
            run.tools
                .iter()
                .any(|needed| !tools.iter().any(|available| available.name == needed.name))
        }) {
            self.stop("An operation provider was disabled");
        }
        self.available_tools = tools;
    }

    pub fn allow_external(&self, call: &ToolCall, tools: &[Tool]) -> Result<(), String> {
        if self.active()
            && !self.tools().iter().any(|tool| tool.name == call.name)
            && tools
                .iter()
                .any(|tool| tool.name == call.name && !tool.read_only)
        {
            return Err("Agent is running. Call agent.stop before taking over.".into());
        }
        Ok(())
    }

    pub fn take_cleanup(&mut self) -> Vec<ToolCall> {
        std::mem::take(&mut self.cleanup)
    }
    pub fn environment(&self) -> Value {
        let mut snapshot = self.run.as_ref().map_or_else(
            || self.last.clone().unwrap_or(json!({"running":false})),
            |run| {
                json!({
                    "running": true, "scope": run.scope, "goal": run.goal,
                    "steps": run.steps, "pending": run.waiting,
                })
            },
        );
        snapshot["memory"] = json!(self.context.memory);
        snapshot["messages"] = json!(self.context.messages);
        snapshot
    }
    fn checkpoint(&self) {
        if let Some(environment) = &self.environment {
            environment.save(self.environment());
        }
    }
    pub fn active(&self) -> bool {
        self.run.is_some()
    }
    pub fn scope(&self) -> Option<&Value> {
        self.run.as_ref().map(|run| &run.scope)
    }
    pub fn cancellation(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.run.as_ref().map(|run| run.cancel.clone())
    }
    pub fn status(&self) -> Value {
        self.run.as_ref().map_or_else(
            || json!({"running": false, "last": self.last}),
            |run| {
                json!({
                    "running": true, "scope": run.scope, "steps": run.steps,
                    "limit": MAX_STEPS, "pending": run.waiting,
                    "elapsed_seconds": run.started.elapsed().as_secs(),
                })
            },
        )
    }
    pub fn start(&mut self, scope: Value, goal: String) -> Result<(), String> {
        if self.active() {
            return Err("An agent is already running; stop it before starting another".into());
        }
        if !self.supports_chat {
            return Err("The selected model does not support general chat".into());
        }
        if goal.trim().is_empty() || goal.len() > MAX_GOAL_BYTES {
            return Err(format!("Supply a goal of 1 to {MAX_GOAL_BYTES} bytes"));
        }
        let fields = scope.as_object().ok_or("Scope must be an object")?;
        let own_tools = self.tools();
        let tools: Vec<_> = self
            .available_tools
            .iter()
            .filter(|tool| {
                !own_tools.iter().any(|own| own.name == tool.name)
                    && fields
                        .keys()
                        .all(|key| tool.input_schema["properties"].get(key).is_some())
            })
            .cloned()
            .collect();
        if tools.is_empty() {
            return Err("No tools accept this scope".into());
        }
        self.context.begin(
            format!(
                "{}\nUI language: {}\nCurrent execution scope: {}\nAvailable namespaces (operation counts): {}\nDiscovery operations (shared with the application CLI): {}",
                include_str!("agent/instructions.md").trim(),
                self.language,
                scope,
                serde_json::to_string(&catalog::namespaces(&tools)).unwrap(),
                serde_json::to_string(&catalog::tools()).unwrap()
            ),
            format!("User instruction:\n{goal}\nRemaining steps: {MAX_STEPS}"),
            self.input_budget,
        )?;
        log::info!(
            "Agent execution started: goal_bytes={}, tools={}, input_budget={}, conversation_messages={}",
            goal.len(),
            tools.len(),
            self.input_budget,
            self.context.messages.len(),
        );
        self.last = None;
        self.message = None;
        self.run = Some(Run {
            cancel: Default::default(),
            scope,
            goal,
            tools,
            reply: None,
            waiting: None,
            cleanup: None,
            next_poll: Instant::now(),
            started: Instant::now(),
            steps: 0,
            errors: 0,
            previous_call: String::new(),
            repeats: 0,
        });
        self.checkpoint();
        Ok(())
    }

    pub fn message(&mut self, message: String) -> Result<(), String> {
        if !self.active() {
            return self.start(json!({}), message);
        }
        let message = message.trim();
        if message.is_empty() || message.len() > MAX_GOAL_BYTES {
            return Err(format!(
                "Supply an instruction of 1 to {MAX_GOAL_BYTES} bytes"
            ));
        }
        let run = self.run.as_mut().unwrap();
        self.context.push(
            PromptMessageRole::User,
            format!("User follow-up instruction:\n{message}"),
        );
        // A human correction starts a new attempt, without resetting the task budget.
        run.errors = 0;
        run.repeats = 0;
        run.previous_call.clear();
        log::info!(
            "Agent instruction received: step={}, bytes={}, model_pending={}, tool_pending={}",
            run.steps,
            message.len(),
            run.reply.is_some(),
            run.waiting.is_some()
        );
        self.checkpoint();
        Ok(())
    }

    pub fn stop(&mut self, reason: &str) -> Option<Value> {
        self.finish(reason, false)
    }

    fn finish(&mut self, reason: &str, completed: bool) -> Option<Value> {
        let run = self.run.take()?;
        run.cancel.store(true, std::sync::atomic::Ordering::Release);
        log::info!("Agent finished after {} steps: {reason}", run.steps);
        let result = json!({"running":false,"completed":completed,"scope":run.scope,"goal":run.goal,"steps":run.steps,"summary":reason,"cancel_requested":run.cleanup});
        self.context.push(
            PromptMessageRole::User,
            format!("Execution ended (data):\n{result}"),
        );
        self.cleanup.extend(run.cleanup);
        self.last = Some(result.clone());
        self.checkpoint();
        Some(result)
    }
    pub fn observe(&mut self, call: &ToolCall, result: ToolResult) {
        let Some(run) = &mut self.run else {
            return;
        };
        match result {
            Ok(value) => {
                if let Some(pending) = value.get("pending") {
                    let pending = serde_json::from_value::<ToolCall>(pending.clone());
                    let cancel = serde_json::from_value::<ToolCall>(value["cancel"].clone());
                    let (Ok(pending), Ok(cancel)) = (pending, cancel) else {
                        self.observe(
                            call,
                            Err("Operation returned an invalid wait/cancel receipt".into()),
                        );
                        return;
                    };
                    if !run.permits(&pending) || !run.permits(&cancel) {
                        self.observe(
                            call,
                            Err("Operation receipt is outside the fixed scope".into()),
                        );
                        return;
                    }
                    let started_waiting = run.waiting.is_none();
                    run.waiting = Some(pending);
                    run.cleanup = Some(cancel);
                    run.next_poll = Instant::now() + Duration::from_millis(250);
                    if started_waiting {
                        self.checkpoint();
                    }
                    return;
                }
                run.waiting = None;
                run.cleanup = None;
                if value.get("changed") == Some(&Value::Bool(false)) {
                    run.errors += 1;
                } else {
                    run.errors = 0;
                }
                run.feedback(&mut self.context, json!({"tool":call,"result":value}));
            }
            Err(error) => {
                run.waiting = None;
                self.cleanup.extend(run.cleanup.take());
                run.errors += 1;
                run.feedback(&mut self.context, json!({"tool":call,"error":error}));
            }
        }
        self.checkpoint();
    }

    pub fn poll(&mut self, mut request: impl FnMut(Vec<PromptMessage>) -> Reply) -> Option<Action> {
        let run = self.run.as_mut()?;
        let reason = if run.steps >= MAX_STEPS && run.reply.is_none() && run.waiting.is_none() {
            Some("Step budget reached")
        } else if run.errors >= MAX_ERRORS {
            Some("Stopped after repeated invalid or ineffective operations")
        } else if run.started.elapsed() > Duration::from_secs(1200) {
            Some("Time budget reached")
        } else {
            None
        };
        if let Some(reason) = reason {
            return self.stop(reason).map(|_| Action::Finished);
        }
        if let Some(call) = &run.waiting {
            return (Instant::now() >= run.next_poll).then(|| Action::Call(call.clone()));
        }
        if let Some((reply, message_count)) = &run.reply {
            let answer = match reply.try_recv() {
                Ok(answer) => answer,
                Err(crossbeam_channel::TryRecvError::Empty) => return None,
                Err(_) => Err("Model request was interrupted".into()),
            };
            let superseded = self.context.messages.len() != *message_count;
            run.reply = None;
            if superseded {
                // This reply has not seen the correction: neither its action nor its
                // progress note may consume or summarize the new user instructions.
                log::info!("Agent skipped an outdated decision after a user instruction");
                self.checkpoint();
                return None;
            }
            let answer = match answer {
                Ok(answer) => answer,
                Err(error) if error.invalid_output => {
                    run.errors += 1;
                    run.feedback(&mut self.context, json!({"error":error.error}));
                    return None;
                }
                Err(error) => return self.stop(&error.error).map(|_| Action::Finished),
            };
            self.context
                .push(PromptMessageRole::Assistant, answer.clone());
            let answer = answer.trim();
            let answer = answer
                .strip_prefix("```json")
                .or_else(|| answer.strip_prefix("```"))
                .and_then(|text| text.trim_end().strip_suffix("```"))
                .unwrap_or(answer)
                .trim();
            let decision = serde_json::from_str::<Decision>(answer);
            match decision {
                Ok(decision) => {
                    self.context.remember(decision.memory);
                    let message = decision.message.filter(|text| !text.trim().is_empty());
                    let call = match decision.action {
                        NextStep::Done(done) => {
                            self.message = Some(message.unwrap_or_else(|| done.clone()));
                            return self.finish(&done, true).map(|_| Action::Finished);
                        }
                        NextStep::Tool(call) => call,
                    };
                    let allowed = run.permits(&call);
                    if allowed {
                        self.message = message;
                        let signature = serde_json::to_string(&call).unwrap();
                        run.repeats = if signature == run.previous_call {
                            run.repeats + 1
                        } else {
                            0
                        };
                        run.previous_call = signature;
                        if run.repeats >= 3 {
                            return self
                                .stop("Stopped after repeated identical operations")
                                .map(|_| Action::Finished);
                        }
                        log::info!("Agent operation: step={}, tool={}", run.steps, call.name);
                        if catalog::handles(&call.name) {
                            let result = catalog::call(&run.tools, &call);
                            self.observe(&call, result);
                            return None;
                        }
                        return Some(Action::Call(call));
                    }
                    run.errors += 1;
                    run.feedback(
                        &mut self.context,
                        json!({"error":"Use tools.list / tools.describe to discover available operations; other calls must include the exact supplied scope"}),
                    );
                }
                Err(error) => {
                    run.errors += 1;
                    run.feedback(&mut self.context, json!({"error":format!("No operation was executed. Invalid decision: {error}. Return one JSON object with action containing only tool or done. Put memory and optional message alongside action, not inside it. Wait for successful tool feedback before reporting an operation as completed.")}));
                }
            }
            return None;
        }
        let messages = match self.context.prepare() {
            Ok(messages) => messages,
            Err(error) => return self.stop(&error).map(|_| Action::Finished),
        };
        run.steps += 1;
        run.reply = Some((request(messages), self.context.messages.len()));
        self.checkpoint();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_goals_use_plugin_operations_and_recover_rejected_model_output() {
        let mut agent = AgentPlugin::default();
        assert!(agent.start(json!({}), "Review".into()).is_err());
        agent.configure(
            true,
            8192,
            vec![
                Tool::new("alpha.inspect", "Inspect state", json!({}), &[]).read_only(),
                Tool::new("beta.update", "Update state", json!({}), &[]),
            ],
            "中文",
        );
        agent
            .start(json!({}), "Inspect and improve the task".into())
            .unwrap();
        assert_eq!(agent.run.as_ref().unwrap().tools.len(), 2);
        let (tx, rx) = crossbeam_channel::bounded(1);
        agent.poll(|_| rx.clone());
        tx.send(Err(xrtranslate_protocol::AskError {
            error: "Return one text decision".into(),
            invalid_output: true,
        }))
        .unwrap();
        assert!(agent.poll(|_| panic!()).is_none());
        assert!(agent.active());
        agent.poll(|messages| {
            assert_eq!(
                messages.len(),
                3,
                "a failed generation is not assistant history"
            );
            assert!(messages[2].content.contains("Return one text decision"));
            rx.clone()
        });
        tx.send(Ok(json!({"action":{"tool":{"name":"alpha.inspect","arguments":{}},"done":"Complete"},"memory":"Complete","message":"已经完成。"}).to_string())).unwrap();
        assert!(agent.poll(|_| panic!()).is_none());
        assert!(
            agent.take_message().is_none(),
            "invalid decisions must not speak"
        );
        assert!(agent.context.memory.is_empty());
        agent.poll(|messages| {
            assert!(
                messages
                    .last()
                    .unwrap()
                    .content
                    .contains("No operation was executed")
            );
            rx.clone()
        });
        let update = "正在检查状态。".repeat(40);
        tx.send(Ok(json!({"action":{"tool":{"name":"alpha.inspect","arguments":{}}},"memory":"Inspecting initial state","message":update}).to_string())).unwrap();
        assert!(
            matches!(agent.poll(|_| panic!()), Some(Action::Call(call)) if call.name == "alpha.inspect")
        );
        assert_eq!(agent.take_message().as_deref(), Some(update.as_str()));
        assert!(agent.take_message().is_none());
        let cancel = agent.cancellation().unwrap();
        agent.configure(false, 8192, Vec::new(), "中文");
        assert!(!agent.active());
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(agent.last.as_ref().unwrap()["completed"], false);
    }

    fn agent() -> AgentPlugin {
        let mut plugin = AgentPlugin::default();
        plugin.configure(
            true,
            16_000,
            ["test.read", "test.cancel"]
                .into_iter()
                .map(|name| {
                    Tool::new(
                        name,
                        "fixture operation",
                        json!({"document":{"type":"string"}}),
                        &["document"],
                    )
                })
                .collect(),
            "English",
        );
        plugin
            .start(json!({"document":"one"}), "Review this document".into())
            .unwrap();
        plugin
    }

    #[test]
    fn scope_feedback_cancellation_and_last_budgeted_reply_are_enforced() {
        let mut agent = agent();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut first_request = Vec::new();
        assert!(
            agent
                .poll(|messages| {
                    first_request = messages;
                    rx.clone()
                })
                .is_none()
        );
        tx.send(Ok(
            json!({"action":{"tool":{"name":"test.read","arguments":{"document":"another"}}}})
                .to_string(),
        ))
        .unwrap();
        assert!(agent.poll(|_| panic!("await reply first")).is_none());
        assert_eq!(agent.run.as_ref().unwrap().errors, 1);
        assert!(
            agent
                .poll(|messages| {
                    assert_eq!(&messages[..first_request.len()], first_request);
                    assert_eq!(messages.len(), first_request.len() + 2);
                    assert_eq!(messages[2].role, PromptMessageRole::Assistant);
                    assert!(messages[2].content.contains("another"));
                    assert!(
                        messages
                            .last()
                            .unwrap()
                            .content
                            .contains("exact supplied scope")
                    );
                    rx.clone()
                })
                .is_none()
        );
        tx.send(Ok(
            json!({"action":{"tool":{"name":"test.read","arguments":{"document":"one"}}}})
                .to_string(),
        ))
        .unwrap();
        let Some(Action::Call(call)) = agent.poll(|_| panic!()) else {
            panic!("valid call must run")
        };
        agent.observe(&call,Ok(json!({"pending":call,"cancel":{"name":"test.cancel","arguments":{"document":"one"}}})));
        agent.stop("Takeover");
        assert_eq!(agent.take_cleanup()[0].name, "test.cancel");
        assert!(
            agent
                .poll(|_| panic!("stopped agent must not request again"))
                .is_none()
        );

        let mut agent = self::agent();
        agent.run.as_mut().unwrap().steps = MAX_STEPS - 1;
        agent.poll(|_| rx.clone());
        let outcome = "Reviewed without dropping the rest of the result.\n".repeat(30);
        tx.send(Ok(json!({"action":{"done":outcome}}).to_string()))
            .unwrap();
        assert!(matches!(agent.poll(|_| panic!()), Some(Action::Finished)));
        assert_eq!(agent.last.as_ref().unwrap()["summary"], outcome);
        assert_eq!(agent.take_message().as_deref(), Some(outcome.as_str()));
    }

    #[test]
    fn conversation_outlives_execution_but_not_the_application() {
        let directory = tempfile::tempdir().unwrap();
        let mut agent = agent();
        agent.environment = Some(environment::Environment::open(directory.path().into()).0);
        let (tx, rx) = crossbeam_channel::bounded(1);
        agent.poll(|_| rx.clone());
        tx.send(Ok(
            json!({"action":{"tool":{"name":"test.read","arguments":{"document":"one"}}}})
                .to_string(),
        ))
        .unwrap();
        let Some(Action::Call(read)) = agent.poll(|_| panic!()) else {
            panic!("expected document read");
        };
        agent.observe(
            &read,
            Ok(json!({"project":"Neuralink","cue":"first-cue","text":"Hello, I how"})),
        );
        agent.poll(|_| rx.clone());
        tx.send(Ok(json!({"action":{"done":"The first cue needs review"},"memory":"Neuralink is document one; first-cue has an incomplete ending. No edits yet."}).to_string())).unwrap();
        let cancellation = agent.cancellation().unwrap();
        assert!(matches!(agent.poll(|_| panic!()), Some(Action::Finished)));
        assert!(!agent.active());
        assert!(cancellation.load(std::sync::atomic::Ordering::Acquire));
        let finished = agent.context.messages.clone();
        let memory = agent.context.memory.clone();
        assert_eq!(agent.environment()["messages"], json!(finished));

        // The same public message operation must continue even after completion.
        agent
            .call(&ToolCall {
                name: "agent.message".into(),
                arguments: json!({"message":"Delete that incomplete ending in the first cue"}),
            })
            .unwrap();
        assert_eq!(&agent.context.messages[1..finished.len()], &finished[1..]);
        assert_eq!(agent.context.messages.len(), finished.len() + 1);
        assert_eq!(agent.context.memory, memory);
        assert_eq!(agent.scope(), Some(&json!({})));
        assert_eq!(agent.run.as_ref().unwrap().steps, 0);
        assert!(!std::sync::Arc::ptr_eq(
            &cancellation,
            &agent.cancellation().unwrap()
        ));
        agent.poll(|messages| {
            assert_eq!(
                messages
                    .iter()
                    .filter(|m| m.role == PromptMessageRole::System)
                    .count(),
                1
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|m| m.content.contains("Hello, I how"))
                    .count(),
                1
            );
            assert!(
                messages
                    .last()
                    .unwrap()
                    .content
                    .contains("Delete that incomplete ending")
            );
            rx.clone()
        });
        let cancellation = agent.cancellation().unwrap();
        agent.stop("Stopped by user");
        assert!(cancellation.load(std::sync::atomic::Ordering::Acquire));
        let stopped = agent.context.messages.clone();
        assert!(stopped.last().unwrap().content.contains("Stopped by user"));
        assert_eq!(agent.last.as_ref().unwrap()["completed"], false);

        // Plugin-scoped work shares history too, but only its current scope grants access.
        agent
            .start(json!({"document":"two"}), "Review another project".into())
            .unwrap();
        assert_eq!(&agent.context.messages[1..stopped.len()], &stopped[1..]);
        assert_eq!(agent.run.as_ref().unwrap().steps, 0);
        assert!(!agent.run.as_ref().unwrap().permits(&read));
        agent.poll(|messages| {
            assert!(
                messages[0]
                    .content
                    .contains("Current execution scope: {\"document\":\"two\"}")
            );
            assert!(messages.iter().any(|m| m.content.contains("Neuralink")));
            assert!(
                messages
                    .iter()
                    .any(|m| m.content.contains("Stopped by user"))
            );
            rx.clone()
        });
        agent.stop("Test finished");
        drop(agent); // Flush diagnostics, just as application shutdown does.
        let saved: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("environment.json")).unwrap(),
        )
        .unwrap();
        assert!(saved["messages"].to_string().contains("Neuralink"));
        let restarted = AgentPlugin::load(directory.path().into());
        assert!(!restarted.active());
        assert!(restarted.context.messages.is_empty());
        assert!(restarted.context.memory.is_empty());
        assert_eq!(restarted.environment()["messages"], json!([]));
        assert_eq!(restarted.last.as_ref().unwrap()["summary"], "Test finished");
    }

    #[test]
    fn follow_ups_preserve_progress_and_skip_outdated_decisions() {
        let mut agent = agent();
        let prefix = agent.context.messages.clone();
        let cancellation = agent.cancellation().unwrap();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let call = json!({"name":"test.read","arguments":{"document":"one"}});
        agent.poll(|_| rx.clone());
        tx.send(Ok(
            json!({"action":{"tool":call},"memory":"Initial review in progress"}).to_string(),
        ))
        .unwrap();
        let Some(Action::Call(operation)) = agent.poll(|_| panic!()) else {
            panic!("expected read");
        };
        agent.observe(&operation, Ok(json!({"result":"First passage reviewed"})));
        let progress = agent.context.memory.clone();

        // Neither a stale tool call nor a stale completion may bypass a correction.
        for (index, action) in [
            json!({"tool":call}),
            json!({"done":"Finished without the update"}),
        ]
        .into_iter()
        .enumerate()
        {
            agent.poll(|_| rx.clone());
            let update = ToolCall {
                name: "agent.message".into(),
                arguments: json!({"message":format!("Correction {index}: keep the source text")}),
            };
            assert!(agent.allow_external(&update, &agent.tools()).is_ok());
            agent.call(&update).unwrap();
            let saved = agent.environment();
            assert!(
                saved["messages"]
                    .to_string()
                    .contains(&format!("Correction {index}"))
            );
            tx.send(Ok(
                json!({"action":action,"memory":"Outdated summary","message":"Already finished"})
                    .to_string(),
            ))
            .unwrap();
            assert!(agent.poll(|_| panic!()).is_none());
            assert!(agent.active());
            assert!(agent.take_message().is_none());
            assert_eq!(agent.context.memory, progress);
            assert_eq!(&agent.context.messages[..2], prefix);
            assert!(
                !agent
                    .context
                    .messages
                    .iter()
                    .any(|message| message.content.contains("Outdated summary"))
            );
        }
        let run = agent.run.as_ref().unwrap();
        assert_eq!(run.steps, 3, "follow-ups must not reset execution progress");
        assert_eq!(run.scope, json!({"document":"one"}));
        assert!(std::sync::Arc::ptr_eq(
            &cancellation,
            &agent.cancellation().unwrap()
        ));
        assert!(!cancellation.load(std::sync::atomic::Ordering::Acquire));
        assert!(!run.permits(&ToolCall {
            name: "test.read".into(),
            arguments: json!({"document":"two"})
        }));

        agent.poll(|messages| {
            for index in 0..2 {
                assert_eq!(
                    messages
                        .iter()
                        .filter(|message| message.content.contains(&format!("Correction {index}")))
                        .count(),
                    1
                );
            }
            assert!(
                messages
                    .iter()
                    .any(|message| message.content.contains("First passage reviewed"))
            );
            rx.clone()
        });
        tx.send(Ok(
            json!({"action":{"tool":call},"memory":"First passage reviewed; preserve source text"})
                .to_string(),
        ))
        .unwrap();
        let Some(Action::Call(operation)) = agent.poll(|_| panic!()) else {
            panic!("expected updated read");
        };
        agent.observe(&operation, Ok(json!({"pending":call,"cancel":{"name":"test.cancel","arguments":{"document":"one"}}})));
        agent.message("Keep timestamps too".into()).unwrap();
        assert!(agent.run.as_ref().unwrap().waiting.is_some());
        assert!(
            agent.take_cleanup().is_empty(),
            "a follow-up must not cancel active tools"
        );
        agent.run.as_mut().unwrap().next_poll = Instant::now();
        let Some(Action::Call(wait)) = agent.poll(|_| panic!("wait for the executing tool")) else {
            panic!("expected operation poll");
        };
        agent.observe(&wait, Ok(json!({"result":"Second passage reviewed"})));
        agent.poll(|messages| {
            let instruction = messages
                .iter()
                .position(|message| message.content.contains("Keep timestamps too"))
                .unwrap();
            assert!(
                messages[instruction + 1]
                    .content
                    .contains("Second passage reviewed")
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| message.content.contains("Keep timestamps too"))
                    .count(),
                1
            );
            rx.clone()
        });
        tx.send(Ok(
            json!({"action":{"done":"Reviewed with the corrections"}}).to_string(),
        ))
        .unwrap();
        assert!(matches!(agent.poll(|_| panic!()), Some(Action::Finished)));
        assert!(!agent.active());
    }

    #[test]
    fn discovery_uses_only_the_run_catalog_and_keeps_schemas_out_of_the_prefix() {
        let mut agent = agent();
        let initial = agent.context.messages.clone();
        assert!(initial[0].content.contains("tools.describe"));
        assert!(!initial[0].content.contains("fixture operation"));
        assert!(!initial[0].content.contains("test.read"));
        let (tx, rx) = crossbeam_channel::bounded(1);
        for arguments in [json!({"names":["test.read"]}), json!({"names":["ui.set"]})] {
            agent.poll(|_| rx.clone());
            tx.send(Ok(json!({"action":{"tool":{"name":"tools.describe","arguments":arguments}},"memory":"Inspecting available operations"}).to_string())).unwrap();
            assert!(
                agent
                    .poll(|_| panic!("already waiting for the model"))
                    .is_none()
            );
        }
        let run = agent.run.as_ref().unwrap();
        assert!(
            agent.context.messages[3]
                .content
                .contains("fixture operation")
        );
        assert!(agent.context.messages[3].content.contains("inputSchema"));
        assert!(
            agent.context.messages[5]
                .content
                .contains("Tool unavailable: ui.set")
        );
        assert_eq!(&agent.context.messages[..2], &initial);
        assert!(!run.permits(&ToolCall {
            name: "test.read".into(),
            arguments: json!({"document":"another"})
        }));
    }
}
