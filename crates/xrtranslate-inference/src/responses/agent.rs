//! Map the scoped Director inventory and its local transcript to native Responses items.
use crate::{ChatCompletion, InferenceError, TokenUsage};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xrtranslate_prompt::{PromptMessage, PromptMessageRole};
use xrtranslate_protocol::AgentTurn;

pub(super) struct Catalogue {
    pub definitions: Vec<Value>,
    local_discovery: bool,
    // A wire name always maps back to exactly one declared local operation.
    names: BTreeMap<String, String>,
}

fn identifier(name: &str) -> String {
    name.chars()
        .take(54)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Catalogue {
    pub fn for_api(turn: &AgentTurn, api: xrtranslate_assets::remote::AgentApi) -> Self {
        let mut tools: Vec<_> = turn.tools.iter().collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let hosted_search = api == xrtranslate_assets::remote::AgentApi::OpenaiResponses;
        let deferred = hosted_search && tools.len() > 16;
        let local_discovery = !hosted_search && tools.len() > 64;
        let mut names = BTreeMap::new();
        let mut groups = BTreeMap::<String, Vec<Value>>::new();
        let mut definitions = Vec::new();
        for (index, tool) in tools.into_iter().enumerate() {
            let base = identifier(&tool.name);
            let mut name = base.clone();
            let mut suffix = index;
            while name.is_empty() || names.contains_key(&name) {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            names.insert(name.clone(), tool.name.clone());
            // Director schemas can include optional fields and open plugin data.
            // Preserve their declared contracts. The Director validates calls locally.
            let mut function = json!({
                "type": "function",
                "name": name,
                "description": tool.description,
                "strict": false,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "arguments": tool.input_schema,
                        "memory": {
                            "type": "string",
                            "description": "Brief cumulative progress note for local context compaction."
                        }
                    },
                    "required": ["arguments", "memory"],
                    "additionalProperties": false
                }
            });
            if deferred {
                let namespace = tool.name.split('.').next().unwrap_or("app");
                function["defer_loading"] = json!(true);
                groups
                    .entry(format!("xr_{}", identifier(namespace)))
                    .or_default()
                    .push(function);
            } else {
                definitions.push(function);
            }
        }
        for (name, functions) in groups {
            let overview = functions
                .iter()
                .filter_map(|f| f["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            definitions.push(json!({
                "type": "namespace", "name": name,
                "description": format!("Application operations: {overview}"),
                "tools": functions
            }));
        }
        if deferred {
            definitions.push(json!({"type":"tool_search"}));
        }
        Self {
            definitions,
            names,
            local_discovery,
        }
    }

    pub fn wire_tools(&self) -> Value {
        if !self.local_discovery {
            return json!(self.definitions);
        }
        // Keep large catalogues discoverable using the plugin's bounded local catalogue.
        // Store the full definitions for exact continuation checks even in this mode.
        let names: std::collections::BTreeSet<_> = self
            .names
            .values()
            .map(String::as_str)
            .chain(["tools.list", "tools.describe"])
            .collect();
        json!([{
            "type": "function", "name": "director_operation",
            "description": "Call one application operation. Start with tools.list (optional namespace or query) or tools.describe (names array) to read schemas, then call the matching name with its declared arguments.",
            "parameters": {
                "type":"object", "properties":{
                    "name":{"type":"string","enum":names},
                    "arguments":{"type":"object","additionalProperties":true},
                    "memory":{"type":"string","description":"Brief cumulative progress note"}
                }, "required":["name","arguments","memory"], "additionalProperties":false
            }
        }])
    }

    pub fn decision(
        &self,
        response: &Value,
    ) -> Result<(ChatCompletion, Option<String>), InferenceError> {
        let invalid = |reason| InferenceError::RejectedOutput {
            operation: "Agent",
            reason,
        };
        let mut action = None;
        let mut call_id = None;
        let mut memory = String::new();
        let mut commentary = String::new();
        let mut final_text = String::new();
        for item in response["output"]
            .as_array()
            .ok_or_else(|| invalid("Missing response output"))?
        {
            match item["type"].as_str() {
                Some("function_call") => {
                    if action.is_some() {
                        return Err(invalid("Expected a single local operation"));
                    }
                    let wire = item["name"].as_str().unwrap_or("");
                    let parameters: Value =
                        serde_json::from_str(item["arguments"].as_str().unwrap_or(""))
                            .map_err(|_| invalid("Invalid function arguments"))?;
                    let name = if self.local_discovery {
                        parameters["name"].as_str().filter(|name| {
                            wire == "director_operation"
                                && (matches!(*name, "tools.list" | "tools.describe")
                                    || self.names.values().any(|local| local.as_str() == *name))
                        })
                    } else {
                        self.names.get(wire).map(String::as_str)
                    }
                    .ok_or_else(|| invalid("Unknown Agent function"))?;
                    if !parameters["arguments"].is_object() {
                        return Err(invalid("Expected operation arguments"));
                    }
                    let id = item["call_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| invalid("Missing function call ID"))?;
                    call_id = Some(id.to_owned());
                    memory = parameters["memory"].as_str().unwrap_or("").into();
                    action =
                        Some(json!({"tool":{"name":name,"arguments":parameters["arguments"]}}));
                }
                Some("message") if item["role"] == "assistant" => {
                    let text = if item["phase"] == "commentary" {
                        &mut commentary
                    } else {
                        &mut final_text
                    };
                    for part in item["content"].as_array().into_iter().flatten() {
                        if let Some(value) =
                            part["text"].as_str().or_else(|| part["refusal"].as_str())
                        {
                            text.push_str(value);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut decision = if let Some(action) = action {
            json!({"action":action,"memory":memory})
        } else if !final_text.trim().is_empty() {
            json!({"action":{"done":final_text},"memory":final_text})
        } else {
            return Err(invalid("No function call or final answer"));
        };
        if call_id.is_some() && !commentary.trim().is_empty() {
            decision["message"] = json!(commentary);
        }
        Ok((
            ChatCompletion {
                text: decision.to_string(),
                usage: TokenUsage::from_json(response.get("usage")),
            },
            call_id,
        ))
    }
}

pub(super) fn input_item(message: &PromptMessage) -> Value {
    match message.role {
        PromptMessageRole::System => json!({"role":"developer","content":message.content}),
        PromptMessageRole::User => json!({"role":"user","content":message.content}),
        PromptMessageRole::Assistant => {
            json!({"role":"assistant","phase":"final_answer","content":message.content})
        }
    }
}

pub(super) struct Previous {
    pub session: String,
    pub scope: Value,
    pub definitions: Vec<Value>,
    pub expected: Vec<PromptMessage>,
    pub id: String,
    pub call_id: Option<String>,
    pub replay: Vec<Value>,
}

impl Previous {
    /// Continue only when the accepted local reply is still an exact prefix.
    /// User additions stay in order and only a confirmed receipt answers a tool call.
    pub fn delta(
        &self,
        messages: &[PromptMessage],
        turn: &AgentTurn,
        catalogue: &Catalogue,
    ) -> Option<Vec<Value>> {
        if self.session != turn.session
            || self.scope != turn.scope
            || self.definitions != catalogue.definitions
            || !messages.starts_with(&self.expected)
            || messages.len() == self.expected.len()
        {
            return None;
        }
        let start = self.expected.len();
        let mut delta: Vec<_> = messages[start..].iter().map(input_item).collect();
        if let Some(id) = &self.call_id {
            let index = turn.feedback_index?.checked_sub(start)?;
            let receipt = messages.get(start + index)?;
            if receipt.role != PromptMessageRole::User {
                return None;
            }
            delta[index] =
                json!({"type":"function_call_output","call_id":id,"output":receipt.content});
        }
        Some(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Catalogue {
        fn new(turn: &AgentTurn) -> Self {
            Self::for_api(turn, xrtranslate_assets::remote::AgentApi::OpenaiResponses)
        }
    }

    fn turn(names: &[&str]) -> AgentTurn {
        let tools: Vec<_> = names.iter().map(|name| json!({
            "name": name, "description": "Read a document", "inputSchema": {"type":"object"},
            "read_only": true
        })).collect();
        serde_json::from_value(json!({"session":"local", "scope":{"doc":"a"}, "tools":tools}))
            .unwrap()
    }

    fn message(role: PromptMessageRole, content: &str) -> PromptMessage {
        PromptMessage {
            role,
            content: content.into(),
        }
    }

    #[test]
    fn dashscope_large_inventory_dispatches_only_declared_operations_or_local_discovery() {
        use xrtranslate_assets::remote::AgentApi;
        let names: Vec<_> = (0..65).map(|i| format!("notes.read_{i}")).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let catalogue = Catalogue::for_api(&turn(&refs), AgentApi::DashscopeResponses);
        let tools = catalogue.wire_tools();
        assert_eq!(tools.as_array().unwrap().len(), 1);
        assert_eq!(tools[0]["type"], "function");
        let select = |name| {
            json!({"output":[{"type":"function_call","name":"director_operation","call_id":"call_1",
            "arguments":json!({"name":name,"arguments":{},"memory":"read"}).to_string()}]})
        };
        for name in ["tools.list", "notes.read_64"] {
            let (result, _) = catalogue.decision(&select(name)).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&result.text).unwrap()["action"]["tool"]["name"],
                name
            );
        }
        assert!(catalogue.decision(&select("notes.delete")).is_err());
    }

    #[test]
    fn only_an_accepted_local_reply_and_its_receipt_can_continue() {
        let mut turn = turn(&["a.read"]);
        let catalogue = Catalogue::new(&turn);
        let accepted = vec![message(PromptMessageRole::Assistant, "accepted decision")];
        let previous = Previous {
            session: turn.session.clone(),
            scope: turn.scope.clone(),
            definitions: catalogue.definitions.clone(),
            expected: accepted.clone(),
            id: "r1".into(),
            call_id: Some("call1".into()),
            replay: Vec::new(),
        };
        let mut transcript = accepted;
        transcript.push(message(PromptMessageRole::User, "also check the title"));
        transcript.push(message(PromptMessageRole::User, "actual tool receipt"));
        assert!(previous.delta(&transcript, &turn, &catalogue).is_none());
        turn.feedback_index = Some(2);
        let delta = previous.delta(&transcript, &turn, &catalogue).unwrap();
        assert_eq!(
            delta,
            [
                json!({"role":"user","content":"also check the title"}),
                json!({"type":"function_call_output","call_id":"call1","output":"actual tool receipt"})
            ]
        );
        turn.scope = json!({"doc":"b"});
        assert!(previous.delta(&transcript, &turn, &catalogue).is_none());
        turn.scope = previous.scope.clone();
        turn.session = "other process".into();
        assert!(previous.delta(&transcript, &turn, &catalogue).is_none());
        turn.session = previous.session.clone();
        turn.tools[0].description.push('!');
        assert!(
            previous
                .delta(&transcript, &turn, &Catalogue::new(&turn))
                .is_none()
        );
        transcript[0].content = "a correction replaced the decision".into();
        assert!(previous.delta(&transcript, &turn, &catalogue).is_none());
    }

    #[test]
    fn deterministic_tool_names_are_unambiguous_and_each_response_selects_one_operation() {
        let catalogue = Catalogue::new(&turn(&["a.b", "a_b"]));
        assert_eq!(
            catalogue.definitions,
            Catalogue::new(&turn(&["a_b", "a.b"])).definitions
        );
        let call = |i: usize| {
            json!({
                "type":"function_call", "call_id":"c1", "name":catalogue.definitions[i]["name"],
                "arguments":r#"{"arguments":{},"memory":"work verified"}"#
            })
        };
        for (i, local) in ["a.b", "a_b"].iter().enumerate() {
            let (answer, id) = catalogue.decision(&json!({"output":[call(i)]})).unwrap();
            let decision: Value = serde_json::from_str(&answer.text).unwrap();
            assert_eq!(decision["action"]["tool"]["name"], *local);
            assert_eq!(id.as_deref(), Some("c1"));
        }
        assert!(
            catalogue
                .decision(&json!({"output":[call(0),call(1)]}))
                .is_err()
        );
    }
}
