use super::*;
use crate::model_install::{RegisteredModel, ResourceId};

pub(crate) struct ModelResource {
    pub id: ResourceId,
    pub title: String,
    pub label: String,
    pub selected: bool,
    pub resource: RegisteredModel,
}

pub(crate) struct PluginModelInfo {
    pub plugin: String,
    pub plugin_title: String,
    pub id: String,
    pub label: String,
    pub group: Option<String>,
    pub runtime: String,
    pub download_bytes: u64,
    pub selected: bool,
    pub resource_id: ResourceId,
}

pub(super) fn package(model: &xrtranslate_plugin::Model) -> xrtranslate_assets::ModelPackage {
    xrtranslate_assets::ModelPackage {
        id: model.id.clone(),
        revision: model.revision.clone(),
        archive: None,
        files: model
            .files
            .iter()
            .map(|file| xrtranslate_assets::PackageFile {
                path: file.path.clone(),
                url: file.url.clone(),
                bytes: file.bytes,
            })
            .collect(),
    }
}

impl Host {
    pub fn model_cards(&self) -> Vec<String> {
        self.installed
            .iter()
            .filter(|(_, p)| p.settings.enabled && !p.manifest.models.is_empty())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn model_card(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        language: UiLanguage,
        is_ready: impl Fn(&ResourceId) -> bool,
    ) {
        use crate::{
            i18n::tr,
            ui::{components, theme},
        };
        let Some(entry) = self.installed.get(id) else {
            return;
        };
        let title = entry.manifest.title.get(locale(language)).to_owned();
        let models = entry.manifest.models.clone();
        let selected_ids: std::collections::BTreeSet<String> = entry
            .settings
            .selected_models(&entry.manifest)
            .into_iter()
            .map(|m| m.id.clone())
            .collect();
        let mut toggled = None;
        components::card(ui, |ui| {
            ui.heading(&title);
            ui.add_space(4.0);
            for model in &models {
                let mut checked = selected_ids.contains(&model.id);
                let total_bytes: u64 = model.files.iter().map(|file| file.bytes).sum();
                let size = components::format_file_size(total_bytes);
                let resource_id = ResourceId::Plugin {
                    plugin: id.to_owned(),
                    model: model.id.clone(),
                };
                let meta = if is_ready(&resource_id) {
                    format!("{size} · {}", tr(language, "Installed"))
                } else {
                    size
                };
                ui.horizontal_wrapped(|ui| {
                    let response = components::checkbox_labeled(
                        ui,
                        &mut checked,
                        &model.label,
                        &format!("{title}: {}", model.label),
                    );
                    ui.label(egui::RichText::new(meta).small().color(theme::text_weak()));
                    if response.changed() {
                        toggled = Some((model.id.clone(), checked));
                    }
                });
            }
        });
        if let Some((model_id, selected)) = toggled
            && let Err(error) = self.set_model_selected(id, &model_id, selected)
        {
            self.error = Some(error);
        }
    }

    pub fn set_model_selected(
        &mut self,
        plugin: &str,
        model_id: &str,
        selected: bool,
    ) -> Result<bool, String> {
        let entry = self
            .installed
            .get_mut(plugin)
            .ok_or_else(|| format!("Plugin `{plugin}` is not installed"))?;
        if !entry.manifest.models.iter().any(|m| m.id == model_id) {
            return Err(format!("Model `{model_id}` is not declared by `{plugin}`"));
        }
        let changed = entry
            .settings
            .set_model_selected(&entry.manifest, model_id, selected);
        if changed {
            entry.environment_dirty = true;
            entry.error = xrtranslate_plugin::directory::require_host(
                &entry.manifest,
                env!("CARGO_PKG_VERSION"),
            )
            .err();
            self.save(plugin)?;
        }
        Ok(changed)
    }

    pub fn plugin_models_snapshot(
        &self,
        plugin_filter: Option<&str>,
        language: UiLanguage,
    ) -> Vec<PluginModelInfo> {
        self.installed
            .iter()
            .filter(|(id, entry)| {
                entry.settings.enabled && plugin_filter.is_none_or(|filter| filter == id.as_str())
            })
            .flat_map(|(id, entry)| {
                let plugin_title = entry.manifest.title.get(locale(language)).to_owned();
                entry.manifest.models.iter().map(move |model| PluginModelInfo {
                    plugin: id.clone(),
                    plugin_title: plugin_title.clone(),
                    id: model.id.clone(),
                    label: model.label.clone(),
                    group: model.group.clone(),
                    runtime: model.runtime.clone(),
                    download_bytes: model.files.iter().map(|file| file.bytes).sum(),
                    selected: entry.settings.has_model(&model.id),
                    resource_id: ResourceId::Plugin {
                        plugin: id.clone(),
                        model: model.id.clone(),
                    },
                })
            })
            .collect()
    }

    pub fn plugin_model_state(&self, plugin: &str, model_id: &str) -> Result<(&xrtranslate_plugin::Model, bool, bool), String> {
        let entry = self
            .installed
            .get(plugin)
            .ok_or_else(|| format!("Plugin `{plugin}` is not installed"))?;
        if !entry.settings.enabled {
            return Err(format!("Plugin `{plugin}` is disabled"));
        }
        let model = entry
            .manifest
            .models
            .iter()
            .find(|m| m.id == model_id)
            .ok_or_else(|| format!("Model `{model_id}` is not declared by `{plugin}`"))?;
        Ok((
            model,
            entry.settings.has_model(model_id),
            entry.process.is_some() && entry.environment_dirty,
        ))
    }

    pub fn model_resources(&self, language: UiLanguage) -> Vec<ModelResource> {
        self.installed
            .iter()
            .flat_map(|(id, entry)| {
                entry
                    .manifest
                    .models
                    .iter()
                    .map(move |model| ModelResource {
                        id: ResourceId::Plugin {
                            plugin: id.clone(),
                            model: model.id.clone(),
                        },
                        title: entry.manifest.title.get(locale(language)).into(),
                        label: model.label.clone(),
                        selected: entry.settings.enabled && entry.settings.has_model(&model.id),
                        resource: RegisteredModel {
                            package: package(model),
                            directory: self.models_root.join(&model.directory),
                        },
                    })
            })
            .collect()
    }

    pub fn model_resource_changed(&mut self, id: &ResourceId) {
        if let ResourceId::Plugin { plugin, model } = id
            && let Some(entry) = self.installed.get_mut(plugin)
            && entry.settings.has_model(model)
        {
            entry.environment_dirty = true;
            entry.error = xrtranslate_plugin::directory::require_host(
                &entry.manifest,
                env!("CARGO_PKG_VERSION"),
            )
            .err();
        }
    }
}
