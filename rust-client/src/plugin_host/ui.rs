use super::*;
use crate::{i18n::tr, ui::components};
use xrtranslate_plugin::Widget;
mod scene;

#[derive(Default)]
pub(super) struct PageState {
    composer: components::text_composer::TextComposer,
    inputs: BTreeMap<String, (String, String)>,
    scenes: BTreeMap<String, (u64, scene::Scene)>,
}
impl Host {
    pub fn composer(&mut self, id: PluginId) -> &mut components::text_composer::TextComposer {
        &mut self
            .installed
            .get_mut(id.as_str())
            .expect("installed page")
            .page
            .composer
    }
    pub fn ui_action(&self, id: PluginId, value: Value) -> Result<(), String> {
        self.installed
            .get(id.as_str())
            .and_then(|p| p.process.as_ref())
            .ok_or("Plugin unavailable")?
            .send(Message::Event {
                name: "ui".into(),
                value,
            })
    }
    pub fn page(
        &mut self,
        ui: &mut egui::Ui,
        id: PluginId,
        language: UiLanguage,
    ) -> Vec<(String, bool)> {
        let Some(entry) = self.installed.get_mut(id.as_str()) else {
            ui.label(tr(language, "Plugin is not installed"));
            return Vec::new();
        };
        ui.heading(entry.manifest.title.get(locale(language)));
        ui.add_space(12.0);
        let mut actions = Vec::new();
        let mut contributions = Vec::new();
        if let Some(process) = &entry.process {
            let widgets = process.view();
            ui.push_id(id, |ui| {
                render(
                    ui,
                    &widgets,
                    &mut entry.page,
                    language,
                    &mut actions,
                    &mut contributions,
                )
            });
            for value in actions {
                if let Err(error) = process.send(Message::Event {
                    name: "ui".into(),
                    value,
                }) {
                    entry.error = Some(error);
                }
            }
        }
        if let Some(error) = &entry.error {
            components::error_notice(ui, language, error);
        }
        contributions
    }
    pub fn settings(&mut self, ui: &mut egui::Ui, language: UiLanguage) {
        if components::button(ui, tr(language, "Plugin folder")).clicked() {
            match std::fs::create_dir_all(&self.directory) {
                Ok(()) => {
                    if let Ok(url) = reqwest::Url::from_directory_path(&self.directory) {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url.to_string()));
                    }
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        let mut enable = None;
        for (id, entry) in &mut self.installed {
            components::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(entry.manifest.title.get(locale(language)));
                    let mut enabled = entry.settings.enabled;
                    if components::toggle_with_label(ui, &mut enabled, tr(language, "Enable"))
                        .changed()
                    {
                        enable = Some((id.clone(), enabled));
                    }
                });
                if let Some(error) = &entry.error {
                    components::error_notice(ui, language, error);
                }
                if !entry.manifest.page
                    && let Some(process) = &entry.process
                {
                    let mut actions = Vec::new();
                    ui.push_id(id, |ui| {
                        render(
                            ui,
                            &process.view(),
                            &mut entry.page,
                            language,
                            &mut actions,
                            &mut Vec::new(),
                        )
                    });
                    for value in actions {
                        if let Err(error) = process.send(Message::Event {
                            name: "ui".into(),
                            value,
                        }) {
                            entry.error = Some(error);
                        }
                    }
                }
            });
        }
        if let Some((id, enabled)) = enable
            && let Err(error) = self.set_enabled(&id, enabled)
        {
            self.error = Some(error);
        }
        if let Some(error) = &self.error {
            components::error_notice(ui, language, error);
        }
    }
}

fn render(
    ui: &mut egui::Ui,
    widgets: &[Widget],
    state: &mut PageState,
    language: UiLanguage,
    actions: &mut Vec<Value>,
    contributions: &mut Vec<(String, bool)>,
) {
    for widget in widgets {
        let lang = locale(language);
        match widget {
            Widget::Label { text } => {
                ui.label(text.get(lang));
            }
            Widget::Heading { text } => {
                ui.heading(text.get(lang));
            }
            Widget::Row { children } => {
                ui.horizontal_wrapped(|ui| {
                    render(ui, children, state, language, actions, contributions)
                });
            }
            Widget::Card { children } => {
                components::card(ui, |ui| {
                    render(ui, children, state, language, actions, contributions)
                });
            }
            Widget::Link { text, url } => {
                ui.hyperlink_to(text.get(lang), url);
            }
            Widget::Column { children } => {
                ui.vertical(|ui| render(ui, children, state, language, actions, contributions));
            }
            Widget::Button {
                id,
                label,
                enabled,
                primary,
            } => {
                ui.push_id(id, |ui| {
                    let clicked = if *primary {
                        components::primary_button_enabled(ui, label.get(lang), *enabled).clicked()
                    } else {
                        ui.add_enabled_ui(*enabled, |ui| components::button(ui, label.get(lang)))
                            .inner
                            .clicked()
                    };
                    if clicked {
                        actions.push(json!({"id":id}));
                    }
                });
            }
            Widget::Input {
                id,
                label,
                value,
                enabled,
            } => {
                ui.push_id(id, |ui| {
                    let (local, remote) = state
                        .inputs
                        .entry(id.clone())
                        .or_insert_with(|| (value.clone(), value.clone()));
                    if remote != value {
                        if local == remote {
                            *local = value.clone();
                        }
                        *remote = value.clone();
                    }
                    let response = ui
                        .add_enabled_ui(*enabled, |ui| {
                            components::singleline_input(
                                ui,
                                local,
                                label.get(lang),
                                ui.available_width().clamp(80.0, 520.0),
                                false,
                            )
                        })
                        .inner;
                    let submit =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if response.changed() || submit {
                        actions.push(json!({"id":id,"value":local,"submit":submit}));
                    }
                });
            }
            Widget::Toggle {
                id,
                label,
                value,
                enabled,
            } => {
                ui.push_id(id, |ui| {
                    let mut value = *value;
                    if ui
                        .add_enabled_ui(*enabled, |ui| {
                            components::toggle_with_label(ui, &mut value, label.get(lang))
                        })
                        .inner
                        .changed()
                    {
                        actions.push(json!({"id":id,"value":value}));
                    }
                });
            }
            Widget::Select {
                id,
                label: _,
                value,
                choices,
            } => {
                ui.push_id(id, |ui| {
                    let mut selected = value.clone();
                    let options: Vec<_> = choices
                        .iter()
                        .map(|(key, text)| (key.clone(), text.get(lang).to_owned()))
                        .collect();
                    let current = options
                        .iter()
                        .find(|(key, _)| key == value)
                        .map(|(_, text)| text.as_str())
                        .unwrap_or(value);
                    components::searchable_combobox(ui, id, current, &mut selected, &options);
                    if &selected != value {
                        actions.push(json!({"id":id,"value":selected}));
                    }
                });
            }
            Widget::Number {
                id,
                label,
                value,
                min,
                max,
            } => {
                ui.push_id(id, |ui| {
                    let mut number = *value;
                    if components::number_input(ui, &mut number, label.get(lang), *min..=*max)
                        .changed()
                    {
                        actions.push(json!({"id":id,"value":number}));
                    }
                });
            }
            Widget::Scene { id, scene: data } => {
                let (revision, scene) = state
                    .scenes
                    .entry(id.clone())
                    .or_insert_with(|| (data.revision, scene::Scene::default()));
                if *revision != data.revision {
                    *scene = scene::Scene::default();
                    *revision = data.revision;
                }
                scene.show(ui, data);
            }
            Widget::Host {
                capability,
                enabled,
            } => contributions.push((capability.clone(), *enabled)),
            Widget::Busy => {
                ui.spinner();
            }
            Widget::Error { message } => {
                components::error_notice(ui, language, message);
            }
        }
    }
}
