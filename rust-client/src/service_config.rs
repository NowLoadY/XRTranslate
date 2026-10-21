use serde_json::{Map, Value};
use std::path::PathBuf;
use xrtranslate_prompt::TranslationPromptProfile;

#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonFieldKind {
    String,
    Bool,
    Number,
    Json,
}

struct ConfigField {
    name: String,
    value: String,
    kind: JsonFieldKind,
}

struct ProviderCard {
    name: String,
    fields: Vec<ConfigField>,
}

struct ServiceCategory {
    key: &'static str,
    selected_provider: String,
    providers: Vec<ProviderCard>,
}

#[derive(Clone)]
pub(crate) struct OnboardingProviderState {
    pub selected: String,
    pub remote: bool,
    pub choices: Vec<OnboardingProviderChoice>,
    pub model: String,
    pub models: Vec<String>,
    pub api_key: String,
}

#[derive(Clone)]
pub(crate) struct OnboardingProviderChoice {
    pub name: String,
    pub remote: bool,
    pub guide_url: Option<String>,
    pub model_asset: Option<String>,
    pub model_assets: Vec<String>,
    pub supported_languages: Vec<String>,
    pub voices: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OnboardingSaveOutcome {
    Saved { resolved_error: Option<String> },
    IncompleteRemoteProvider,
}

/// Editable view of the ASR, translation, and TTS provider portions of `config.json`.
/// The original JSON document is retained so unrelated project settings are preserved.
pub struct ServiceConfigEditor {
    path: PathBuf,
    document: Value,
    categories: Vec<ServiceCategory>,
    dirty: bool,
    message: Option<String>,
    onboarding_save_error: Option<String>,
    asr_streaming: std::sync::OnceLock<Option<bool>>,
    translation_contract: std::sync::OnceLock<Option<(TranslationPromptProfile, bool)>>,
    language_capabilities:
        std::sync::OnceLock<Result<xrtranslate_engine::language::LanguageCapabilities, String>>,
}

impl ServiceConfigEditor {
    pub fn default_audio_preprocessing(&self) -> xrtranslate_protocol::AudioPreprocessing {
        if self
            .document
            .pointer("/denoise/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            xrtranslate_protocol::AudioPreprocessing::Gtcrn
        } else {
            xrtranslate_protocol::AudioPreprocessing::None
        }
    }

    pub fn load() -> Self {
        let path = project_config_path();
        let mut editor = Self {
            path,
            document: Value::Object(Map::new()),
            categories: Vec::new(),
            dirty: false,
            message: None,
            onboarding_save_error: None,
            language_capabilities: Default::default(),
            translation_contract: Default::default(),
            asr_streaming: Default::default(),
        };
        if let Err(error) = editor.reload() {
            editor.message = Some(error);
        }
        editor
    }

    pub fn reload(&mut self) -> Result<(), String> {
        self.language_capabilities.take();
        self.translation_contract.take();
        self.asr_streaming.take();
        self.document = xrtranslate_config::load_user_config_document(&self.path, &project_root())
            .map_err(|error| format!("Cannot read {}: {error}", self.path.display()))?;
        self.categories = [
            "asr",
            "translation",
            "tts",
            #[cfg(not(target_os = "android"))]
            "ocr",
        ]
        .into_iter()
        .map(|key| Self::make_category(&self.document, key))
        .collect();
        self.message = None;
        self.onboarding_save_error = None;
        self.dirty = false;
        Ok(())
    }

    pub fn translation_prompt_profile(&self) -> TranslationPromptProfile {
        self.translation_contract()
            .map(|(profile, _)| profile)
            .unwrap_or_default()
    }

    pub(crate) fn asr_streaming(&self) -> Option<bool> {
        *self.asr_streaming.get_or_init(|| {
            let config = xrtranslate_config::AppConfig::from_value(self.document.clone()).ok()?;
            Some(
                config
                    .native_model_route()
                    .ok()?
                    .asr
                    .supports_audio_streaming(),
            )
        })
    }

    pub(crate) fn supports_ask(&self) -> bool {
        self.translation_contract()
            .is_some_and(|(profile, chat)| chat || profile == TranslationPromptProfile::Contextual)
    }

    fn translation_contract(&self) -> Option<(TranslationPromptProfile, bool)> {
        *self.translation_contract.get_or_init(|| {
            xrtranslate_config::AppConfig::from_value(self.document.clone())
                .ok()
                .and_then(|config| config.native_model_route().ok())
                .and_then(|route| {
                    Some((
                        route.translation.translation_prompt_profile().ok()?,
                        route.translation.supports_chat().ok()?,
                    ))
                })
        })
    }

    pub(crate) fn language_capabilities(
        &self,
    ) -> Result<xrtranslate_engine::language::LanguageCapabilities, String> {
        self.language_capabilities
            .get_or_init(|| {
                xrtranslate_config::AppConfig::from_value(self.document.clone())
                    .map_err(|error| error.to_string())?
                    .native_model_route()
                    .map_err(|error| error.to_string())?
                    .language_capabilities()
            })
            .clone()
    }

    pub fn runtime_requirements(&self) -> xrtranslate_config::RuntimeRequirements {
        let mut document = self.document.clone();
        let _ = Self::sync_categories(&mut document, &self.categories);
        xrtranslate_config::AppConfig::from_value(document)
            .map(|config| config.runtime_requirements())
            .unwrap_or_default()
    }

    pub fn selected_model_asset_ids(&self) -> Vec<xrtranslate_assets::ModelAssetId> {
        xrtranslate_config::AppConfig::from_value(self.document.clone())
            .map(|config| {
                config
                    .active_native_model_assets()
                    .into_iter()
                    .filter_map(|key| xrtranslate_assets::ModelAssetId::from_config_key(&key))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selected_tts_provider(&self) -> Option<&ProviderCard> {
        let category = self
            .categories
            .iter()
            .find(|category| category.key == "tts")?;
        category
            .providers
            .iter()
            .find(|provider| provider.name == category.selected_provider)
    }

    pub(crate) fn tts_supports_cloning(&self) -> bool {
        self.selected_tts_provider().is_some_and(|provider| {
            if provider_is_remote(provider) {
                provider_remote_model(provider)
                    .and_then(|model| model.tts.as_ref())
                    .is_some_and(|tts| tts.voice_cloning)
            } else {
                self.tts_is_configured()
            }
        })
    }

    pub(crate) fn tts_sample_rate(&self) -> u32 {
        if let Some(tts) = self
            .selected_tts_provider()
            .and_then(provider_remote_model)
            .and_then(|model| model.tts.as_ref())
        {
            return tts.sample_rate;
        }
        let provider = self
            .document
            .get("tts")
            .and_then(Value::as_object)
            .and_then(|section| {
                let selected = section.get("provider")?.as_str()?;
                section.get("providers")?.get(selected)
            });
        let native_rate = provider
            .and_then(|provider| provider.get("model_asset"))
            .and_then(Value::as_str)
            .and_then(xrtranslate_assets::ModelAssetId::from_config_key)
            .and_then(|id| xrtranslate_assets::manifest_for(id).audio_output)
            .map(|audio| audio.sample_rate_hz);
        native_rate.unwrap_or_else(|| {
            provider
                .and_then(|provider| provider.get("sample_rate"))
                .and_then(Value::as_u64)
                .and_then(|rate| u32::try_from(rate).ok())
                .unwrap_or(44_100)
        })
    }

    pub(crate) fn tts_is_configured(&self) -> bool {
        self.document
            .get("tts")
            .and_then(Value::as_object)
            .and_then(|section| section.get("provider"))
            .and_then(Value::as_str)
            .is_some_and(|provider| provider != "none" && !provider.trim().is_empty())
    }

    pub(crate) fn ocr_is_configured(&self) -> bool {
        !cfg!(target_os = "android")
            && self
                .document
                .get("ocr")
                .and_then(|section| section.get("provider"))
                .and_then(Value::as_str)
                .is_some_and(|provider| provider != "none" && !provider.trim().is_empty())
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn ocr_model_asset_id(&self) -> Option<xrtranslate_assets::ModelAssetId> {
        let section = self.document.get("ocr")?;
        let provider = section.get("provider")?.as_str()?;
        let key = section
            .get("providers")?
            .get(provider)?
            .get("model_asset")?
            .as_str()?;
        let id = xrtranslate_assets::ModelAssetId::from_config_key(key)?;
        let manifest = xrtranslate_assets::manifest_for(id);
        (manifest.capability == xrtranslate_assets::ModelCapability::Ocr
            && manifest.provider == provider)
            .then_some(id)
    }

    pub(crate) const fn has_unsaved_changes(&self) -> bool {
        self.dirty
    }

    pub(crate) fn onboarding_provider_state(
        &self,
        category_key: &str,
    ) -> Option<OnboardingProviderState> {
        let category = self
            .categories
            .iter()
            .find(|category| category.key == category_key)?;
        let selected = category
            .providers
            .iter()
            .find(|provider| provider.name == category.selected_provider)?;
        let field = |name: &str| {
            selected
                .fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| field.value.clone())
                .unwrap_or_default()
        };
        Some(OnboardingProviderState {
            selected: selected.name.clone(),
            remote: provider_is_remote(selected),
            choices: category
                .providers
                .iter()
                .map(|provider| OnboardingProviderChoice {
                    name: provider.name.clone(),
                    remote: provider_is_remote(provider),
                    guide_url: provider
                        .fields
                        .iter()
                        .find(|field| field.name == "guide_url")
                        .map(|field| field.value.clone())
                        .filter(|url| !url.trim().is_empty()),
                    model_asset: provider_model_asset(provider),
                    model_assets: provider_model_assets(provider),
                    supported_languages: provider_model_languages(provider),
                    voices: provider_voice_presets(provider),
                })
                .collect(),
            models: category_capability(category_key)
                .and_then(|capability| {
                    xrtranslate_assets::remote::service(&selected.name, capability)
                })
                .map(|service| service.models.clone())
                .unwrap_or_default(),
            model: field("model"),
            api_key: field("api_key"),
        })
    }

    pub(crate) fn select_onboarding_provider(&mut self, category_key: &str, provider_name: &str) {
        if let Some(category) = self
            .categories
            .iter_mut()
            .find(|category| category.key == category_key)
            && category
                .providers
                .iter()
                .any(|provider| provider.name == provider_name)
        {
            category.selected_provider = provider_name.to_owned();
            self.dirty = true;
            self.message = None;
        }
    }

    pub(crate) fn set_onboarding_model_enabled(
        &mut self,
        category_key: &str,
        provider_name: &str,
        model_asset: &str,
        enabled: bool,
    ) {
        let Some(provider) = self
            .categories
            .iter_mut()
            .find(|category| category.key == category_key)
            .and_then(|category| {
                category
                    .providers
                    .iter_mut()
                    .find(|provider| provider.name == provider_name)
            })
        else {
            return;
        };
        update_provider_model_selection(provider, model_asset, enabled);
        self.dirty = true;
        self.message = None;
    }

    pub(crate) fn set_onboarding_voice_preset(
        &mut self,
        provider_name: &str,
        language: &str,
        preset: &str,
    ) {
        let Some(provider) = self
            .categories
            .iter_mut()
            .find(|category| category.key == "tts")
            .and_then(|category| {
                category
                    .providers
                    .iter_mut()
                    .find(|provider| provider.name == provider_name)
            })
        else {
            return;
        };
        update_provider_voice_preset(provider, language, preset);
        self.dirty = true;
        self.message = None;
    }

    pub(crate) fn set_onboarding_remote_fields(
        &mut self,
        category_key: &str,
        model: String,
        api_key: String,
    ) {
        let Some(category) = self
            .categories
            .iter_mut()
            .find(|category| category.key == category_key)
        else {
            return;
        };
        let Some(provider) = category
            .providers
            .iter_mut()
            .find(|provider| provider.name == category.selected_provider)
        else {
            return;
        };
        for (name, value) in [("model", model), ("api_key", api_key)] {
            if let Some(field) = provider.fields.iter_mut().find(|field| field.name == name) {
                field.value = value;
            }
        }
        self.dirty = true;
        self.message = None;
    }

    pub(crate) fn preferred_gpu(&self) -> Option<String> {
        self.document
            .get("model_manager")
            .and_then(|m| m.get("preferred_gpu"))
            .and_then(|g| g.as_str())
            .map(|s| s.to_string())
    }

    pub(crate) fn set_preferred_gpu(&mut self, gpu_name: Option<&str>) {
        if let Some(root) = self.document.as_object_mut() {
            let model_manager = root
                .entry("model_manager")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                .as_object_mut();
            if let Some(model_manager) = model_manager {
                match gpu_name {
                    Some(name) if !name.trim().is_empty() => {
                        model_manager.insert(
                            "preferred_gpu".into(),
                            serde_json::Value::String(name.trim().to_string()),
                        );
                    }
                    _ => {
                        model_manager.remove("preferred_gpu");
                    }
                }
                self.dirty = true;
                self.message = None;
            }
        }
    }

    pub(crate) fn save_onboarding_configuration(
        &mut self,
    ) -> Result<OnboardingSaveOutcome, String> {
        let incomplete = self.has_incomplete_remote_provider();
        let result = if incomplete {
            self.save_draft()
        } else {
            self.save()
        };
        match result {
            Ok(()) => {
                self.message = None;
                if incomplete {
                    Ok(OnboardingSaveOutcome::IncompleteRemoteProvider)
                } else {
                    Ok(OnboardingSaveOutcome::Saved {
                        resolved_error: self.onboarding_save_error.take(),
                    })
                }
            }
            Err(error) => {
                self.message = Some(error.clone());
                self.onboarding_save_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub(crate) fn onboarding_message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    fn has_incomplete_remote_provider(&self) -> bool {
        self.categories
            .iter()
            .filter(|category| matches!(category.key, "asr" | "translation" | "tts"))
            .filter_map(|category| {
                category
                    .providers
                    .iter()
                    .find(|provider| provider.name == category.selected_provider)
            })
            .any(|provider| {
                provider_is_remote(provider)
                    && ["model", "api_key"].iter().any(|required| {
                        provider
                            .fields
                            .iter()
                            .find(|field| field.name == *required)
                            .is_none_or(|field| field.value.trim().is_empty())
                    })
            })
    }

    fn make_category(document: &Value, key: &'static str) -> ServiceCategory {
        let section = document.get(key).and_then(Value::as_object);
        let mut providers: Vec<ProviderCard> = section
            .and_then(|section| section.get("providers"))
            .and_then(Value::as_object)
            .map(|providers| {
                providers
                    .iter()
                    .filter(|(name, config)| {
                        config.get("connector").is_none()
                            && !config.get("transport").and_then(Value::as_str).is_some_and(
                                |transport| {
                                    matches!(
                                        transport,
                                        "openai" | "dashscope" | "websocket" | "gemini"
                                    )
                                },
                            )
                            || category_capability(key).is_some_and(|capability| {
                                xrtranslate_assets::remote::service(name, capability).is_some()
                            })
                    })
                    .map(|(name, config)| ProviderCard {
                        name: name.clone(),
                        fields: config
                            .as_object()
                            .map(|config| {
                                config
                                    .iter()
                                    .map(|(name, value)| ConfigField {
                                        name: name.clone(),
                                        value: display_value(value),
                                        kind: field_kind(value),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        sort_providers(key, &mut providers);

        let selected_provider = section
            .and_then(|section| section.get("provider"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| providers.first().map(|provider| provider.name.clone()))
            .unwrap_or_default();

        ServiceCategory {
            key,
            selected_provider,
            providers,
        }
    }

    fn save(&mut self) -> Result<(), String> {
        self.language_capabilities.take();
        self.translation_contract.take();
        self.asr_streaming.take();
        Self::sync_categories(&mut self.document, &self.categories)?;
        let parsed = xrtranslate_config::AppConfig::from_value(self.document.clone())
            .map_err(|error| format!("Invalid configuration: {error}"))?;
        let route = parsed
            .native_model_route()
            .map_err(|error| format!("Invalid model settings: {error}"))?;
        validate_native_provider_asset(&route.asr, xrtranslate_assets::ModelCapability::Asr)?;
        validate_native_provider_asset(
            &route.translation,
            xrtranslate_assets::ModelCapability::Translation,
        )?;
        parsed.remote_tts()?;
        validate_tts_provider_asset(&parsed.tts)?;
        #[cfg(not(target_os = "android"))]
        validate_ocr_provider_asset(&parsed.ocr)?;
        self.save_draft()
    }

    fn save_draft(&mut self) -> Result<(), String> {
        self.language_capabilities.take();
        self.translation_contract.take();
        self.asr_streaming.take();
        Self::sync_categories(&mut self.document, &self.categories)?;
        xrtranslate_config::save_user_config_document(&self.path, &project_root(), &self.document)?;
        self.dirty = false;
        Ok(())
    }

    fn sync_categories(document: &mut Value, categories: &[ServiceCategory]) -> Result<(), String> {
        let root = document
            .as_object_mut()
            .ok_or("config.json root must be an object")?;
        for category in categories {
            let section = root
                .get_mut(category.key)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("Missing {} section", category.key))?;
            section.insert(
                "provider".into(),
                Value::String(category.selected_provider.clone()),
            );
            let providers = section
                .get_mut("providers")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("Missing {}.providers section", category.key))?;
            for provider in &category.providers {
                let config = providers
                    .get_mut(&provider.name)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| format!("Missing provider {}", provider.name))?;
                for field in &provider.fields {
                    config.insert(field.name.clone(), parse_value(&field.value, field.kind)?);
                }
            }
        }
        Ok(())
    }
}

fn category_capability(category: &str) -> Option<xrtranslate_assets::ModelCapability> {
    match category {
        "asr" => Some(xrtranslate_assets::ModelCapability::Asr),
        "translation" => Some(xrtranslate_assets::ModelCapability::Translation),
        "tts" => Some(xrtranslate_assets::ModelCapability::Tts),
        "ocr" => Some(xrtranslate_assets::ModelCapability::Ocr),
        _ => None,
    }
}

fn provider_sort_rank(category: &str, provider_name: &str) -> usize {
    if provider_name == "none" {
        return 0;
    }
    let Some(capability) = category_capability(category) else {
        return 100;
    };
    if let Some(pos) = xrtranslate_assets::manifests_for_capability(capability)
        .position(|manifest| manifest.provider == provider_name)
    {
        return 1 + pos;
    }
    100
}

fn sort_providers(category: &str, providers: &mut [ProviderCard]) {
    providers.sort_by(|a, b| {
        provider_sort_rank(category, &a.name)
            .cmp(&provider_sort_rank(category, &b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub fn provider_display_label(name: &str, language: crate::i18n::UiLanguage) -> String {
    match name {
        "qwen" => crate::i18n::tr(language, "qwen (China Mainland)").to_owned(),
        "qwen-intl" => crate::i18n::tr(language, "qwen (International)").to_owned(),
        other => other.to_owned(),
    }
}

fn validate_native_provider_asset(
    provider: &xrtranslate_config::NativeProviderConfig,
    capability: xrtranslate_assets::ModelCapability,
) -> Result<(), String> {
    if !provider.uses_local_runtime() {
        return Ok(());
    }
    let manifest = if let Some(key) = provider.model_asset.as_deref() {
        let id = xrtranslate_assets::ModelAssetId::from_config_key(key).ok_or_else(|| {
            format!(
                "Unknown model package {key} for provider {}.",
                provider.provider
            )
        })?;
        xrtranslate_assets::manifest_for(id)
    } else {
        xrtranslate_assets::tier_default_manifest(
            &provider.provider,
            capability,
            xrtranslate_assets::ModelLevel::Normal,
        )
        .ok_or_else(|| {
            format!(
                "Provider {} has no default local model package.",
                provider.provider
            )
        })?
    };
    if manifest.provider != provider.provider || manifest.capability != capability {
        return Err(format!(
            "Model package {} does not belong to provider {} for {capability:?}.",
            manifest.id, provider.provider
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn validate_ocr_provider_asset(ocr: &xrtranslate_config::OcrConfig) -> Result<(), String> {
    if ocr.provider.trim().is_empty() || ocr.provider == "none" {
        return Ok(());
    }
    let manifest = ocr
        .provider_config(&ocr.provider)
        .and_then(|provider| provider.get("model_asset"))
        .and_then(Value::as_str)
        .and_then(xrtranslate_assets::ModelAssetId::from_config_key)
        .map(xrtranslate_assets::manifest_for)
        .ok_or_else(|| "Select an OCR model package.".to_owned())?;
    if manifest.capability != xrtranslate_assets::ModelCapability::Ocr
        || manifest.provider != ocr.provider
    {
        return Err("The selected OCR model does not belong to this provider.".to_owned());
    }
    Ok(())
}

fn validate_tts_provider_asset(tts: &xrtranslate_config::TtsConfig) -> Result<(), String> {
    let provider = tts.provider.trim();
    if provider.is_empty() || provider.eq_ignore_ascii_case("none") {
        return Ok(());
    }
    let values = tts
        .provider_config(provider)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("tts.providers.{provider} must be an object"))?;
    let keys: Vec<&str> = values
        .get("model_assets")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_else(|| {
            values
                .get("model_asset")
                .and_then(serde_json::Value::as_str)
                .into_iter()
                .collect()
        });
    let mut selected_manifests = Vec::new();
    let mut claimed_languages = std::collections::BTreeMap::<&str, &str>::new();
    for key in keys {
        let id = xrtranslate_assets::ModelAssetId::from_config_key(key)
            .ok_or_else(|| format!("Unknown model asset {key}"))?;
        let manifest = xrtranslate_assets::manifest_for(id);
        if manifest.provider != provider
            || manifest.capability != xrtranslate_assets::ModelCapability::Tts
        {
            return Err(format!(
                "Model asset {key} does not belong to provider {provider} for TTS"
            ));
        }
        for language in manifest.languages {
            if let Some(existing) = claimed_languages.insert(language, key) {
                return Err(format!(
                    "TTS model assets {existing} and {key} both claim language {language}; select one model variant per language."
                ));
            }
        }
        selected_manifests.push(manifest);
    }
    if let Some(voices) = values.get("voices").and_then(serde_json::Value::as_object) {
        for (language, value) in voices {
            let key = value.as_str().ok_or_else(|| {
                format!("tts.providers.{provider}.voices.{language} must be a string")
            })?;
            let valid = selected_manifests.iter().any(|manifest| {
                manifest
                    .voice_presets
                    .iter()
                    .any(|preset| preset.language == language && preset.key == key)
            });
            if !valid {
                return Err(format!(
                    "Voice preset {key:?} is not provided by the selected {provider} model for language {language}."
                ));
            }
        }
    }
    Ok(())
}

fn provider_model_asset(provider: &ProviderCard) -> Option<String> {
    provider
        .fields
        .iter()
        .find(|field| field.name == "model_asset")
        .map(|field| field.value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn provider_model_assets(provider: &ProviderCard) -> Vec<String> {
    provider
        .fields
        .iter()
        .find(|field| field.name == "model_assets")
        .and_then(|field| serde_json::from_str::<Vec<String>>(&field.value).ok())
        .filter(|assets| !assets.is_empty())
        .unwrap_or_else(|| provider_model_asset(provider).into_iter().collect())
}

fn update_provider_model_selection(provider: &mut ProviderCard, model_asset: &str, enabled: bool) {
    let mut assets = provider_model_assets(provider);
    if enabled {
        if let Some(next_id) = xrtranslate_assets::ModelAssetId::from_config_key(model_asset) {
            let next = xrtranslate_assets::manifest_for(next_id);
            assets.retain(|asset| {
                let Some(existing_id) = xrtranslate_assets::ModelAssetId::from_config_key(asset)
                else {
                    return false;
                };
                let existing = xrtranslate_assets::manifest_for(existing_id);
                existing
                    .languages
                    .iter()
                    .all(|language| !next.languages.iter().any(|candidate| candidate == language))
            });
        }
        if !assets.iter().any(|asset| asset == model_asset) {
            assets.push(model_asset.to_owned());
        }
    } else {
        assets.retain(|asset| asset != model_asset);
    }
    let encoded = serde_json::to_string(&assets).expect("string lists serialize");
    if let Some(field) = provider
        .fields
        .iter_mut()
        .find(|field| field.name == "model_assets")
    {
        field.value = encoded;
        field.kind = JsonFieldKind::Json;
    } else {
        provider.fields.push(ConfigField {
            name: "model_assets".to_owned(),
            value: encoded,
            kind: JsonFieldKind::Json,
        });
    }
    // Preserve the singular key as a compatibility alias for older builds.
    if let Some(first) = assets.first()
        && let Some(field) = provider
            .fields
            .iter_mut()
            .find(|field| field.name == "model_asset")
    {
        field.value = first.clone();
    }

    let selected = assets
        .iter()
        .filter_map(|asset| xrtranslate_assets::ModelAssetId::from_config_key(asset))
        .map(xrtranslate_assets::manifest_for)
        .collect::<Vec<_>>();
    let mut voices = provider_voice_presets(provider);
    voices.retain(|language, preset| {
        selected.iter().any(|manifest| {
            manifest
                .voice_presets
                .iter()
                .any(|candidate| candidate.language == language && candidate.key == preset)
        })
    });
    for manifest in &selected {
        for preset in manifest
            .voice_presets
            .iter()
            .filter(|preset| preset.is_default)
        {
            voices
                .entry(preset.language.to_owned())
                .or_insert_with(|| preset.key.to_owned());
        }
    }
    if !voices.is_empty() || provider.fields.iter().any(|field| field.name == "voices") {
        let encoded = serde_json::to_string(&voices).expect("voice maps serialize");
        if let Some(field) = provider
            .fields
            .iter_mut()
            .find(|field| field.name == "voices")
        {
            field.value = encoded;
            field.kind = JsonFieldKind::Json;
        } else {
            provider.fields.push(ConfigField {
                name: "voices".to_owned(),
                value: encoded,
                kind: JsonFieldKind::Json,
            });
        }
    }
}

fn provider_voice_presets(provider: &ProviderCard) -> std::collections::BTreeMap<String, String> {
    provider
        .fields
        .iter()
        .find(|field| field.name == "voices")
        .and_then(|field| {
            serde_json::from_str::<std::collections::BTreeMap<String, String>>(&field.value).ok()
        })
        .unwrap_or_default()
}

fn update_provider_voice_preset(provider: &mut ProviderCard, language: &str, preset: &str) {
    let mut voices = provider_voice_presets(provider);
    voices.insert(language.to_owned(), preset.to_owned());
    let encoded = serde_json::to_string(&voices).expect("voice maps serialize");
    if let Some(field) = provider
        .fields
        .iter_mut()
        .find(|field| field.name == "voices")
    {
        field.value = encoded;
        field.kind = JsonFieldKind::Json;
    } else {
        provider.fields.push(ConfigField {
            name: "voices".to_owned(),
            value: encoded,
            kind: JsonFieldKind::Json,
        });
    }
}

fn provider_is_remote(provider: &ProviderCard) -> bool {
    provider
        .fields
        .iter()
        .find(|field| field.name == "transport")
        .is_some_and(|field| {
            matches!(
                field.value.trim().to_ascii_lowercase().as_str(),
                "openai" | "dashscope" | "websocket" | "gemini"
            )
        })
}

fn provider_supported_languages(provider: &ProviderCard) -> Vec<String> {
    provider
        .fields
        .iter()
        .find(|field| field.name == "supported_languages")
        .and_then(|field| serde_json::from_str::<Vec<String>>(&field.value).ok())
        .unwrap_or_default()
}

fn provider_remote_model(
    provider: &ProviderCard,
) -> Option<&'static xrtranslate_assets::remote::RemoteModelManifest> {
    let field = |name| {
        provider
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.trim())
    };
    use xrtranslate_assets::remote::{RemoteConnector, remote_model};
    let connector = field("connector")
        .and_then(|value| serde_json::from_value(Value::from(value)).ok())
        .or_else(|| {
            RemoteConnector::from_transport(
                "asr",
                field("transport")?,
                field("url").unwrap_or_default(),
            )
        })?;
    remote_model(connector, field("model")?)
}

fn provider_model_languages(provider: &ProviderCard) -> Vec<String> {
    if let Some(card) = provider_remote_model(provider) {
        return card.languages.clone();
    }
    let mut languages = provider_model_assets(provider)
        .into_iter()
        .filter_map(|key| xrtranslate_assets::ModelAssetId::from_config_key(&key))
        .flat_map(|id| {
            xrtranslate_assets::manifest_for(id)
                .languages
                .iter()
                .copied()
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if languages.is_empty() {
        languages = provider_supported_languages(provider);
    }
    languages.sort();
    languages.dedup();
    languages
}

fn project_config_path() -> PathBuf {
    for start in [std::env::current_dir().ok(), std::env::current_exe().ok()] {
        let Some(start) = start else {
            continue;
        };
        let directory = if start.is_dir() {
            start
        } else {
            start.parent().map(PathBuf::from).unwrap_or(start)
        };
        for ancestor in directory.ancestors() {
            let candidate = ancestor.join("config.json");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("config.json")
}

fn project_root() -> PathBuf {
    project_config_path()
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn field_kind(value: &Value) -> JsonFieldKind {
    match value {
        Value::String(_) => JsonFieldKind::String,
        Value::Bool(_) => JsonFieldKind::Bool,
        Value::Number(_) => JsonFieldKind::Number,
        _ => JsonFieldKind::Json,
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn parse_value(value: &str, kind: JsonFieldKind) -> Result<Value, String> {
    match kind {
        JsonFieldKind::String => Ok(Value::String(value.into())),
        JsonFieldKind::Bool => value
            .trim()
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| format!("{value:?} must be true or false")),
        JsonFieldKind::Number => serde_json::from_str::<Value>(value.trim())
            .ok()
            .filter(Value::is_number)
            .ok_or_else(|| format!("{value:?} must be a JSON number")),
        JsonFieldKind::Json => serde_json::from_str(value.trim())
            .map_err(|error| format!("Invalid JSON value {value:?}: {error}")),
    }
}
