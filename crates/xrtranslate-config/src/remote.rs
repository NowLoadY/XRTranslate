//! Resolve online presets and normalize their model features at the config boundary.
use crate::AsrPromptMode;
use serde_json::{Map, Value};
use xrtranslate_assets::{
    ModelCapability,
    remote::{self, RemoteCapabilities, RemoteConnector, RemoteStreaming},
};

/// Validated online synthesis selection. Credentials stay in runtime settings.
pub struct RemoteTtsConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: &'static remote::RemoteModelManifest,
}

impl crate::AppConfig {
    pub fn remote_tts(&self) -> Result<Option<RemoteTtsConfig>, String> {
        let provider = self.tts.provider.trim();
        if provider.is_empty() || provider == "none" {
            return Ok(None);
        }
        let settings = self
            .tts
            .provider_config(provider)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("tts.providers.{provider} must be an object"))?;
        let field = |key| {
            settings
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
        };
        if matches!(field("transport"), "onnx" | "local") {
            return Ok(None);
        }
        let mut issues = Vec::new();
        let (connector, _) = resolve(
            "tts",
            Some(settings),
            field("transport"),
            "tts",
            &mut issues,
        );
        if !issues.is_empty() {
            return Err(issues.join("; "));
        }
        let model = connector
            .and_then(|connector| remote::remote_model(connector, field("model")))
            .filter(|model| model.tts.is_some())
            .ok_or_else(|| "Select a supported TTS model on the Welcome Page.".to_owned())?;
        for key in ["url", "api_key"] {
            if field(key).is_empty() {
                return Err(format!("tts.providers.{provider}.{key} is required"));
            }
        }
        let mut endpoint = field("url").to_owned();
        if endpoint.contains("{workspace_id}") {
            let workspace = field("workspace_id");
            if workspace.is_empty()
                || workspace.len() > 63
                || !workspace
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err("Enter a valid workspace ID for the selected TTS provider.".into());
            }
            endpoint = endpoint.replace("{workspace_id}", workspace);
        }
        Ok(Some(RemoteTtsConfig {
            endpoint,
            api_key: field("api_key").to_owned(),
            model,
        }))
    }
}

/// Expand only configured preset entries. Custom provider IDs remain ordinary
/// objects and use exactly the same connector/capability resolution.
pub fn apply_remote_defaults(document: &mut Value) {
    for provider in &remote::catalog().providers {
        for (section, capability) in [
            ("asr", ModelCapability::Asr),
            ("translation", ModelCapability::Translation),
            ("tts", ModelCapability::Tts),
        ] {
            for service in provider
                .services
                .iter()
                .filter(|s| s.capability == capability)
            {
                for alias in &service.provider_aliases {
                    if let Some(category) = document.get_mut(section) {
                        fold_provider_alias(category, &provider.id, alias, service);
                    }
                }
            }
            let Some(settings) = document
                .get_mut(section)
                .and_then(|section| section.get_mut("providers"))
                .and_then(|providers| providers.get_mut(&provider.id))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            let services = || {
                provider
                    .services
                    .iter()
                    .filter(|s| s.capability == capability)
            };
            let model = settings
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(service) = services()
                .find(|service| service.models.iter().any(|id| id == model))
                .or_else(|| services().next())
            else {
                continue;
            };
            for (key, value) in &service.defaults {
                if key == "context_window_tokens" {
                    continue;
                }
                // Follow the selected model when switching built-in protocols.
                // Credentials and explicitly customized endpoints remain user-owned.
                let inherited_route = matches!(
                    key.as_str(),
                    "connector" | "url" | "max_tokens" | "parallel_slots"
                ) && services().any(|other| {
                    settings
                        .get(key)
                        .is_some_and(|current| other.defaults.get(key) == Some(current))
                });
                if inherited_route {
                    settings.insert(key.clone(), value.clone());
                } else {
                    settings.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            settings
                .entry("guide_url")
                .or_insert_with(|| Value::from(provider.guide_url.clone()));
        }
    }
    // Legacy UI/runtime consumers still read transport. Keep it derived from
    // the explicit connector when present, including for custom providers.
    for section in ["asr", "translation", "tts"] {
        if let Some(providers) = document
            .get_mut(section)
            .and_then(|value| value.get_mut("providers"))
            .and_then(Value::as_object_mut)
        {
            for settings in providers.values_mut().filter_map(Value::as_object_mut) {
                if let Some(connector) = settings
                    .get("connector")
                    .and_then(|value| serde_json::from_value::<RemoteConnector>(value.clone()).ok())
                {
                    settings.insert("transport".into(), Value::from(connector.transport()));
                }
            }
        }
    }
    crate::context_window::migrate_context_windows(document);
}

fn fold_provider_alias(
    category: &mut Value,
    provider: &str,
    alias: &str,
    service: &remote::RemoteServiceManifest,
) {
    let selected = category.get("provider").and_then(Value::as_str) == Some(alias);
    let Some(providers) = category.get_mut("providers").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(Value::Object(mut previous)) = providers.remove(alias) else {
        return;
    };
    if selected {
        for (key, value) in &service.defaults {
            if key != "context_window_tokens" {
                previous.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        providers.insert(provider.to_owned(), Value::Object(previous));
        category["provider"] = provider.into();
    } else if let Some(key) = previous.remove("api_key") {
        let settings = providers
            .entry(provider)
            .or_insert_with(|| Value::Object(Map::new()));
        if settings
            .get("api_key")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            settings["api_key"] = key;
        }
    }
}

pub(crate) fn resolve(
    section: &str,
    settings: Option<&Map<String, Value>>,
    transport: &str,
    path: &str,
    issues: &mut Vec<String>,
) -> (Option<RemoteConnector>, RemoteCapabilities) {
    let get = |key| settings.and_then(|settings| settings.get(key));
    let url = get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let model = get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let connector = if let Some(value) = get("connector") {
        match serde_json::from_value::<RemoteConnector>(value.clone()) {
            Ok(connector) => Some(connector),
            Err(error) => {
                issues.push(format!("{path}.connector: {error}"));
                None
            }
        }
    } else {
        RemoteConnector::from_transport(section, transport, url)
    };
    if connector.is_none() && !matches!(transport, "local" | "onnx-cpu") {
        issues.push(format!(
            "{path}.connector does not support {section} with transport {transport:?}"
        ));
    }
    let capability = match section {
        "asr" => ModelCapability::Asr,
        "tts" => ModelCapability::Tts,
        _ => ModelCapability::Translation,
    };
    if connector.is_some_and(|connector| connector.capability() != capability) {
        issues.push(format!("{path}.connector does not support {section}"));
    }
    if connector.is_some_and(|connector| connector.account_provider().is_some())
        && connector
            .and_then(|connector| remote::model_capabilities(connector, model))
            .is_none()
    {
        issues.push(format!(
            "{path}.model must select a registered account service profile"
        ));
    }
    let mut capabilities = if let Some(declared) =
        connector.and_then(|connector| remote::model_capabilities(connector, model))
    {
        declared.clone()
    } else {
        // Flat fields remain accepted for existing custom configurations.
        let supports_context = get("supports_prompt_context")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let asr_prompt_mode = get("asr_prompt_mode")
            .cloned()
            .map(serde_json::from_value)
            .transpose();
        let mode = match asr_prompt_mode {
            Ok(Some(mode)) => mode,
            Ok(None)
                if supports_context
                    || get("supports_prompt").and_then(Value::as_bool) == Some(true) =>
            {
                AsrPromptMode::Instruction
            }
            Ok(None) => AsrPromptMode::None,
            Err(error) => {
                issues.push(format!("{path}.asr_prompt_mode: {error}"));
                AsrPromptMode::None
            }
        };
        RemoteCapabilities {
            translation_prompt_profile: Default::default(),
            // A chat-shaped endpoint does not prove the model follows instructions.
            // Unknown models opt in through their declared capabilities.
            supports_ask: false,
            supports_chat: false,
            supports_json_output: false,
            streaming: if capability == ModelCapability::Translation {
                RemoteStreaming::Delta
            } else {
                RemoteStreaming::None
            },
            supports_context,
            asr_prompt_mode: if capability == ModelCapability::Asr {
                mode
            } else {
                AsrPromptMode::None
            },
            asr_language_hint: get("supports_language").and_then(Value::as_bool),
            asr_response_format: None,
            context_max_chars: get("asr_context_max_chars")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok()),
            vocabulary_bias: get("supports_vocabulary_bias")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            single_user_message: false,
            reasoning_effort: None,
        }
    };
    // Explicit neutral overrides support private deployments and unknown revisions.
    if let Some(overrides) = get("capabilities") {
        let mut value = serde_json::to_value(&capabilities).expect("serializable capabilities");
        crate::merge_config_values(&mut value, overrides.clone());
        match serde_json::from_value(value) {
            Ok(value) => capabilities = value,
            Err(error) => issues.push(format!("{path}.capabilities: {error}")),
        }
    }
    if capability == ModelCapability::Asr
        && let Some(connector) = connector
    {
        if !connector.supports_prompt(capabilities.asr_prompt_mode) {
            issues.push(format!(
                "{path}.capabilities.asr_prompt_mode is not supported by this connector"
            ));
        }
        if capabilities.vocabulary_bias && !connector.supports_vocabulary() {
            issues.push(format!(
                "{path}.capabilities.vocabulary_bias is not supported by this connector"
            ));
        }
        if capabilities.streaming != RemoteStreaming::None
            && (!connector.supports_audio_streaming()
                || capabilities.streaming != RemoteStreaming::Cumulative)
        {
            issues.push(format!(
                "{path}.capabilities.streaming: live ASR requires a streaming audio connector with cumulative transcripts"
            ));
        }
    }
    if connector == Some(RemoteConnector::DoubaoWebAsr)
        && (capabilities.asr_language_hint == Some(true) || capabilities.supports_context)
    {
        issues.push(format!(
            "{path}.capabilities: this account connector cannot accept language hints or context"
        ));
    }
    if capabilities.asr_response_format.is_some()
        && connector != Some(RemoteConnector::OpenaiTranscription)
    {
        issues.push(format!(
            "{path}.capabilities.asr_response_format requires the openai-transcription connector"
        ));
    }
    if capabilities.reasoning_effort.is_some()
        && !matches!(
            connector,
            Some(
                RemoteConnector::OpenaiChat
                    | RemoteConnector::OpenaiAudioChat
                    | RemoteConnector::ChatgptResponses
            )
        )
    {
        issues.push(format!(
            "{path}.capabilities.reasoning_effort requires a chat inference connector"
        ));
    }
    if get("asr_context_max_chars")
        .is_some_and(|value| value.as_u64().is_none_or(|limit| limit == 0))
    {
        issues.push(format!(
            "{path}.asr_context_max_chars must be a positive integer"
        ));
    }
    if let Some(declared) = connector.and_then(remote::account_model_capabilities) {
        if &capabilities != declared {
            issues.push(format!(
                "{path}.capabilities must match the account connector's supported capabilities"
            ));
        }
    }
    if capabilities.context_max_chars == Some(0) {
        issues.push(format!(
            "{path}.capabilities.context_max_chars must be positive"
        ));
    }
    (connector, capabilities)
}

/// Materialize the built-in provider defaults for a release, with no API keys.
/// This never reads runtime or other user configuration files.
pub fn release_config_document(mut document: Value) -> Value {
    fn clear_keys(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if key == "api_key" {
                        *value = Value::from("");
                    } else {
                        clear_keys(value);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    clear_keys(value);
                }
            }
            _ => {}
        }
    }
    apply_remote_defaults(&mut document);
    clear_keys(&mut document);
    document
}
