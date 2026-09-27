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
            if model.capability != ModelCapability::Tts {
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
