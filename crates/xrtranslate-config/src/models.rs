//! Read-only installed model discovery and in-memory package selection.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;
use xrtranslate_assets::{
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelHardwareRequirements, ModelRuntime,
    manifest_for,
};
use xrtranslate_engine::language::{LanguageSet, SupportedLanguage};

use crate::AppConfig;

/// Installation is verified by file presence, readability and size. Runtime
/// health, free VRAM and inference quality still require executing the model.
#[derive(Debug, Serialize)]
pub struct InstalledModel {
    pub id: ModelAssetId,
    pub label: &'static str,
    pub capability: ModelCapability,
    pub provider: &'static str,
    pub languages: &'static [&'static str],
    pub hardware: ModelHardwareRequirements,
    pub directory: PathBuf,
}

impl InstalledModel {
    /// Use the same locale/base-language policy as the production pipeline.
    pub fn supports_language(&self, language: SupportedLanguage) -> bool {
        LanguageSet::from_codes(
            self.languages,
            self.capability != ModelCapability::Translation,
        )
        .contains(language)
    }
}

impl AppConfig {
    /// Catalogue-backed Chat choices, including explicitly declared custom and
    /// account models. Availability of credentials is checked when saving/running.
    pub fn chat_model_selections(&self) -> Vec<crate::TextModelSelection> {
        let mut choices = Vec::new();
        for (provider, settings) in &self.translation.providers {
            let local = settings
                .get("transport")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|transport| matches!(transport, "local" | "onnx-cpu"));
            let models: Vec<String> = if local {
                xrtranslate_assets::manifests_for_capability(ModelCapability::Translation)
                    .filter(|model| model.provider == provider)
                    .filter(|model| {
                        matches!(
                            model.runtime,
                            Some(ModelRuntime::LlamaTextChat {
                                supports_chat: true,
                                ..
                            })
                        )
                    })
                    .map(|model| model.id.as_str().to_owned())
                    .collect()
            } else {
                xrtranslate_assets::remote::services(provider, ModelCapability::Translation)
                    .flat_map(|service| service.models.iter().cloned())
                    .chain(
                        settings
                            .get("model")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                    )
                    .chain(
                        self.agent
                            .model
                            .as_ref()
                            .filter(|selection| selection.provider == *provider)
                            .map(|selection| selection.model.clone()),
                    )
                    .collect()
            };
            for model in models {
                let selection = crate::TextModelSelection {
                    provider: provider.clone(),
                    model,
                };
                if choices.contains(&selection) {
                    continue;
                }
                let chat = local || self.text_model_supports_chat(&selection);
                if chat {
                    choices.push(selection);
                }
            }
        }
        choices
    }

    pub fn text_model_supports_chat(&self, selection: &crate::TextModelSelection) -> bool {
        self.with_text_model(selection).is_ok_and(|config| {
            if let Some(id) = ModelAssetId::from_config_key(&selection.model) {
                return matches!(
                    manifest_for(id).runtime,
                    Some(ModelRuntime::LlamaTextChat {
                        supports_chat: true,
                        ..
                    })
                );
            }
            let settings = config.translation.providers[&selection.provider].as_object();
            let transport = settings
                .and_then(|s| s.get("transport"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let mut issues = Vec::new();
            let (_, capabilities) = crate::remote::resolve(
                "translation",
                settings,
                transport,
                &selection.provider,
                &mut issues,
            );
            issues.is_empty() && capabilities.supports_chat
        })
    }

    /// Resolve a text model using the existing provider's credentials and tuning.
    /// This temporary selection cannot change the user's translation choice.
    pub fn with_text_model(&self, selection: &crate::TextModelSelection) -> Result<Self, String> {
        let provider = self
            .translation
            .providers
            .get(&selection.provider)
            .ok_or("The selected text provider is not configured")?;
        if let Some(id) = ModelAssetId::from_config_key(&selection.model) {
            let model = manifest_for(id);
            if model.capability != ModelCapability::Translation
                || model.provider != selection.provider
            {
                return Err("The text model does not belong to this provider".into());
            }
            return self.with_model_assets(&[id]);
        }
        if matches!(
            provider
                .get("transport")
                .and_then(serde_json::Value::as_str),
            Some("local" | "onnx-cpu")
        ) {
            return Err("Select a local model from the catalogue".into());
        }
        let mut raw = self.raw.clone();
        raw["translation"]["provider"] = json!(selection.provider);
        raw["translation"]["providers"][&selection.provider]["model"] = json!(selection.model);
        let mut selected = Self::from_value(raw).map_err(|error| error.to_string())?;
        selected.source_path = self.source_path.clone();
        Ok(selected)
    }

    /// A separate Agent uses the same model plan machinery as translation.
    /// Following, or explicitly selecting the identical route, reuses that plan.
    pub fn for_agent(&self) -> Result<Option<Self>, String> {
        let Some(selection) = &self.agent.model else {
            return Ok(None);
        };
        let mut selected = self.with_text_model(selection)?;
        let route = selected
            .native_model_route()
            .map_err(|error| error.to_string())?;
        if !route.translation.supports_chat()? {
            return Err("Agent requires a model with general chat capability".into());
        }
        if self
            .native_model_route()
            .map_err(|error| error.to_string())?
            .translation
            == route.translation
        {
            return Ok(None);
        }
        selected.agent.model = None;
        selected.raw["agent"]["model"] = serde_json::Value::Null;
        Ok(Some(selected))
    }

    pub fn agent_provider(&self) -> Result<crate::NativeProviderConfig, String> {
        self.for_agent()?
            .as_ref()
            .unwrap_or(self)
            .native_model_route()
            .map(|route| route.translation)
            .map_err(|error| error.to_string())
    }

    /// Shared install paths, independent of the currently selected providers.
    pub fn model_asset_paths(&self) -> ModelAssetsConfig {
        ModelAssetsConfig::with_directory_overrides(
            self.model_manager.models_directory.clone(),
            self.model_manager.qwen3_asr_gguf_directory.clone(),
            self.model_manager.hunyuan_mt_gguf_directory.clone(),
        )
    }

    /// Enumerates all installed catalog models, including inactive packages.
    /// Remote endpoints are not installations and are not included.
    pub fn installed_models(&self, project_root: &Path) -> Vec<InstalledModel> {
        self.model_asset_paths()
            .resolve_selected(project_root)
            .installed_assets()
            .map(|asset| {
                let model = asset.manifest();
                InstalledModel {
                    id: model.id,
                    label: model.label,
                    capability: model.capability,
                    provider: model.provider,
                    languages: model.languages,
                    hardware: model.hardware,
                    directory: asset.directory().to_owned(),
                }
            })
            .collect()
    }

    /// Returns a temporary configuration selecting exactly one package per
    /// supplied capability. Unspecified capabilities and provider tuning remain
    /// unchanged. Neither the base file nor the user override file is written.
    pub fn with_model_assets(&self, ids: &[ModelAssetId]) -> Result<Self, String> {
        let mut raw = self.raw.clone();
        let mut seen = Vec::new();
        for &id in ids {
            let model = manifest_for(id);
            if seen.contains(&model.capability) {
                return Err(format!(
                    "duplicate capability in model selection: {:?}",
                    model.capability
                ));
            }
            seen.push(model.capability);
            let section = match model.capability {
                ModelCapability::Asr => "asr",
                ModelCapability::Translation => "translation",
                ModelCapability::Tts => "tts",
                ModelCapability::Ocr => "ocr",
                ModelCapability::SourceSeparation | ModelCapability::NoiseSuppression => {
                    return Err(
                        "Audio preprocessing is selected by each input or media task.".into(),
                    );
                }
            };
            raw[section]["provider"] = json!(model.provider);
            let provider = &mut raw[section]["providers"][model.provider];
            if provider.is_null() {
                *provider = json!({});
            }
            let object = provider
                .as_object_mut()
                .ok_or("provider configuration must be an object")?;
            object.insert("model_asset".into(), json!(id.as_str()));
            object.insert("model_assets".into(), json!([id.as_str()]));
            object.insert(
                "transport".into(),
                json!(model.runtime.map_or("onnx", ModelRuntime::transport)),
            );
            if matches!(
                model.capability,
                ModelCapability::Asr | ModelCapability::Translation
            ) {
                // Selecting a local package cannot retain a remote endpoint.
                let port = if model.capability == ModelCapability::Asr {
                    8001
                } else {
                    8002
                };
                object.insert(
                    "url".into(),
                    json!(format!("http://127.0.0.1:{port}/v1/chat/completions")),
                );
            }
        }
        let mut selected = Self::from_value(raw).map_err(|error| error.to_string())?;
        selected.source_path = self.source_path.clone();
        Ok(selected)
    }
}
