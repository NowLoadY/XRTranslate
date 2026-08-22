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
}

/// Editable view of the ASR, translation, and TTS provider portions of `config.json`.
/// The original JSON document is retained so unrelated project settings are preserved.
pub struct ServiceConfigEditor {
    path: PathBuf,
    document: Value,
    categories: Vec<ServiceCategory>,
    dirty: bool,
    message: Option<String>,
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
        self.document
            .get("tts")
            .and_then(Value::as_object)
            .and_then(|section| {
                let selected = section.get("provider")?.as_str()?;
                section
                    .get("providers")?
                    .get(selected)?
                    .get("sample_rate")?
                    .as_u64()
            })
            .and_then(|rate| u32::try_from(rate).ok())
            .unwrap_or(44_100)
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

    pub(crate) fn save_onboarding_configuration(&mut self) -> Result<(), String> {
        let result = self.save();
        self.message = result.as_ref().err().and_then(|error| {
            if error.contains(".api_key is required for remote API providers") {
                None
            } else {
                Some(error.clone())
            }
        });
        result
    }

    pub(crate) fn onboarding_message(&self) -> Option<&str> {
        self.message.as_deref()
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
        providers.sort_by(|a: &ProviderCard, b: &ProviderCard| a.name.cmp(&b.name));

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
                        let model_asset =
                            provider_model_asset(&self.categories[cat_idx].providers[provider_idx]);
                        let remote =
                            provider_is_remote(&self.categories[cat_idx].providers[provider_idx]);

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
                                                model_asset: model_asset.as_deref(),
                                                remote,
                                            },
                                        ) {
                                            self.message = Some(message);
                                        }
                                    });

                                    ui.add_space(8.0);

                                    let fields_len = self.categories[cat_idx].providers
                                        [provider_idx]
                                        .fields
                                        .iter()
                                        .filter(|field| {
                                            provider_field_is_visible(
                                                field,
                                                category_key,
                                                &provider_name,
                                                model_asset.is_some(),
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
                                                        model_asset.is_some(),
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
                        let model_asset =
                            provider_model_asset(&self.categories[cat_idx].providers[idx]);
                        let remote = provider_is_remote(&self.categories[cat_idx].providers[idx]);

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
                                    model_asset: model_asset.as_deref(),
                                    remote,
                                },
                            ) {
                                self.message = Some(message);
                            }
                        });
                        ui.add_space(10.0);

                        let fields_len = self.categories[cat_idx].providers[idx]
                            .fields
                            .iter()
                            .filter(|field| {
                                provider_field_is_visible(
                                    field,
                                    category_key,
                                    &provider_name,
                                    model_asset.is_some(),
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
                                            model_asset.is_some(),
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
                if let Err(error) =
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
        category.providers.sort_by(|a, b| a.name.cmp(&b.name));
        category.selected_provider = name;
        Ok(())
    }
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

fn provider_model_asset(provider: &ProviderCard) -> Option<String> {
    provider
        .fields
        .iter()
        .find(|field| field.name == "model_asset")
        .map(|field| field.value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

/// Renders the same model lifecycle control inside every provider card that
/// declares a `model_asset`. The provider configuration, rather than a model
/// name in the UI, decides which package is offered.
struct ProviderModelAction<'a> {
    project_root: &'a std::path::Path,
    language: crate::i18n::UiLanguage,
    category_key: &'a str,
    provider_name: &'a str,
    model_asset: Option<&'a str>,
    remote: bool,
}

fn render_provider_model_action(
    ui: &mut eframe::egui::Ui,
    backend: &mut crate::backend::BackendManager,
    model_tasks: &mut crate::model_install::NativeModelTaskManager,
    request: ProviderModelAction<'_>,
) -> Option<String> {
    use crate::model_install::{NativeModelTaskState, model_package_for_provider_config_key};
    use eframe::egui;

    let Some(model_asset) = request.model_asset else {
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
    };

    let Some(capability) = model_capability_for_category(request.category_key) else {
        return Some(format!(
            "Unknown model capability for service category {}.",
            request.category_key
        ));
    };
    let package = match model_package_for_provider_config_key(
        request.project_root,
        request.provider_name,
        capability,
        model_asset,
    ) {
        Ok(package) => package,
        Err(error) => return Some(error),
    };

    let ready = model_tasks.is_model_ready(package.id);
    let present = model_tasks.is_model_present(package.id);
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
            components::format_file_size(package.download_bytes),
        )
    };
    let clicked = ui
        .add_enabled(!busy, egui::Button::new(action_label))
        .on_hover_text(package.label)
        .clicked();
    let previous_source = model_tasks.use_mirror();
    let mut use_mirror = previous_source;
    crate::ui::components::download_mirror_toggle(ui, request.language, &mut use_mirror);
    if use_mirror != previous_source
        && let Err(error) = model_tasks.switch_download_source(
            request.project_root.to_path_buf(),
            package.id,
            use_mirror,
        )
    {
        return Some(error);
    }
    if clicked {
        return model_tasks
            .install(request.project_root.to_path_buf(), package.id)
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
        } if *asset_id == package.id => {
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
        } if *asset_id == package.id => {
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
            "device"
                | "sample_rate"
                | "max_input_chars"
                | "clone_min_seconds"
                | "clone_max_seconds"
        )
    {
        return true;
    }
    if category_key == "tts"
        && native_model
        && matches!(field.name.as_str(), "transport" | "url" | "model")
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
        ConfigField, JsonFieldKind, ServiceConfigEditor, prompt_target_for_translation_provider,
        provider_field_is_visible, validate_native_provider_asset,
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
        };

        editor.select_onboarding_provider("asr", "openai");
        assert!(editor.save_onboarding_configuration().is_err());
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
}
