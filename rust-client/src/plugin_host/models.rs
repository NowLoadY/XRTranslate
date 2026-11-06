use super::*;
use crossbeam_channel::{Receiver, bounded};
use xrtranslate_download::{DownloadCancellation, DownloadClient, DownloadSource};

enum Update {
    Progress(u64, u64),
    Finished(Result<(), String>),
}
#[derive(Default)]
pub(super) struct Downloads {
    active: Option<(String, DownloadCancellation, Receiver<Update>)>,
    pub progress: Option<(u64, u64)>,
    pub error: Option<(String, String)>,
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
impl Downloads {
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
    fn active_for(&self, plugin: &str) -> bool {
        self.active.as_ref().is_some_and(|(id, _, _)| id == plugin)
    }
    pub fn cancel(&mut self, plugin: &str) {
        if let Some((id, cancel, _)) = &self.active
            && id == plugin
        {
            cancel.cancel();
        }
    }
    pub fn start(
        &mut self,
        plugin: &str,
        model: &xrtranslate_plugin::Model,
        root: &Path,
        proxy: &str,
        mirror: bool,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("A model download is already running".into());
        }
        let destination = root.join(&model.directory);
        let package = package(model);
        let cancel = DownloadCancellation::default();
        let client = DownloadClient::with_proxy_source_and_cancellation(
            "XRTranslate",
            (!proxy.trim().is_empty()).then_some(proxy),
            if mirror {
                DownloadSource::Mirror
            } else {
                DownloadSource::Official
            },
            cancel.clone(),
        )
        .map_err(|e| e.to_string())?;
        let (tx, rx) = bounded(16);
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(package.install(&client, &destination, |_, done, total| {
                            let _ = tx.try_send(Update::Progress(done, total));
                        }))
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
            let _ = tx.send(Update::Finished(result));
        });
        self.active = Some((plugin.into(), cancel, rx));
        self.progress = Some((0, model.files.iter().map(|f| f.bytes).sum()));
        self.error = None;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<String> {
        let (id, _, rx) = self.active.as_ref()?;
        let id = id.clone();
        let mut finished = None;
        loop {
            match rx.try_recv() {
                Ok(Update::Progress(done, total)) => self.progress = Some((done, total)),
                Ok(Update::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    finished = Some(Err("Download interrupted".into()));
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
            }
        }
        if let Some(result) = finished {
            self.error = result.err().map(|error| (id.clone(), error));
            self.active = None;
            self.progress = None;
            return Some(id);
        }
        None
    }
}
impl Drop for Downloads {
    fn drop(&mut self) {
        if let Some((_, cancel, _)) = &self.active {
            cancel.cancel();
        }
    }
}

impl Host {
    pub fn model_cards(&self) -> Vec<String> {
        self.installed
            .iter()
            .filter(|(_, p)| !p.manifest.models.is_empty())
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn model_card(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        language: UiLanguage,
        proxy: &str,
        mirror: bool,
    ) {
        use crate::{i18n::tr, ui::components};
        let Some(entry) = self.installed.get(id) else {
            return;
        };
        let title = entry.manifest.title.get(locale(language)).to_owned();
        let models = entry.manifest.models.clone();
        let before = entry.settings.model.clone();
        let mut selected = before.clone().unwrap_or_default();
        let mut install = false;
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
            if let Some(model) = models.iter().find(|m| m.id == selected) {
                let ready = package(model)
                    .check(&self.models_root.join(&model.directory))
                    .unwrap_or(false);
                if ready {
                    ui.label(tr(language, "Installed"));
                } else if ui
                    .add_enabled_ui(!self.models.busy(), |ui| {
                        components::button(ui, tr(language, "Download"))
                    })
                    .inner
                    .clicked()
                {
                    install = true;
                }
                if self.models.active_for(id) {
                    if let Some((done, total)) = self.models.progress {
                        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32));
                    }
                    if components::button(ui, tr(language, "Cancel")).clicked() {
                        self.models.cancel(id);
                    }
                }
            }
            if let Some((plugin, error)) = &self.models.error
                && plugin == id
            {
                components::error_notice(ui, language, error);
            }
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
        if install && let Some(model) = models.iter().find(|m| Some(&m.id) == selected.as_ref()) {
            if let Err(error) = self
                .models
                .start(id, model, &self.models_root, proxy, mirror)
            {
                self.models.error = Some((id.into(), error));
            }
        }
    }
}
