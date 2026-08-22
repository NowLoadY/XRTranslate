use serde_json::{Map, Value};
use std::path::PathBuf;
use xrtranslate_prompt::PromptProviderTarget;

use crate::ui::components;
use provider_schema::{ProviderFieldEditor, provider_field_descriptor};

mod provider_schema;

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
    title: &'static str,
    selected_provider: String,
    providers: Vec<ProviderCard>,
    show_all: bool,
}

#[derive(Clone)]
pub(crate) struct OnboardingProviderState {
    pub selected: String,
    pub remote: bool,
    pub choices: Vec<OnboardingProviderChoice>,
    pub model: String,
    pub api_key: String,
}

#[derive(Clone)]
pub(crate) struct OnboardingProviderChoice {
    pub name: String,
    pub remote: bool,
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
}

impl ServiceConfigEditor {
    pub fn load() -> Self {
        let path = project_config_path();
        let mut editor = Self {
            path,
            document: Value::Object(Map::new()),
            categories: Vec::new(),
            dirty: false,
            message: None,
            onboarding_save_error: None,
        };
        if let Err(error) = editor.reload() {
            editor.message = Some(error);
        }
        editor
    }

    pub fn reload(&mut self) -> Result<(), String> {
        self.document = xrtranslate_config::load_user_config_document(&self.path, &project_root())
            .map_err(|error| format!("Cannot read {}: {error}", self.path.display()))?;
        self.categories = [
            ("asr", "ASR / Speech Recognition"),
            ("translation", "Translation"),
            ("tts", "Text to Speech"),
        ]
        .into_iter()
        .map(|(key, title)| Self::make_category(&self.document, key, title))
        .collect();
        self.dirty = false;
        self.message = None;
        self.onboarding_save_error = None;
        Ok(())
    }

    pub fn translation_prompt_target(&self) -> PromptProviderTarget {
        let Some(category) = self
            .categories
            .iter()
            .find(|category| category.key == "translation")
        else {
            return PromptProviderTarget::Hunyuan;
        };
        let transport = category
            .providers
            .iter()
            .find(|provider| provider.name == category.selected_provider)
            .and_then(|provider| {
                provider
                    .fields
                    .iter()
                    .find(|field| field.name == "transport")
            })
            .map(|field| field.value.as_str())
            .unwrap_or("local");
        prompt_target_for_translation_provider(&category.selected_provider, transport)
    }

    pub fn runtime_requirements(&self) -> xrtranslate_config::RuntimeRequirements {
        let mut document = self.document.clone();
        let _ = Self::sync_categories(&mut document, &self.categories);
        xrtranslate_config::AppConfig::from_value(document)
            .map(|config| config.runtime_requirements())
            .unwrap_or_default()
    }

    pub(crate) fn tts_sample_rate(&self) -> u32 {
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
                    model_asset: provider_model_asset(provider),
                    model_assets: provider_model_assets(provider),
                    supported_languages: provider_model_languages(provider),
                    voices: provider_voice_presets(provider),
                })
                .collect(),
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

    pub(crate) fn save_onboarding_configuration(
        &mut self,
    ) -> Result<OnboardingSaveOutcome, String> {
        if self.has_incomplete_remote_provider() {
            self.message = None;
            return Ok(OnboardingSaveOutcome::IncompleteRemoteProvider);
        }

        match self.save() {
            Ok(()) => {
                self.message = None;
                Ok(OnboardingSaveOutcome::Saved {
                    resolved_error: self.onboarding_save_error.take(),
                })
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
            .filter(|category| matches!(category.key, "asr" | "translation"))
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

    fn make_category(document: &Value, key: &'static str, title: &'static str) -> ServiceCategory {
        let section = document.get(key).and_then(Value::as_object);
        let mut providers: Vec<ProviderCard> = section
            .and_then(|section| section.get("providers"))
            .and_then(Value::as_object)
            .map(|providers| {
                providers
                    .iter()
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
            title,
            selected_provider,
            providers,
            show_all: false,
        }
    }

    pub fn render(
        &mut self,
        ui: &mut eframe::egui::Ui,
        backend: &mut crate::backend::BackendManager,
        model_tasks: &mut crate::model_install::NativeModelTaskManager,
        runtime_installer: &mut crate::runtime_install::RuntimeInstaller,
        live_tts_backend: Option<&str>,
        live_tts_cuda_version: Option<&str>,
        project_root: &std::path::Path,
        language: crate::i18n::UiLanguage,
    ) -> (bool, bool) {
        use crate::ui::components::{self, section};
        use eframe::egui;

        ui.label(
            egui::RichText::new(crate::i18n::tr(language, "Service Providers"))
                .size(22.0)
                .color(crate::ui::theme::text_strong())
                .strong(),
        );
        ui.add_space(14.0);

        if model_tasks.needs_discovery()
            && let Err(error) = model_tasks.discover_existing(project_root.to_path_buf())
        {
            self.message = Some(error);
        }

        let runtime_requirements = self.runtime_requirements();
        let local_model_availability = runtime_installer.local_model_availability();
        let mut apply_runtime_config = false;
        let mut delete_runtime_requested = false;
        let mut runtime_action = crate::ui::RuntimeUiAction::None;
        for cat_idx in 0..self.categories.len() {
            let category_title = crate::i18n::tr(language, self.categories[cat_idx].title);
            let category_key = self.categories[cat_idx].key;

            section(ui, category_title, |ui| {
                // Row 1: Active Provider selector & View All toggle
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::i18n::tr(language, "Provider:")).strong());
                    let previous = self.categories[cat_idx].selected_provider.clone();
                    let selected_label = if self.categories[cat_idx].selected_provider.is_empty() {
                        crate::i18n::tr(language, "No providers configured")
                    } else {
                        &self.categories[cat_idx].selected_provider
                    };

                    let provider_names: Vec<String> = self.categories[cat_idx]
                        .providers
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();

                    let combo_resp = egui::ComboBox::from_id_salt((category_key, "provider_combo"))
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for name in &provider_names {
                                ui.selectable_value(
                                    &mut self.categories[cat_idx].selected_provider,
                                    name.clone(),
                                    name,
                                );
                            }
                        });

                    if self.categories[cat_idx].selected_provider != previous {
                        self.dirty = true;
                    }

                    if combo_resp.response.changed() {
                        self.dirty = true;
                    }

                    ui.add_space(16.0);
                    ui.checkbox(
                        &mut self.categories[cat_idx].show_all,
                        crate::i18n::tr(language, "All providers"),
                    );
                    if ui
                        .button(crate::i18n::tr(language, "Add online API"))
                        .clicked()
                    {
                        if let Err(error) = self.add_remote_provider(category_key) {
                            self.message = Some(error);
                        } else {
                            self.dirty = true;
                        }
                    }
                });

                ui.add_space(12.0);

                if self.categories[cat_idx].providers.is_empty() {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr(language, "No providers"))
                            .color(crate::ui::theme::text_weak()),
                    );
                    return;
                }

                let show_all = self.categories[cat_idx].show_all;
                let active_name = self.categories[cat_idx].selected_provider.clone();

                if show_all {
                    // Render Grid for ALL Providers
                    for provider_idx in 0..self.categories[cat_idx].providers.len() {
                        let provider_name = self.categories[cat_idx].providers[provider_idx]
                            .name
                            .clone();
                        let is_active = provider_name == active_name;
                        let model_assets = provider_model_assets(
                            &self.categories[cat_idx].providers[provider_idx],
                        );
                        let remote =
                            provider_is_remote(&self.categories[cat_idx].providers[provider_idx]);
                        let supported_languages = provider_model_languages(
                            &self.categories[cat_idx].providers[provider_idx],
                        );

                        ui.push_id(&provider_name, |ui| {
                            egui::Frame::new()
                                .fill(if is_active {
                                    egui::Color32::from_rgb(240, 246, 255)
                                } else {
                                    egui::Color32::from_gray(250)
                                })
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(12))
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if is_active {
                                        egui::Color32::from_rgb(59, 130, 246)
                                    } else {
                                        egui::Color32::from_gray(225)
                                    },
                                ))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&provider_name)
                                                .strong()
                                                .size(14.0)
                                                .color(crate::ui::theme::text_strong()),
                                        );
                                        if is_active {
                                            ui.label(
                                                egui::RichText::new(crate::i18n::tr(
                                                    language, "(Active)",
                                                ))
                                                .color(egui::Color32::from_rgb(37, 99, 235))
                                                .size(12.0)
                                                .strong(),
                                            );
                                        }
                                        if let Some(message) = render_provider_model_action(
                                            ui,
                                            backend,
                                            model_tasks,
                                            ProviderModelAction {
                                                project_root,
                                                language,
                                                category_key,
                                                provider_name: &provider_name,
                                                model_assets: &model_assets,
                                                remote,
                                                local_models_available: matches!(
                                                    local_model_availability,
                                                    crate::runtime_install::LocalModelAvailability::Available { .. }
                                                ),
                                            },
                                        ) {
                                            self.message = Some(message);
                                        }
                                    });

                                    ui.add_space(8.0);

                                    render_provider_capabilities(
                                        ui,
                                        language,
                                        category_key,
                                        &supported_languages,
                                    );
                                    if category_key == "tts"
                                        && render_tts_model_selection(
                                            ui,
                                            &mut self.categories[cat_idx].providers[provider_idx],
                                            language,
                                            &local_model_availability,
                                        )
                                    {
                                        self.dirty = true;
                                    }

                                    let fields_len = self.categories[cat_idx].providers
                                        [provider_idx]
                                        .fields
                                        .iter()
                                        .filter(|field| {
                                            provider_field_is_visible(
                                                field,
                                                category_key,
                                                &provider_name,
                                                !model_assets.is_empty(),
                                            )
                                        })
                                        .count();
                                    if fields_len == 0 {
                                        ui.label(
                                            egui::RichText::new(crate::i18n::tr(
                                                language,
                                                "No parameters",
                                            ))
                                            .color(crate::ui::theme::text_weak())
                                            .size(12.0),
                                        );
                                    } else {
                                        egui::Grid::new((category_key, &provider_name, "all_grid"))
                                            .num_columns(2)
                                            .spacing([16.0, 8.0])
                                            .min_col_width(130.0)
                                            .show(ui, |ui| {
                                                for field in &mut self.categories[cat_idx].providers
                                                    [provider_idx]
                                                    .fields
                                                {
                                                    if !provider_field_is_visible(
                                                        field,
                                                        category_key,
                                                        &provider_name,
                                                        !model_assets.is_empty(),
                                                    ) {
                                                        continue;
                                                    }
                                                    let label =
                                                        provider_field_label(language, &field.name);
                                                    let label_response = ui.label(
                                                        egui::RichText::new(label)
                                                            .color(crate::ui::theme::text_normal()),
                                                    );
                                                    if let Some(help) =
                                                        provider_field_help(language, &field.name)
                                                    {
                                                        label_response.on_hover_text(help);
                                                    }
                                                    let edit_w =
                                                        (ui.available_width() - 20.0).max(200.0);
                                                    if render_field_input(
                                                        ui,
                                                        field,
                                                        edit_w,
                                                        language,
                                                        category_key,
                                                        &provider_name,
                                                    ) {
                                                        self.dirty = true;
                                                    }
                                                    ui.end_row();
                                                }
                                            });
                                    }
                                });
                        });
                        ui.add_space(10.0);
                    }
                } else {
                    // Render Form Grid for the ACTIVE Provider Only
                    let active_idx = self.categories[cat_idx]
                        .providers
                        .iter()
                        .position(|p| p.name == active_name);

                    if let Some(idx) = active_idx {
                        let provider_name = self.categories[cat_idx].providers[idx].name.clone();
                        let model_assets =
                            provider_model_assets(&self.categories[cat_idx].providers[idx]);
                        let remote = provider_is_remote(&self.categories[cat_idx].providers[idx]);
                        let supported_languages =
                            provider_model_languages(&self.categories[cat_idx].providers[idx]);

                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(&provider_name)
                                    .size(13.5)
                                    .color(crate::ui::theme::text_strong())
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            if let Some(message) = render_provider_model_action(
                                ui,
                                backend,
                                model_tasks,
                                ProviderModelAction {
                                    project_root,
                                    language,
                                    category_key,
                                    provider_name: &provider_name,
                                    model_assets: &model_assets,
                                    remote,
                                    local_models_available: matches!(
                                        local_model_availability,
                                        crate::runtime_install::LocalModelAvailability::Available { .. }
                                    ) && !runtime_installer.is_busy(),
                                },
                            ) {
                                self.message = Some(message);
                            }
                        });
                        ui.add_space(10.0);
                        render_provider_capabilities(
                            ui,
                            language,
                            category_key,
                            &supported_languages,
                        );
                        if category_key == "tts"
                            && render_tts_model_selection(
                                ui,
                                &mut self.categories[cat_idx].providers[idx],
                                language,
                                &local_model_availability,
                            )
                        {
                            self.dirty = true;
                        }

                        let fields_len = self.categories[cat_idx].providers[idx]
                            .fields
                            .iter()
                            .filter(|field| {
                                provider_field_is_visible(
                                    field,
                                    category_key,
                                    &provider_name,
                                    !model_assets.is_empty(),
                                )
                            })
                            .count();
                        if fields_len == 0 {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(language, "No parameters"))
                                    .color(crate::ui::theme::text_weak()),
                            );
                        } else {
                            egui::Grid::new((category_key, &provider_name, "active_grid"))
                                .num_columns(2)
                                .spacing([20.0, 10.0])
                                .min_col_width(140.0)
                                .show(ui, |ui| {
                                    for field in &mut self.categories[cat_idx].providers[idx].fields
                                    {
                                        if !provider_field_is_visible(
                                            field,
                                            category_key,
                                            &provider_name,
                                            !model_assets.is_empty(),
                                        ) {
                                            continue;
                                        }
                                        let label = provider_field_label(language, &field.name);
                                        let label_response = ui.label(
                                            egui::RichText::new(label)
                                                .strong()
                                                .color(crate::ui::theme::text_strong()),
                                        );
                                        if let Some(help) =
                                            provider_field_help(language, &field.name)
                                        {
                                            label_response.on_hover_text(help);
                                        }
                                        let edit_w =
                                            (ui.available_width() - 20.0).clamp(240.0, 360.0);
                                        if render_field_input(
                                            ui,
                                            field,
                                            edit_w,
                                            language,
                                            category_key,
                                            &provider_name,
                                        ) {
                                            self.dirty = true;
                                        }
                                        ui.end_row();
                                    }
                                });
                        }
                    }
                }
                if category_key == "tts" && runtime_requirements.onnx_tts && !self.dirty {
                    ui.add_space(12.0);
                    let can_delete_runtime =
                        runtime_installer.managed_resources_are_present(project_root);
                    runtime_action = crate::ui::render_tts_runtime_status(
                        ui,
                        language,
                        runtime_installer,
                        runtime_requirements,
                        can_delete_runtime,
                        live_tts_backend,
                        live_tts_cuda_version,
                    );
                }
            });
            ui.add_space(12.0);
        }

        match runtime_action {
            crate::ui::RuntimeUiAction::None => {}
            crate::ui::RuntimeUiAction::Install => {
                if model_tasks.is_busy() {
                    self.message = Some(
                        "Wait for the queued model downloads before installing the runtime.".into(),
                    );
                } else if let Err(error) =
                    runtime_installer.install_recommended(project_root.to_path_buf())
                {
                    self.message = Some(error);
                }
            }
            crate::ui::RuntimeUiAction::Retry => {
                if let Err(error) =
                    runtime_installer.prepare_for(project_root.to_path_buf(), runtime_requirements)
                {
                    self.message = Some(error);
                }
            }
            crate::ui::RuntimeUiAction::SwitchSource(use_mirror) => {
                if let Err(error) =
                    runtime_installer.switch_download_source(project_root.to_path_buf(), use_mirror)
                {
                    self.message = Some(error);
                }
            }
            crate::ui::RuntimeUiAction::DeleteResources => {
                delete_runtime_requested = true;
            }
        }

        // Action Toolbar
        ui.horizontal(|ui| {
            let save_label = if self.dirty {
                crate::i18n::tr(language, "Save *")
            } else {
                crate::i18n::tr(language, "Save")
            };
            let save = components::primary_button(ui, save_label);
            if save.clicked() {
                match self.save() {
                    Ok(()) => {
                        apply_runtime_config = true;
                        self.message = Some(
                            crate::i18n::tr(language, "Saved. Applying model settings.").to_owned(),
                        )
                    }
                    Err(error) => self.message = Some(error),
                }
            }
            if components::animated_button(ui, crate::i18n::tr(language, "Reload")).clicked()
                && let Err(error) = self.reload()
            {
                self.message = Some(error);
            }
            if self.dirty {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(language, "Unsaved"))
                        .color(egui::Color32::from_rgb(217, 119, 6))
                        .strong(),
                );
            }
        });
        if let Some(message) = &self.message {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(message)
                    .color(crate::ui::theme::text_weak())
                    .size(12.0),
            );
        }
        (apply_runtime_config, delete_runtime_requested)
    }

    fn save(&mut self) -> Result<(), String> {
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
        validate_tts_provider_asset(&parsed.tts)?;
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

    fn add_remote_provider(&mut self, category_key: &str) -> Result<(), String> {
        let category_index = self
            .categories
            .iter()
            .position(|category| category.key == category_key)
            .ok_or_else(|| format!("Unknown provider category {category_key}"))?;
        let providers = self
            .document
            .get_mut(category_key)
            .and_then(Value::as_object_mut)
            .and_then(|section| section.get_mut("providers"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("Missing {category_key}.providers section"))?;
        let base = "openai-custom";
        let mut name = base.to_owned();
        let mut index = 2;
        while providers.contains_key(&name) {
            name = format!("{base}-{index}");
            index += 1;
        }
        providers.insert(
            name.clone(),
            serde_json::json!({
                "transport": "openai",
                "url": "https://api.openai.com/v1/chat/completions",
                "api_key": "",
                "model": if category_key == "asr" { "gpt-4o-transcribe" } else { "gpt-4o-mini" },
                "context_window_tokens": 8192,
                "max_tokens": if category_key == "asr" { 256 } else { 512 },
                "parallel_slots": 2,
                "asr_prompt_mode": if category_key == "asr" { "instruction" } else { "none" },
                "supports_prompt_context": true
            }),
        );
        let category = &mut self.categories[category_index];
        category.providers.push(ProviderCard {
            name: name.clone(),
            fields: providers
                .get(&name)
                .and_then(Value::as_object)
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
        });
        sort_providers(&category.key, &mut category.providers);
        category.selected_provider = name;
        Ok(())
    }
}

fn category_capability(category: &str) -> Option<xrtranslate_assets::ModelCapability> {
    match category {
        "asr" => Some(xrtranslate_assets::ModelCapability::Asr),
        "translation" => Some(xrtranslate_assets::ModelCapability::Translation),
        "tts" => Some(xrtranslate_assets::ModelCapability::Tts),
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

fn prompt_target_for_translation_provider(provider: &str, transport: &str) -> PromptProviderTarget {
    if provider.trim() == "hunyuan" && transport.trim() != "openai" {
        PromptProviderTarget::Hunyuan
    } else {
        PromptProviderTarget::OpenAiCompatible
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
        xrtranslate_assets::manifests_for_capability(capability)
            .find(|manifest| {
                manifest.provider == provider.provider
                    && manifest.level == xrtranslate_assets::ModelLevel::Normal
            })
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
                "openai" | "websocket"
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

fn provider_model_languages(provider: &ProviderCard) -> Vec<String> {
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

fn render_provider_capabilities(
    ui: &mut eframe::egui::Ui,
    language: crate::i18n::UiLanguage,
    category_key: &str,
    supported_languages: &[String],
) {
    if category_key != "tts" || supported_languages.is_empty() {
        return;
    }
    ui.label(
        eframe::egui::RichText::new(format!(
            "{} {}",
            crate::i18n::tr(language, "Supported synthesis languages:"),
            supported_languages.join(", ")
        ))
        .color(crate::ui::theme::text_weak())
        .size(12.0),
    );
    ui.add_space(8.0);
}

fn render_tts_model_selection(
    ui: &mut eframe::egui::Ui,
    provider: &mut ProviderCard,
    language: crate::i18n::UiLanguage,
    availability: &crate::runtime_install::LocalModelAvailability,
) -> bool {
    let packages = crate::model_install::model_packages_for_provider(
        &provider.name,
        xrtranslate_assets::ModelCapability::Tts,
    );
    if packages.is_empty() {
        return false;
    }
    let selected = provider_model_assets(provider);
    let mut change = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            eframe::egui::RichText::new(crate::i18n::tr(language, "Models:"))
                .size(12.0)
                .color(crate::ui::theme::text_weak()),
        );
        for package in packages {
            let checked = selected.iter().any(|asset| asset == package.id.as_str());
            let available = matches!(
                (availability, package.hardware.accelerator),
                (
                    crate::runtime_install::LocalModelAvailability::Available {
                        memory_bytes,
                        ..
                    },
                    xrtranslate_assets::ModelAccelerator::NvidiaCuda
                ) if *memory_bytes >= package.hardware.minimum_memory_bytes
            );
            let mut next = checked;
            let label = if package.languages.is_empty() {
                package.label.to_owned()
            } else {
                format!("{} — {}", package.languages.join(", "), package.label)
            };
            if ui
                .add_enabled(
                    available && (!checked || selected.len() > 1),
                    eframe::egui::Checkbox::new(&mut next, label),
                )
                .changed()
            {
                change = Some((package.id.as_str().to_owned(), next));
            }
        }
    });
    let mut changed = false;
    if let Some((asset, enabled)) = change {
        update_provider_model_selection(provider, &asset, enabled);
        changed = true;
    }
    let voices = provider_voice_presets(provider);
    for asset in provider_model_assets(provider) {
        let Some(id) = xrtranslate_assets::ModelAssetId::from_config_key(&asset) else {
            continue;
        };
        let manifest = xrtranslate_assets::manifest_for(id);
        let mut languages = manifest
            .voice_presets
            .iter()
            .map(|preset| preset.language)
            .collect::<Vec<_>>();
        languages.sort_unstable();
        languages.dedup();
        for voice_language in languages {
            let choices = manifest
                .voice_presets
                .iter()
                .filter(|preset| preset.language == voice_language)
                .collect::<Vec<_>>();
            let Some(default) = choices
                .iter()
                .copied()
                .find(|preset| preset.is_default)
                .or_else(|| choices.first().copied())
            else {
                continue;
            };
            let configured_key = voices.get(voice_language);
            let configured = configured_key
                .and_then(|key| choices.iter().copied().find(|preset| preset.key == key))
                .unwrap_or(default);
            let mut selected_key = configured.key.to_owned();
            ui.horizontal(|ui| {
                ui.label(
                    eframe::egui::RichText::new(format!(
                        "{} ({voice_language}):",
                        crate::i18n::tr(language, "Base voice / accent")
                    ))
                    .size(12.0)
                    .color(crate::ui::theme::text_weak()),
                );
                eframe::egui::ComboBox::from_id_salt(("tts_voice", &provider.name, id))
                    .selected_text(configured.label)
                    .show_ui(ui, |ui| {
                        for preset in &choices {
                            ui.selectable_value(
                                &mut selected_key,
                                preset.key.to_owned(),
                                preset.label,
                            );
                        }
                    });
            });
            if selected_key != configured.key
                || configured_key.is_some_and(|key| key != configured.key)
            {
                update_provider_voice_preset(provider, voice_language, &selected_key);
                changed = true;
            }
        }
    }
    changed
}

/// Renders the same model lifecycle control inside every provider card that
/// declares one or more model assets. The provider configuration, rather than
/// model names in the UI, decides which complete package set is offered.
struct ProviderModelAction<'a> {
    project_root: &'a std::path::Path,
    language: crate::i18n::UiLanguage,
    category_key: &'a str,
    provider_name: &'a str,
    model_assets: &'a [String],
    remote: bool,
    local_models_available: bool,
}

fn render_provider_model_action(
    ui: &mut eframe::egui::Ui,
    backend: &mut crate::backend::BackendManager,
    model_tasks: &mut crate::model_install::NativeModelTaskManager,
    request: ProviderModelAction<'_>,
) -> Option<String> {
    use crate::model_install::{NativeModelTaskState, model_package_for_provider_config_key};
    use eframe::egui;

    if request.model_assets.is_empty() {
        if request.remote {
            return None;
        }
        return crate::ui::components::animated_button(
            ui,
            crate::i18n::tr(request.language, "Check model files"),
        )
        .clicked()
        .then(|| {
            match backend.check_model_files(request.category_key, request.provider_name) {
                Ok(message) => message,
                Err(error) => error,
            }
        });
    }

    let Some(capability) = model_capability_for_category(request.category_key) else {
        return Some(format!(
            "Unknown model capability for service category {}.",
            request.category_key
        ));
    };
    let packages = match request
        .model_assets
        .iter()
        .map(|model_asset| {
            model_package_for_provider_config_key(
                request.project_root,
                request.provider_name,
                capability,
                model_asset,
            )
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(packages) => packages,
        Err(error) => return Some(error),
    };

    let ready = packages
        .iter()
        .all(|package| model_tasks.is_model_ready(package.id));
    let present = packages
        .iter()
        .all(|package| model_tasks.is_model_present(package.id));
    let missing_download_bytes = packages
        .iter()
        .filter(|package| !model_tasks.is_model_present(package.id))
        .map(|package| package.download_bytes)
        .sum::<u64>();
    let busy = model_tasks.is_busy();
    let action = if present {
        "Verify"
    } else if matches!(model_tasks.state(), NativeModelTaskState::Failed(_)) {
        "Retry"
    } else {
        "Download"
    };
    let action_label = if present {
        crate::i18n::tr(request.language, action).to_owned()
    } else {
        format!(
            "{} · {}",
            crate::i18n::tr(request.language, action),
            components::format_file_size(missing_download_bytes),
        )
    };
    let clicked = ui
        .add_enabled(
            !busy && request.local_models_available,
            egui::Button::new(action_label),
        )
        .on_hover_text(
            packages
                .iter()
                .map(|package| package.label)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .clicked();
    let previous_source = model_tasks.use_mirror();
    let mut use_mirror = previous_source;
    crate::ui::components::download_mirror_toggle(ui, request.language, &mut use_mirror);
    if use_mirror != previous_source
        && let Err(error) =
            model_tasks.switch_download_source(request.project_root.to_path_buf(), use_mirror)
    {
        return Some(error);
    }
    if clicked {
        return model_tasks
            .enqueue_many(
                request.project_root.to_path_buf(),
                packages.iter().map(|package| package.id),
            )
            .err();
    }

    let status = match model_tasks.state() {
        NativeModelTaskState::Discovering => Some("Looking for existing model packages..."),
        NativeModelTaskState::Detected { .. } if ready => Some("Model package verified."),
        NativeModelTaskState::Detected { .. } if present => {
            Some("Model files found. Verify before use.")
        }
        NativeModelTaskState::Installing {
            asset_id,
            relative_path,
            ..
        } if packages.iter().any(|package| package.id == *asset_id) => {
            if let Some(path) = relative_path {
                ui.label(
                    egui::RichText::new(path)
                        .size(11.0)
                        .color(crate::ui::theme::text_weak()),
                );
            }
            Some("Preparing native model installation...")
        }
        NativeModelTaskState::Installed {
            asset_id,
            directory,
        } if packages.iter().any(|package| package.id == *asset_id) => {
            ui.label(
                egui::RichText::new(directory.display().to_string())
                    .size(11.0)
                    .color(crate::ui::theme::text_weak()),
            );
            Some("Model package verified.")
        }
        NativeModelTaskState::Failed(error) => return Some(error.clone()),
        _ => None,
    };
    if let Some(status) = status {
        ui.label(
            egui::RichText::new(crate::i18n::tr(request.language, status))
                .size(11.0)
                .color(if ready {
                    egui::Color32::from_rgb(5, 150, 105)
                } else {
                    crate::ui::theme::text_weak()
                }),
        );
    }
    None
}

fn render_field_input(
    ui: &mut eframe::egui::Ui,
    field: &mut ConfigField,
    width: f32,
    language: crate::i18n::UiLanguage,
    category_key: &str,
    provider_name: &str,
) -> bool {
    use eframe::egui;

    let descriptor = provider_field_descriptor(&field.name);
    if matches!(
        descriptor.map(|descriptor| descriptor.editor),
        Some(ProviderFieldEditor::ModelLevel)
    ) {
        let Some(current_id) =
            xrtranslate_assets::ModelAssetId::from_config_key(field.value.trim())
        else {
            return false;
        };
        let current = xrtranslate_assets::manifest_for(current_id);
        let capability = model_capability_for_category(category_key).unwrap_or(current.capability);
        let mut selected = current_id;
        let response =
            egui::ComboBox::from_id_salt(("provider_model_level", provider_name, capability))
                .selected_text(crate::i18n::tr(language, current.level.as_str()))
                .show_ui(ui, |ui| {
                    for package in crate::model_install::model_level_packages_for_provider(
                        provider_name,
                        capability,
                    ) {
                        ui.selectable_value(
                            &mut selected,
                            package.id,
                            crate::i18n::tr(language, package.level.as_str()),
                        );
                    }
                });
        if response.response.changed() || selected != current_id {
            field.value = selected.as_str().to_owned();
            return true;
        }
        return false;
    }

    match field.kind {
        JsonFieldKind::Bool => {
            let mut val = field.value.trim().parse::<bool>().unwrap_or(false);
            let label = if val { "true" } else { "false" };
            if ui.checkbox(&mut val, label).changed() {
                field.value = val.to_string();
                true
            } else {
                false
            }
        }
        JsonFieldKind::Number
            if matches!(
                descriptor.map(|descriptor| descriptor.editor),
                Some(ProviderFieldEditor::UnsignedRange { .. })
            ) =>
        {
            let Some(ProviderFieldEditor::UnsignedRange {
                minimum,
                maximum,
                speed,
            }) = descriptor.map(|descriptor| descriptor.editor)
            else {
                unreachable!("numeric editor checked above")
            };
            let Ok(mut value) = field.value.trim().parse::<u32>() else {
                return ui
                    .add(
                        egui::TextEdit::singleline(&mut field.value)
                            .desired_width(width.min(180.0))
                            .hint_text(crate::i18n::tr(language, "Positive integer")),
                    )
                    .changed();
            };
            let response = ui.add(
                egui::DragValue::new(&mut value)
                    .range(minimum..=maximum)
                    .speed(speed),
            );
            if response.changed() {
                field.value = value.to_string();
                true
            } else {
                false
            }
        }
        _ => {
            if let Some(ProviderFieldEditor::Options(options)) =
                descriptor.map(|descriptor| descriptor.editor)
            {
                let mut changed = false;
                let current = field.value.clone();
                egui::ComboBox::from_id_salt(&field.name)
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        for &opt in options {
                            if ui
                                .selectable_value(&mut field.value, opt.to_string(), opt)
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                changed
            } else {
                crate::ui::components::singleline_input(
                    ui,
                    &mut field.value,
                    value_hint(field.kind),
                    width.min(360.0),
                    field.name == "api_key",
                )
                .changed()
            }
        }
    }
}

fn provider_field_label(language: crate::i18n::UiLanguage, name: &str) -> String {
    provider_field_descriptor(name).map_or_else(
        || name.to_owned(),
        |descriptor| crate::i18n::tr(language, descriptor.label).to_owned(),
    )
}

fn provider_field_is_visible(
    field: &ConfigField,
    category_key: &str,
    provider_name: &str,
    native_model: bool,
) -> bool {
    if provider_name == "openai" && matches!(field.name.as_str(), "transport" | "url") {
        return false;
    }
    if category_key == "tts"
        && native_model
        && matches!(
            field.name.as_str(),
            "device" | "max_input_chars" | "clone_min_seconds" | "clone_max_seconds"
        )
    {
        return true;
    }
    if category_key == "tts"
        && native_model
        && matches!(
            field.name.as_str(),
            "transport"
                | "url"
                | "model"
                | "model_asset"
                | "model_assets"
                | "supported_languages"
                | "voices"
        )
    {
        return false;
    }
    provider_field_descriptor(&field.name).map_or(!native_model, |descriptor| {
        descriptor.is_visible(native_model)
    })
}

fn provider_field_help(language: crate::i18n::UiLanguage, name: &str) -> Option<&'static str> {
    provider_field_descriptor(name)
        .and_then(|descriptor| descriptor.help)
        .map(|help| crate::i18n::tr(language, help))
}

fn model_capability_for_category(
    category_key: &str,
) -> Option<xrtranslate_assets::ModelCapability> {
    match category_key {
        "asr" => Some(xrtranslate_assets::ModelCapability::Asr),
        "translation" => Some(xrtranslate_assets::ModelCapability::Translation),
        "tts" => Some(xrtranslate_assets::ModelCapability::Tts),
        _ => None,
    }
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

fn value_hint(kind: JsonFieldKind) -> &'static str {
    match kind {
        JsonFieldKind::String => "Text",
        JsonFieldKind::Bool => "true / false",
        JsonFieldKind::Number => "Number",
        JsonFieldKind::Json => "JSON value",
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

#[cfg(test)]
mod tests {
    use super::{
        ConfigField, JsonFieldKind, OnboardingSaveOutcome, ProviderCard, ServiceConfigEditor,
        prompt_target_for_translation_provider, provider_field_is_visible,
        provider_supported_languages, provider_voice_presets, update_provider_model_selection,
        validate_native_provider_asset, validate_tts_provider_asset,
    };
    use serde_json::Value;
    use xrtranslate_assets::ModelCapability;
    use xrtranslate_config::{AsrPromptMode, LocalModelRuntimeConfig, NativeProviderConfig};
    use xrtranslate_prompt::PromptProviderTarget;

    fn provider(name: &str, model_asset: Option<&str>) -> NativeProviderConfig {
        NativeProviderConfig {
            provider: name.into(),
            transport: "local".into(),
            url: "http://127.0.0.1:8000/v1/chat/completions".into(),
            model: String::new(),
            api_key: None,
            model_asset: model_asset.map(str::to_owned),
            runtime: LocalModelRuntimeConfig {
                context_window_tokens: 2_048,
                max_tokens: 128,
                parallel_slots: 1,
            },
            supports_prompt_context: false,
            asr_prompt_mode: AsrPromptMode::None,
            asr_context_max_chars: None,
            supports_vocabulary_bias: false,
            vocabulary_weight: 4,
        }
    }

    #[test]
    fn service_save_accepts_the_legacy_provider_default_asset() {
        assert!(
            validate_native_provider_asset(&provider("qwen3-gguf", None), ModelCapability::Asr)
                .is_ok()
        );
    }

    #[test]
    fn service_save_rejects_an_asset_borrowed_from_another_provider() {
        let error = validate_native_provider_asset(
            &provider("future-provider", Some("hy-mt2")),
            ModelCapability::Translation,
        )
        .unwrap_err();
        assert!(error.contains("does not belong to provider future-provider"));
    }

    #[test]
    fn service_save_validates_tts_asset_ownership() {
        let mut document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        document["tts"]["provider"] = Value::from("openvoice");
        let config = xrtranslate_config::AppConfig::from_value(document.clone()).unwrap();
        assert!(validate_tts_provider_asset(&config.tts).is_ok());

        document["tts"]["providers"]["openvoice"]["model_assets"] =
            serde_json::json!(["audio8-tts-onnx-fp16"]);
        let config = xrtranslate_config::AppConfig::from_value(document).unwrap();
        assert!(
            validate_tts_provider_asset(&config.tts)
                .unwrap_err()
                .contains("does not belong")
        );
    }

    #[test]
    fn tts_validation_rejects_overlapping_variants_and_invalid_voice_keys() {
        let mut document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        document["tts"]["provider"] = Value::from("openvoice");
        document["tts"]["providers"]["openvoice"]["model_assets"] =
            serde_json::json!(["openvoice-v2-onnx-fp16", "openvoice-v3-onnx-fp16"]);
        let config = xrtranslate_config::AppConfig::from_value(document.clone()).unwrap();
        assert!(
            validate_tts_provider_asset(&config.tts)
                .unwrap_err()
                .contains("both claim language en")
        );

        document["tts"]["providers"]["openvoice"]["model_assets"] =
            serde_json::json!(["openvoice-v2-onnx-fp16"]);
        document["tts"]["providers"]["openvoice"]["voices"] =
            serde_json::json!({"en": "en-newest"});
        let config = xrtranslate_config::AppConfig::from_value(document).unwrap();
        assert!(
            validate_tts_provider_asset(&config.tts)
                .unwrap_err()
                .contains("not provided")
        );
    }

    #[test]
    fn replacing_a_tts_variant_resets_stale_voice_presets_generically() {
        let mut provider = ProviderCard {
            name: "openvoice".into(),
            fields: vec![
                ConfigField {
                    name: "model_asset".into(),
                    value: "openvoice-v2-onnx-fp16".into(),
                    kind: JsonFieldKind::String,
                },
                ConfigField {
                    name: "model_assets".into(),
                    value: r#"["openvoice-v2-onnx-fp16"]"#.into(),
                    kind: JsonFieldKind::Json,
                },
                ConfigField {
                    name: "voices".into(),
                    value: r#"{"en":"en-british"}"#.into(),
                    kind: JsonFieldKind::Json,
                },
            ],
        };

        update_provider_model_selection(&mut provider, "openvoice-v3-onnx-fp16", true);

        assert_eq!(
            provider_voice_presets(&provider),
            std::collections::BTreeMap::from([("en".into(), "en-newest".into())])
        );
    }

    #[test]
    fn native_tts_playback_uses_the_model_audio_contract() {
        let mut document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        document["tts"]["provider"] = Value::from("openvoice");
        document["tts"]["providers"]["openvoice"]["sample_rate"] = Value::from(8_000);
        let editor = ServiceConfigEditor {
            path: "config.json".into(),
            document,
            categories: Vec::new(),
            dirty: false,
            message: None,
            onboarding_save_error: None,
        };
        assert_eq!(editor.tts_sample_rate(), 22_050);
    }

    #[test]
    fn tts_language_capability_remains_structured_for_generic_ui() {
        let provider = ProviderCard {
            name: "future".into(),
            fields: vec![ConfigField {
                name: "supported_languages".into(),
                value: r#"["en","fr-CA"]"#.into(),
                kind: JsonFieldKind::Json,
            }],
        };
        assert_eq!(
            provider_supported_languages(&provider),
            vec!["en".to_owned(), "fr-CA".to_owned()]
        );
    }

    #[test]
    fn translation_provider_selects_its_matching_prompt_page() {
        assert_eq!(
            prompt_target_for_translation_provider("hunyuan", "local"),
            PromptProviderTarget::Hunyuan
        );
        assert_eq!(
            prompt_target_for_translation_provider("hunyuan", "openai"),
            PromptProviderTarget::OpenAiCompatible
        );
        assert_eq!(
            prompt_target_for_translation_provider("openai-custom", "openai"),
            PromptProviderTarget::OpenAiCompatible
        );
    }

    #[test]
    fn official_openai_hides_fixed_connection_fields() {
        let field = |name: &str| ConfigField {
            name: name.into(),
            value: String::new(),
            kind: JsonFieldKind::String,
        };

        assert!(!provider_field_is_visible(
            &field("url"),
            "asr",
            "openai",
            false
        ));
        assert!(!provider_field_is_visible(
            &field("transport"),
            "asr",
            "openai",
            false
        ));
        assert!(provider_field_is_visible(
            &field("api_key"),
            "asr",
            "openai",
            false
        ));
        assert!(provider_field_is_visible(
            &field("url"),
            "asr",
            "openai-compatible",
            false
        ));
    }

    #[test]
    fn onboarding_provider_drafts_drive_shared_runtime_requirements() {
        let document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        let categories = vec![
            ServiceConfigEditor::make_category(&document, "asr", "ASR / Speech Recognition"),
            ServiceConfigEditor::make_category(&document, "translation", "Translation"),
        ];
        let mut editor = ServiceConfigEditor {
            path: "config.json".into(),
            document,
            categories,
            dirty: false,
            message: None,
            onboarding_save_error: None,
        };

        editor.select_onboarding_provider("asr", "openai");
        assert_eq!(
            editor.save_onboarding_configuration().unwrap(),
            OnboardingSaveOutcome::IncompleteRemoteProvider
        );
        assert_eq!(editor.onboarding_message(), None);
        editor.set_onboarding_remote_fields("asr", "gpt-4o-transcribe".into(), "asr-key".into());
        assert_eq!(editor.onboarding_message(), None);
        editor.select_onboarding_provider("translation", "openai");
        editor.set_onboarding_remote_fields(
            "translation",
            "gpt-4o-mini".into(),
            "translation-key".into(),
        );

        assert_eq!(
            editor.runtime_requirements(),
            xrtranslate_config::RuntimeRequirements::default()
        );
        assert!(editor.has_unsaved_changes());
    }

    #[test]
    fn inactive_remote_provider_does_not_block_local_onboarding_selection() {
        let document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        let categories = vec![
            ServiceConfigEditor::make_category(&document, "asr", "ASR / Speech Recognition"),
            ServiceConfigEditor::make_category(&document, "translation", "Translation"),
            ServiceConfigEditor::make_category(&document, "tts", "Text to Speech"),
        ];
        let mut editor = ServiceConfigEditor {
            path: "config.json".into(),
            document,
            categories,
            dirty: false,
            message: None,
            onboarding_save_error: None,
        };

        editor.select_onboarding_provider("asr", "qwen-audio-streaming");
        assert!(editor.has_incomplete_remote_provider());

        editor.select_onboarding_provider("asr", "qwen3-gguf");
        assert!(!editor.has_incomplete_remote_provider());
        assert_eq!(
            editor.onboarding_provider_state("asr").unwrap().selected,
            "qwen3-gguf"
        );
    }

    #[test]
    fn onboarding_preserves_errors_from_the_selected_local_provider() {
        let document: Value = serde_json::from_str(include_str!("../../config.json")).unwrap();
        let categories = vec![
            ServiceConfigEditor::make_category(&document, "asr", "ASR / Speech Recognition"),
            ServiceConfigEditor::make_category(&document, "translation", "Translation"),
            ServiceConfigEditor::make_category(&document, "tts", "Text to Speech"),
        ];
        let mut editor = ServiceConfigEditor {
            path: "config.json".into(),
            document,
            categories,
            dirty: false,
            message: None,
            onboarding_save_error: None,
        };
        let asr = editor
            .categories
            .iter_mut()
            .find(|category| category.key == "asr")
            .unwrap();
        let local = asr
            .providers
            .iter_mut()
            .find(|provider| provider.name == asr.selected_provider)
            .unwrap();
        local
            .fields
            .iter_mut()
            .find(|field| field.name == "url")
            .unwrap()
            .value
            .clear();

        let error = editor.save_onboarding_configuration().unwrap_err();
        assert!(error.contains("asr.providers.qwen3-gguf.url"));
        assert_eq!(editor.onboarding_message(), Some(error.as_str()));
        assert_eq!(
            editor.onboarding_save_error.as_deref(),
            Some(error.as_str())
        );
    }
}
