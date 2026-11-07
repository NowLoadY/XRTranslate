use super::*;
use crate::model_install::{RegisteredModel, ResourceId};

pub(crate) struct ModelResource {
    pub id: ResourceId,
    pub title: String,
    pub label: String,
    pub selected: bool,
    pub resource: RegisteredModel,
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
    pub fn model_card(&mut self, ui: &mut egui::Ui, id: &str, language: UiLanguage) {
        use crate::{i18n::tr, ui::components};
        let Some(entry) = self.installed.get(id) else {
            return;
        };
        let title = entry.manifest.title.get(locale(language)).to_owned();
        let models = entry.manifest.models.clone();
        let before = entry.settings.model.clone();
        let mut selected = before.clone().unwrap_or_default();
        components::card(ui, |ui| {
            ui.heading(&title);
            let options: Vec<_> = std::iter::once((String::new(), tr(language, "Skip").to_owned()))
                .chain(
                    models
                        .iter()
                        .map(|model| (model.id.clone(), model.label.clone())),
                )
                .collect();
            let label = options
                .iter()
                .find(|(key, _)| key == &selected)
                .map(|(_, label)| label.clone())
                .unwrap_or_default();
            components::searchable_combobox(
                ui,
                format!("plugin_model_{id}"),
                label,
                &mut selected,
                &options,
            );
        });
        let selected = (!selected.is_empty()).then_some(selected);
        if selected != before {
            self.stop(id);
            let entry = self.installed.get_mut(id).unwrap();
            entry.settings.model = selected.clone();
            entry.error = xrtranslate_plugin::directory::require_host(
                &entry.manifest,
                env!("CARGO_PKG_VERSION"),
            )
            .err();
            if let Err(error) = self.save(id) {
                self.error = Some(error);
            }
        }
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
                        selected: entry.settings.enabled
                            && entry.settings.model.as_ref() == Some(&model.id),
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
            && self
                .installed
                .get(plugin)
                .is_some_and(|entry| entry.settings.model.as_ref() == Some(model))
        {
            self.stop(plugin);
            let entry = self.installed.get_mut(plugin).unwrap();
            entry.error = xrtranslate_plugin::directory::require_host(
                &entry.manifest,
                env!("CARGO_PKG_VERSION"),
            )
            .err();
        }
    }
}
