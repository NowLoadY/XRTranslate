use serde::Deserialize;
use serde_json::json;
use ui_director::{Tool, ToolCall, ToolResult, arguments};

pub const READ: &str = "skills.read";
const IMAGEGEN: &str = include_str!("../skills/imagegen/SKILL.md");

pub fn tool() -> Tool {
    Tool::new(READ, "Read an available built-in skill or its named reference on demand. imagegen covers raster image generation and edits.", json!({
        "workspace":{"type":"string"},
        "name":{"type":"string","enum":["imagegen"]},
        "resource":{"type":"string","description":"Optional exact reference path listed in the skill."}
    }), &["name"]).read_only()
}

pub fn read(call: &ToolCall) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Read {
        name: String,
        resource: Option<String>,
        #[serde(rename = "workspace")]
        _workspace: Option<String>,
    }
    let args: Read = arguments(call)?;
    if args.name != "imagegen" {
        return Err("Unknown built-in skill".into());
    }
    let resource = args.resource.as_deref().unwrap_or("SKILL.md");
    let content = match resource {
        "SKILL.md" => IMAGEGEN,
        "references/prompting.md" => include_str!("../skills/imagegen/references/prompting.md"),
        "references/sample-prompts.md" => {
            include_str!("../skills/imagegen/references/sample-prompts.md")
        }
        _ => return Err("Read a reference named in the skill".into()),
    };
    Ok(json!({"name":args.name,"resource":resource,"content":content}))
}

pub fn inventory(available: bool) -> &'static str {
    if available {
        "Available skill: imagegen — generate raster artwork, make cutouts, or edit an existing local image. Read skills.read(name=imagegen) when the task calls for these operations."
    } else {
        "Raster generation and editing are unavailable in this scope. They require an OpenAI API-key Agent with Responses image tools."
    }
}
