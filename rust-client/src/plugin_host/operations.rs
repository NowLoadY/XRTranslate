//! Resolve the same asynchronous operation receipts used by the native CLI.
use super::*;
use std::time::{Duration, Instant};
use ui_director::{ToolCall, ToolResult};

/// Host consumers use the same asynchronous receipts as CLI/plugin callers.
pub(crate) struct Call {
    next: ToolCall,
    cancel: Option<ToolCall>,
}
impl Call {
    pub fn new(name: String, arguments: Value) -> Self {
        Self {
            next: ToolCall { name, arguments },
            cancel: None,
        }
    }
    pub fn poll(&mut self, host: &mut Host) -> Option<ToolResult> {
        use ui_director::ToolProvider;
        let result = host.call(&self.next);
        if let Ok(value) = &result
            && value.get("pending").is_some_and(Value::is_object)
        {
            match serde_json::from_value(value["pending"].clone()) {
                Ok(next) => {
                    self.next = next;
                    if let Some(cancel) = value.get("cancel") {
                        self.cancel = serde_json::from_value(cancel.clone()).ok();
                    }
                    return None;
                }
                Err(error) => return Some(Err(error.to_string())),
            }
        }
        Some(result)
    }
    pub fn cancel(&self, host: &mut Host) {
        use ui_director::ToolProvider;
        if let Some(cancel) = &self.cancel {
            let _ = host.call(cancel);
        }
    }
}

pub(super) struct Pending {
    plugin: String,
    id: u64,
    next: ToolCall,
    cancel: Option<ToolCall>,
    ready: Instant,
}

impl Host {
    pub(super) fn cancel_operations(&mut self, plugin: &str, id: Option<u64>) {
        self.inference.retain(|request| {
            if request.plugin != plugin || id.is_some_and(|id| id != request.id) {
                return true;
            }
            request.cancel.store(true, Ordering::Release);
            false
        });
        self.operations.retain(|request| {
            if request.plugin != plugin || id.is_some_and(|id| id != request.id) {
                return true;
            }
            self.cleanup.extend(request.cancel.clone());
            false
        });
    }
    pub(super) fn operation_reply(&mut self, plugin: String, id: u64, result: ToolResult) {
        if let Ok(value) = &result
            && let Some(pending) = value.get("pending").filter(|pending| pending.is_object())
        {
            let next = serde_json::from_value(pending.clone());
            let cancel = value
                .get("cancel")
                .cloned()
                .map(serde_json::from_value)
                .transpose();
            match (next, cancel) {
                (Ok(next), Ok(cancel)) => self.operations.push(Pending {
                    plugin,
                    id,
                    next,
                    cancel,
                    ready: Instant::now() + Duration::from_millis(50),
                }),
                _ => self.reply(
                    &plugin,
                    id,
                    Err("Invalid asynchronous operation receipt".into()),
                ),
            }
        } else {
            self.reply(&plugin, id, result);
        }
    }
}

impl crate::XRTranslateApp {
    pub(super) fn poll_plugin_operations(&mut self) {
        for call in std::mem::take(&mut self.plugin_host.cleanup) {
            let _ = self.call_director_tool(&call);
        }
        let pending = std::mem::take(&mut self.plugin_host.operations);
        for operation in pending {
            if Instant::now() < operation.ready {
                self.plugin_host.operations.push(operation);
                continue;
            }
            let mut result = self.call_director_tool(&operation.next);
            if let Ok(value) = &mut result
                && value.get("pending").is_some_and(Value::is_object)
                && value.get("cancel").is_none()
                && let Some(cancel) = operation.cancel
            {
                value["cancel"] = json!(cancel);
            }
            self.plugin_host
                .operation_reply(operation.plugin, operation.id, result);
        }
    }
}
