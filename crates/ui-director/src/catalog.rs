//! Bounded discovery over provider-owned declarations; no plugin knowledge or execution state.
use super::{Tool, ToolCall, ToolResult, arguments};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "tools.list",
            "Discover available operations. Filter by namespace or query to receive instructions and argument schemas together; an unfiltered overview returns names and descriptions. schemas optionally overrides this default. query matches literal name/description text, not intent; if no matches, list a relevant namespace. Results are paged; namespace counts summarize the full catalog.",
            json!({"namespace":{"type":"string"},"query":{"type":"string","maxLength":256},"schemas":{"type":"boolean"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}}),
            &[],
        ).read_only(),
        Tool::new(
            "tools.describe",
            "Read full instructions and argument schemas for up to 8 exact operation names. Discover names with tools.list; inspect only the operations needed for the next step. Call the described operation directly using its name and arguments.",
            json!({"names":{"type":"array","minItems":1,"maxItems":8,"uniqueItems":true,"items":{"type":"string"}}}),
            &["names"],
        ).read_only(),
    ]
}

pub fn handles(name: &str) -> bool {
    matches!(name, "tools.list" | "tools.describe")
}

fn namespace(tool: &Tool) -> &str {
    tool.name
        .split_once('.')
        .map_or(tool.name.as_str(), |(prefix, _)| prefix)
}

pub fn namespaces(tools: &[Tool]) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for tool in tools {
        *counts.entry(namespace(tool)).or_default() += 1;
    }
    counts
}

pub fn call(tools: &[Tool], call: &ToolCall) -> ToolResult {
    match call.name.as_str() {
        "tools.list" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct List {
                namespace: Option<String>,
                query: Option<String>,
                #[serde(default)]
                offset: usize,
                limit: Option<usize>,
                schemas: Option<bool>,
            }
            let args: List = arguments(call)?;
            let limit = args.limit.unwrap_or(12);
            if !(1..=50).contains(&limit) {
                return Err("limit must be in 1..=50".into());
            }
            let query = args.query.unwrap_or_default().to_lowercase();
            if query.chars().count() > 256 {
                return Err("Search text must contain at most 256 characters".into());
            }
            let schemas = args
                .schemas
                .unwrap_or(args.namespace.is_some() || !query.is_empty());
            let mut matches = tools
                .iter()
                .filter(|tool| {
                    args.namespace
                        .as_deref()
                        .is_none_or(|name| namespace(tool) == name)
                        && (tool.name.to_lowercase().contains(&query)
                            || tool.description.to_lowercase().contains(&query))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|a, b| a.name.cmp(&b.name));
            let count = matches.len();
            let page = matches.into_iter().skip(args.offset).take(limit).map(|tool| {
                let mut entry = json!({"name":tool.name,"description":tool.description,"read_only":tool.read_only});
                if schemas {
                    entry["inputSchema"] = tool.input_schema.clone();
                }
                entry
            }).collect::<Vec<_>>();
            let next = args.offset.saturating_add(page.len());
            Ok(
                json!({"namespaces":namespaces(tools),"count":count,"tools":page,"next_offset":(next<count).then_some(next)}),
            )
        }
        "tools.describe" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Describe {
                names: Vec<String>,
            }
            let args: Describe = arguments(call)?;
            if !(1..=8).contains(&args.names.len()) {
                return Err("Supply 1 to 8 operation names".into());
            }
            let mut selected = Vec::new();
            for name in &args.names {
                let tool = tools
                    .iter()
                    .find(|tool| &tool.name == name)
                    .ok_or_else(|| format!("Tool unavailable: {name}; use tools.list"))?;
                if !selected
                    .iter()
                    .any(|previous: &&Tool| previous.name == tool.name)
                {
                    selected.push(tool);
                }
            }
            Ok(json!({"tools":selected}))
        }
        _ => Err("Unknown discovery operation".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_paged_and_describes_only_requested_operations() {
        let tools = vec![
            Tool::new(
                "notes.edit",
                "Edit a note",
                json!({"body":{"type":"string"}}),
                &["body"],
            ),
            Tool::new(
                "video.read",
                "Read a video",
                json!({"path":{"type":"string"}}),
                &["path"],
            )
            .read_only(),
            Tool::new("notes.read", "Read a note", json!({}), &[]).read_only(),
        ];
        let list = call(
            &tools,
            &ToolCall {
                name: "tools.list".into(),
                arguments: json!({"namespace":"notes","limit":1}),
            },
        )
        .unwrap();
        assert_eq!(list["count"], 2);
        assert_eq!(list["next_offset"], 1);
        assert_eq!(list["namespaces"]["video"], 1);
        assert_eq!(list["tools"][0]["name"], "notes.edit");
        assert_eq!(list["tools"][0], serde_json::to_value(&tools[0]).unwrap());
        for arguments in [json!({}), json!({"namespace":"notes","schemas":false})] {
            let overview = call(
                &tools,
                &ToolCall {
                    name: "tools.list".into(),
                    arguments,
                },
            )
            .unwrap();
            assert!(!overview.to_string().contains("inputSchema"));
        }
        let ready = call(
            &tools,
            &ToolCall {
                name: "tools.list".into(),
                arguments: json!({"query":"edit"}),
            },
        )
        .unwrap();
        assert_eq!(ready["count"], 1);
        assert_eq!(ready["tools"][0], serde_json::to_value(&tools[0]).unwrap());
        let page = call(
            &tools,
            &ToolCall {
                name: "tools.list".into(),
                arguments: json!({"namespace":"notes","offset":1,"limit":1}),
            },
        )
        .unwrap();
        assert_eq!(page["tools"][0]["name"], "notes.read");
        assert!(page["next_offset"].is_null());
        let described = call(
            &tools,
            &ToolCall {
                name: "tools.describe".into(),
                arguments: json!({"names":["notes.edit"]}),
            },
        )
        .unwrap();
        assert_eq!(
            described["tools"][0]["inputSchema"]["required"],
            json!(["body"])
        );
        assert_eq!(described["tools"].as_array().unwrap().len(), 1);
        assert!(
            call(
                &tools,
                &ToolCall {
                    name: "tools.describe".into(),
                    arguments: json!({"names":["unavailable.tool"]})
                }
            )
            .is_err()
        );
    }
}
