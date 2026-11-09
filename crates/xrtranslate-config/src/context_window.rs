use serde_json::Value;
use xrtranslate_assets::{
    DEFAULT_CONTEXT_WINDOW_TOKENS, ModelAssetId, ModelCapability, manifest_for,
    remote::{self, RemoteConnector},
};

/// Effective user value and its immutable catalogue default for one model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelContextWindow {
    pub model: String,
    pub current: u32,
    pub default: u32,
    pub minimum: u32,
    pub maximum: u32,
    pub customized: bool,
}

fn is_local(settings: &Value) -> bool {
    settings.get("connector").is_none()
        && settings
            .get("transport")
            .and_then(Value::as_str)
            .is_none_or(|transport| matches!(transport, "local" | "onnx-cpu"))
}

pub fn context_model_id(settings: &Value) -> Option<&str> {
    let local = is_local(settings);
    let key = if local { "model_asset" } else { "model" };
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .or_else(|| {
            local
                .then(|| settings.get("model_assets")?.as_array()?.first()?.as_str())
                .flatten()
        })
        .or_else(|| {
            settings
                .get("model")?
                .as_str()
                .map(str::trim)
                .filter(|model| !model.is_empty())
        })
        .map(|model| {
            if local {
                ModelAssetId::from_config_key(model).map_or(model, |id| id.as_str())
            } else {
                model
            }
        })
}

pub fn default_context_window(section: &str, provider: &str, settings: &Value) -> u32 {
    let model = context_model_id(settings).unwrap_or_default();
    if is_local(settings)
        && let Some(id) = ModelAssetId::from_config_key(model)
        && let Some(tokens) = manifest_for(id).default_context_window_tokens
    {
        return tokens;
    }
    let connector = settings
        .get("connector")
        .and_then(|value| serde_json::from_value::<RemoteConnector>(value.clone()).ok())
        .or_else(|| {
            RemoteConnector::from_transport(
                section,
                settings["transport"].as_str().unwrap_or_default(),
                settings["url"].as_str().unwrap_or_default(),
            )
        });
    if let Some(tokens) = connector
        .and_then(|connector| remote::remote_model(connector, model))
        .and_then(|model| model.default_context_window_tokens)
    {
        return tokens;
    }
    let capability = if section == "asr" {
        ModelCapability::Asr
    } else {
        ModelCapability::Translation
    };
    // Signed-in catalogues discover model IDs at runtime; their service supplies
    // the baseline. An unknown custom model otherwise uses the common baseline.
    remote::services(provider, capability)
        .find(|service| {
            service.models.iter().any(|id| id == model) || service.account_models.is_some()
        })
        .and_then(|service| service.defaults.get("context_window_tokens"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
}

pub(crate) fn context_window_override(settings: &Value) -> Option<&Value> {
    settings
        .get("context_windows")?
        .get(context_model_id(settings)?)
}

pub fn model_context_window(
    section: &str,
    provider: &str,
    settings: &Value,
) -> Option<ModelContextWindow> {
    let model = context_model_id(settings)?.to_owned();
    let default = default_context_window(section, provider, settings);
    let override_value = context_window_override(settings);
    let output_tokens = settings
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(if section == "asr" { 128 } else { 256 });
    Some(ModelContextWindow {
        model,
        current: override_value
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(default),
        default,
        minimum: output_tokens
            .saturating_add(128)
            .max(256)
            .min(u32::MAX as u64) as u32,
        maximum: if is_local(settings) {
            32_768
        } else {
            1_048_576
        },
        customized: override_value.is_some(),
    })
}

/// Older files stored one provider-wide value. Associate it only with the model
/// selected in that file, so switching models cannot carry it to another model.
pub(crate) fn migrate_context_windows(document: &mut Value) {
    for section in ["asr", "translation"] {
        let Some(providers) = document
            .get_mut(section)
            .and_then(|value| value.get_mut("providers"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (provider, settings) in providers {
            let Some(model) = context_model_id(settings).map(str::to_owned) else {
                continue;
            };
            let default = default_context_window(section, provider, settings);
            let Some(fields) = settings.as_object_mut() else {
                continue;
            };
            let Some(previous) = fields.remove("context_window_tokens") else {
                continue;
            };
            if previous.as_u64() == Some(u64::from(default)) {
                continue;
            }
            let overrides = fields
                .entry("context_windows")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(overrides) = overrides.as_object_mut() {
                overrides.entry(model).or_insert(previous);
            }
        }
    }
}
