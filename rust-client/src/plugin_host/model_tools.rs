//! Host-side Agent tools for discovering and installing optional plugin models.
use crate::{
    XRTranslateApp,
    model_install::{NativeModelTaskState, ResourceId},
    runtime_install::RuntimeInstallState,
    ui::components,
};
use serde::Deserialize;
use serde_json::{Value, json};
use ui_director::{Tool, ToolCall, ToolResult, arguments};

impl XRTranslateApp {
    pub(crate) fn plugin_model_tools(&self) -> Vec<Tool> {
        vec![
            Tool::new(
                "models.list",
                "List optional plugin models, their download sizes, selection status, and installation readiness.",
                json!({
                    "workspace": {"type": "string"},
                    "plugin": {"type": "string", "description": "Optional plugin ID filter, e.g. \"media\"."}
                }),
                &[],
            )
            .read_only(),
            Tool::new(
                "models.install",
                "Select and install an optional plugin model and its required runtime. Ask the user first with the model's purpose and download size; set user_confirmed=true only after the user explicitly agrees.",
                json!({
                    "workspace": {"type": "string"},
                    "plugin": {"type": "string", "description": "Plugin ID, e.g. \"media\"."},
                    "model": {"type": "string", "description": "Model ID from models.list, e.g. \"longpipe-small\"."},
                    "user_confirmed": {"type": "boolean", "description": "Must be true and set only after the user explicitly agreed to install this model."}
                }),
                &["plugin", "model", "user_confirmed"],
            ),
        ]
    }

    pub(crate) fn call_plugin_model_tool(&mut self, call: &ToolCall) -> Option<ToolResult> {
        match call.name.as_str() {
            "models.list" => Some(self.tool_models_list(call)),
            "models.install" => Some(self.tool_models_install(call)),
            "models.status" => Some(self.tool_models_status(call)),
            "models.cancel" => Some(self.tool_models_cancel(call)),
            _ => None,
        }
    }

    fn plugin_runtime_ready(&self, runtime: &str) -> bool {
        match runtime {
            "" => true,
            "onnx" | "onnx-cpu" => self
                .backend_manager
                .runtime_layout()
                .onnx_cpu_core_library()
                .is_file(),
            "onnx-cuda" | "llama.cpp" => {
                let reqs = self.runtime_requirements();
                let assets = self.service_config.selected_model_asset_ids();
                self.runtime_installer.plan_matches(reqs, &assets)
                    && self.runtime_installer.plan_is_ready()
            }
            _ => true,
        }
    }

    fn plugin_model_download_status(&self, id: &ResourceId) -> &'static str {
        if self.model_task_manager.is_model_ready(id) {
            return "installed";
        }
        if self.model_task_manager.is_model_paused(id) {
            return "paused";
        }
        if matches!(
            self.model_task_manager.state(),
            NativeModelTaskState::Installing { asset_id, .. } if asset_id == id
        ) {
            return "downloading";
        }
        if self.model_task_manager.is_model_queued(id) {
            return "queued";
        }
        if matches!(
            self.model_task_manager.state(),
            NativeModelTaskState::Idle | NativeModelTaskState::Discovering
        ) {
            return "checking";
        }
        "not_installed"
    }

    fn sync_plugin_models_registry(&mut self) {
        self.model_task_manager.set_plugin_models(
            self.plugin_host
                .model_resources(self.ui_language)
                .into_iter()
                .map(|model| (model.id, model.resource))
                .collect(),
        );
    }

    fn tool_models_list(&self, call: &ToolCall) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            plugin: Option<String>,
            #[serde(rename = "workspace")]
            _workspace: Option<String>,
        }
        let args: Args = arguments(call)?;
        let items = self
            .plugin_host
            .plugin_models_snapshot(args.plugin.as_deref(), self.ui_language);
        let models: Vec<Value> = items
            .into_iter()
            .map(|item| {
                let installed = self.model_task_manager.is_model_ready(&item.resource_id);
                let runtime_ready = self.plugin_runtime_ready(&item.runtime);
                let status = if installed && runtime_ready && item.selected {
                    "ready"
                } else {
                    self.plugin_model_download_status(&item.resource_id)
                };
                json!({
                    "plugin": item.plugin,
                    "plugin_title": item.plugin_title,
                    "id": item.id,
                    "label": item.label,
                    "group": item.group,
                    "selected": item.selected,
                    "installed": installed,
                    "runtime": item.runtime,
                    "runtime_ready": runtime_ready,
                    "download_bytes": item.download_bytes,
                    "download_size": components::format_file_size(item.download_bytes),
                    "status": status,
                })
            })
            .collect();
        Ok(json!({ "models": models }))
    }

    fn tool_models_install(&mut self, call: &ToolCall) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            plugin: String,
            model: String,
            user_confirmed: bool,
            #[serde(rename = "workspace")]
            _workspace: Option<String>,
        }
        let args: Args = arguments(call)?;
        if !args.user_confirmed {
            return Err(
                "Ask the user for confirmation before installing a model, then retry with user_confirmed=true."
                    .into(),
            );
        }
        let _ = self
            .plugin_host
            .plugin_model_state(&args.plugin, &args.model)?;
        self.plugin_host
            .set_model_selected(&args.plugin, &args.model, true)?;
        self.sync_plugin_models_registry();
        self.drive_plugin_model_install(&args.plugin, &args.model, true)
    }

    fn tool_models_status(&mut self, call: &ToolCall) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            plugin: String,
            model: String,
        }
        let args: Args = arguments(call)?;
        self.drive_plugin_model_install(&args.plugin, &args.model, false)
    }

    fn tool_models_cancel(&mut self, call: &ToolCall) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            plugin: String,
            model: String,
        }
        let args: Args = arguments(call)?;
        let res_id = ResourceId::Plugin {
            plugin: args.plugin,
            model: args.model,
        };
        let _ = self.model_task_manager.pause(res_id);
        if self.runtime_installer.is_busy() {
            let _ = self.runtime_installer.pause();
        }
        Ok(json!({ "cancelled": true }))
    }

    fn drive_plugin_model_install(
        &mut self,
        plugin: &str,
        model_id: &str,
        initiate: bool,
    ) -> ToolResult {
        let (label, runtime, selected, needs_live_refresh) = {
            let (model, selected, needs_live_refresh) =
                self.plugin_host.plugin_model_state(plugin, model_id)?;
            (
                model.label.clone(),
                model.runtime.clone(),
                selected,
                needs_live_refresh,
            )
        };
        if !selected {
            return Err(format!(
                "Model `{model_id}` for plugin `{plugin}` was deselected before installation finished."
            ));
        }

        let res_id = ResourceId::Plugin {
            plugin: plugin.to_owned(),
            model: model_id.to_owned(),
        };
        let model_ready = self.model_task_manager.is_model_ready(&res_id);
        if !model_ready {
            if !initiate && self.model_task_manager.is_model_paused(&res_id) {
                return Err(format!("Download of `{label}` was paused."));
            }
            if !initiate
                && let NativeModelTaskState::Failed(error) = self.model_task_manager.state()
                && self
                    .model_task_manager
                    .batch_snapshot()
                    .is_some_and(|batch| batch.failed_asset_id.as_ref() == Some(&res_id))
            {
                return Err(format!("Failed to download `{label}`: {error}"));
            }
            let active_or_queued = self.model_task_manager.is_model_queued(&res_id)
                || matches!(
                    self.model_task_manager.state(),
                    NativeModelTaskState::Installing { asset_id, .. } if asset_id == &res_id
                );
            let discovering = matches!(
                self.model_task_manager.state(),
                NativeModelTaskState::Discovering
            );
            if initiate || (!active_or_queued && !discovering) {
                self.model_task_manager
                    .enqueue_many(self.project_root(), [res_id.clone()])?;
            }
        }

        let runtime_ready = self.plugin_runtime_ready(&runtime);
        if !runtime_ready {
            let reqs = self.runtime_requirements();
            let assets = self.service_config.selected_model_asset_ids();
            if !initiate && self.runtime_installer.is_paused() {
                return Err("Runtime download was paused.".into());
            }
            if !initiate
                && let RuntimeInstallState::Failed(error) = self.runtime_installer.state()
            {
                return Err(format!("Failed to install required runtime: {error}"));
            }
            if !self.runtime_installer.is_busy() {
                if !self.runtime_installer.plan_matches(reqs, &assets) {
                    self.runtime_installer
                        .prepare_for(self.project_root(), reqs)?;
                } else if !self.runtime_installer.plan_is_ready() {
                    self.runtime_installer
                        .install_recommended(self.project_root())?;
                }
            }
        }

        if model_ready && runtime_ready && !needs_live_refresh {
            return Ok(json!({
                "status": "ready",
                "plugin": plugin,
                "model": model_id,
                "label": label,
                "installed": true,
                "runtime_ready": true,
            }));
        }

        Ok(json!({
            "pending": {
                "name": "models.status",
                "arguments": { "plugin": plugin, "model": model_id }
            },
            "cancel": {
                "name": "models.cancel",
                "arguments": { "plugin": plugin, "model": model_id }
            }
        }))
    }
}
