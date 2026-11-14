//! Adapter for existing UI Director operations. No tutorial API is required in the client.
use crate::labels;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use xrt_guide::guide::{Context, Page, Step};

const REQUIRED: [&str; 6] = [
    "ui.pages",
    "ui.page",
    "ui.read",
    "ui.click",
    "ui.scroll",
    "ui.cancel",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    Disconnected,
    Incompatible,
    Unavailable,
    Launch,
    AlreadyRunning,
    Cancelled,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Remote {
    pub port: u16,
    pub generation: u64,
    pub current: Arc<AtomicU64>,
}

#[derive(Debug, Deserialize)]
struct Control {
    id_hex: String,
    label: String,
    kind: String,
    value: Value,
    enabled: bool,
    rect: [f32; 4],
    #[serde(default)]
    clip_rect: Option<[f32; 4]>,
    #[serde(default)]
    actions: Vec<String>,
}

impl Control {
    fn matches(&self, key: &str) -> bool {
        labels::matches(&self.label, key)
            || key == "Translation languages"
                && ["Source language", "Target language"]
                    .iter()
                    .any(|key| labels::matches(&self.label, key))
            || key == "Audio preprocessing" && labels::matches(&self.label, "Audio processing")
    }
    fn visible(&self) -> bool {
        let [x, y, w, h] = self.rect;
        w > 0.0
            && h > 0.0
            && self.clip_rect.is_none_or(|[cx, cy, cw, ch]| {
                x + w / 2.0 >= cx
                    && x + w / 2.0 <= cx + cw
                    && y + h / 2.0 >= cy
                    && y + h / 2.0 <= cy + ch
            })
    }
}

impl Remote {
    fn check(&self) -> Result<()> {
        if self.current.load(Ordering::Acquire) == self.generation {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    }

    fn request(&self, request: &Value) -> Result<Value> {
        let response = ui_director::client::send(self.port, request, Duration::from_secs(2))
            .map_err(|_| Error::Disconnected)?;
        if response["success"] != true {
            return Err(Error::Incompatible);
        }
        Ok(response["data"].clone())
    }

    pub fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        self.check()?;
        let mut value = self.request(&json!({"name": name, "arguments": arguments}))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let Some(pending) = value.get("pending").cloned() else {
                self.check()?;
                return Ok(value.get("result").cloned().unwrap_or(value));
            };
            if self.check().is_err() || Instant::now() >= deadline {
                if let Some(cancel) = value.get("cancel") {
                    let _ = self.request(cancel);
                }
                self.check()?;
                return Err(Error::Unavailable);
            }
            thread::sleep(Duration::from_millis(30));
            value = self.request(&pending)?;
        }
    }

    pub fn connect(&self, executable: Option<&Path>) -> Result<Context> {
        let initial = self.call("ui.pages", json!({}));
        if let Err(error @ (Error::Incompatible | Error::Cancelled)) = initial {
            return Err(error);
        }
        if initial.is_err() {
            if let Some(executable) = executable {
                self.check()?;
                let mut command = std::process::Command::new(executable);
                command.args(["--director-port", &self.port.to_string()]);
                command.current_dir(executable.parent().ok_or(Error::Launch)?);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x0800_0000); // no console; keep the client's own window
                }
                let mut child = command.spawn().map_err(|_| Error::Launch)?;
                let deadline = Instant::now() + Duration::from_secs(25);
                loop {
                    self.check()?;
                    if self.call("ui.pages", json!({})).is_ok() {
                        break;
                    }
                    if let Some(status) = child.try_wait().ok().flatten() {
                        return Err(if status.success() {
                            Error::AlreadyRunning
                        } else {
                            Error::Launch
                        });
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::Disconnected);
                    }
                    thread::sleep(Duration::from_millis(150));
                }
                // Exiting the player deliberately leaves the user's client running.
            } else {
                return Err(Error::Disconnected);
            }
        }
        let tools = self.call("tools.describe", json!({"names": REQUIRED}))?;
        let tools = tools["tools"].as_array().ok_or(Error::Incompatible)?;
        if !REQUIRED
            .iter()
            .all(|name| tools.iter().any(|tool| tool["name"] == *name))
        {
            return Err(Error::Incompatible);
        }
        self.context()
    }

    pub fn context(&self) -> Result<Context> {
        let value = self.call("ui.pages", json!({}))?;
        let pages: Vec<Page> = value["pages"]
            .as_array()
            .ok_or(Error::Incompatible)?
            .iter()
            .filter_map(|page| {
                let route = page["route"].as_str()?;
                (!route.starts_with("onboarding:")).then(|| Page {
                    route: route.into(),
                    label: page["label"].as_str().unwrap_or(route).into(),
                })
            })
            .collect();
        let language = pages
            .iter()
            .find(|page| page.route == "Translation")
            .map_or("en", |page| labels::language(&page.label));
        Ok(Context {
            page: value["location"]["page"]
                .as_str()
                .ok_or(Error::Incompatible)?
                .into(),
            language: language.into(),
            pages,
            capabilities: vec![],
        })
    }

    fn controls(&self, route: &str) -> Result<Vec<Control>> {
        let mut controls = vec![];
        let mut offset = 0;
        loop {
            let value = self.call("ui.read", json!({"offset": offset, "limit": 50}))?;
            if value["location"]["page"] != route {
                return Err(Error::Unavailable);
            }
            controls.extend(
                serde_json::from_value::<Vec<Control>>(value["controls"].clone())
                    .map_err(|_| Error::Incompatible)?,
            );
            match value["next_offset"].as_u64() {
                Some(next) if next > offset && next < 2000 => offset = next,
                None => return Ok(controls),
                _ => return Err(Error::Incompatible),
            }
        }
    }

    /// Returns the editor's *visible* admission state, never inferred from files or version.
    pub fn present(&self, page: &str, step: &Step) -> Result<Option<bool>> {
        self.call("ui.page", json!({"page": page}))?;
        let mut controls = self.controls(page)?;
        let editor = if page == "PromptStudio" {
            if let Some(back) = controls
                .iter()
                .find(|control| control.matches("Back to styles") && control.enabled)
            {
                self.call("ui.click", json!({"target": back.id_hex}))?;
                controls = self.controls(page)?;
            }
            Some(
                controls
                    .iter()
                    .any(|control| control.matches("Edit prompts") && control.enabled),
            )
        } else {
            None
        };
        if step.id == "edit-prompts" && editor != Some(true) {
            return Err(Error::Unavailable);
        }
        let mut revealed = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut scrolls = 0;
        loop {
            self.check()?;
            let target = controls
                .iter()
                .filter(|control| step.targets.iter().any(|key| control.matches(key)))
                .min_by_key(|control| matches!(control.kind.as_str(), "button" | "tab"));
            if let Some(target) = target.filter(|control| {
                !matches!(control.kind.as_str(), "button" | "tab")
                    || revealed.len() == step.reveal.len()
            }) {
                if target.visible() {
                    // Reading one target uses the old client's own visual feedback.
                    self.call("ui.read", json!({"target": target.id_hex}))?;
                    return Ok(editor);
                }
                if scrolls < 4
                    && let Some([x, y, w, h]) = target.clip_rect
                {
                    let delta = (y + h / 2.0 - (target.rect[1] + target.rect[3] / 2.0))
                        .clamp(-600.0, 600.0);
                    self.call(
                        "ui.scroll",
                        json!({"x": x+w/2.0, "y": y+h/2.0, "delta": delta}),
                    )?;
                    scrolls += 1;
                }
            }
            // Older registries do not name every custom control. Once its view is
            // open the explanation remains useful; highlighting is best effort.
            if revealed.len() == step.reveal.len() {
                return Ok(editor);
            }
            let disclosure = step
                .reveal
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, key)| !revealed.contains(*key))
                .find_map(|(index, key)| {
                    controls
                        .iter()
                        .filter(|control| {
                            control.matches(key)
                                && control.enabled
                                && control.actions.iter().any(|action| action == "click")
                        })
                        .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
                        .map(|control| (index, control))
                });
            if let Some((index, control)) = disclosure {
                // A visible inner disclosure means its outer view is already open.
                revealed.extend(step.reveal.iter().take(index + 1).cloned());
                if control.value != true {
                    self.call("ui.click", json!({"target": control.id_hex}))?;
                }
            }
            if step.targets.is_empty() {
                return Ok(editor);
            }
            if Instant::now() >= deadline {
                return Err(Error::Unavailable);
            }
            thread::sleep(Duration::from_millis(100));
            controls = self.controls(page)?;
        }
    }
}

#[cfg(test)]
mod tests;
