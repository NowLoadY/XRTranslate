use super::{Host, UiLanguage, egui};
use crate::{file_dialog::FileDialog, i18n::tr, ui::components};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
    time::Instant,
};
use xrtranslate_plugin::{Manifest, install::Prepared};

#[derive(Default)]
pub(super) enum Import {
    #[default]
    Idle,
    Preparing(Receiver<io::Result<Prepared>>),
    Installing(Receiver<io::Result<Manifest>>),
    Installed(xrtranslate_plugin::Text),
    Failed(String),
}

impl Host {
    pub(super) fn import_control(&mut self, ui: &mut egui::Ui, language: UiLanguage) {
        let busy = matches!(self.import, Import::Preparing(_) | Import::Installing(_));
        let label = tr(language, "Import plugin");
        let filter = tr(language, "Plugin ZIP or plugin.json");
        let action = ui
            .add_enabled_ui(!busy, |ui| components::button(ui, label))
            .inner
            .on_hover_text(filter);
        let picked = FileDialog::new()
            .add_filter(filter, &["zip", "json"])
            .pick_file(ui.ctx(), "settings_plugin_import", action.clicked());
        let dropped = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_owned())
        });
        if !busy && let Some(path) = picked.or(dropped) {
            self.prepare_import(path, ui.ctx());
        }
        match &self.import {
            Import::Preparing(_) | Import::Installing(_) => {
                ui.spinner();
            }
            Import::Installed(title) => {
                ui.weak(format!(
                    "{} · {}",
                    tr(language, "Imported"),
                    title.get(super::locale(language))
                ));
            }
            Import::Failed(error) => {
                components::error_notice(ui, language, error);
            }
            Import::Idle => {}
        }
    }

    fn prepare_import(&mut self, source: PathBuf, ctx: &egui::Context) {
        let directory = self.directory.clone();
        self.import = Import::Preparing(self.io.submit(
            move || {
                xrtranslate_plugin::install::prepare(&source, &directory, env!("CARGO_PKG_VERSION"))
            },
            Some(ctx.clone()),
        ));
    }

    /// Hold discovery only during publication so an old scan cannot restart a replaced executable.
    pub(super) fn poll_import(&mut self, ctx: &egui::Context) -> bool {
        match &self.import {
            Import::Preparing(reply) => match reply.try_recv() {
                Ok(Ok(prepared)) => {
                    let id = prepared.manifest.provider_id();
                    if self
                        .installed
                        .get(id)
                        .is_some_and(|p| p.manifest.id == prepared.manifest.id)
                    {
                        self.stop(id);
                        self.installed.remove(id);
                        self.navigation_changed = true;
                        self.context.remove("host.context");
                    }
                    self.scan = None;
                    self.import = Import::Installing(
                        self.io
                            .submit(move || prepared.install(), Some(ctx.clone())),
                    );
                    return true;
                }
                Ok(Err(error)) => self.import = Import::Failed(error.to_string()),
                Err(TryRecvError::Disconnected) => {
                    self.import = Import::Failed("Plugin import interrupted".into())
                }
                Err(TryRecvError::Empty) => {}
            },
            Import::Installing(reply) => {
                self.import = match reply.try_recv() {
                    Ok(Ok(manifest)) => Import::Installed(manifest.title),
                    Ok(Err(error)) => Import::Failed(error.to_string()),
                    Err(TryRecvError::Disconnected) => {
                        Import::Failed("Plugin import interrupted".into())
                    }
                    Err(TryRecvError::Empty) => return true,
                };
                self.next_scan = Instant::now();
            }
            _ => {}
        }
        false
    }
}
