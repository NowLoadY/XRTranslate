//! Vocabulary browsing and editing; bookmarks remain independent of plugin enablement.
mod detail;
mod examples;
use crate::{
    ask,
    history::archive::{
        Archive, PAGE_SIZE,
        vocabulary::{Action, DEFAULT_FOLDER, Entry, Folder, Output, Record, Reply, Translation},
    },
    i18n::{UiLanguage, tr},
    ui::{components, theme},
};
use eframe::egui;
use xrtranslate_engine::language::LanguageCapabilities;

struct Detail {
    source: String,
    source_lang: String,
    rows: Vec<Translation>,
    edit_on_load: Option<i64>,
}

pub(crate) struct ViewState {
    pub detail: bool,
    pub empty: bool,
    pub working: bool,
}

pub(crate) struct Page {
    folder: i64,
    folders: Vec<Folder>,
    records: Vec<Record>,
    query: String,
    offset: usize,
    more: bool,
    reload: bool,
    revision: u64,
    pending: Option<(bool, Reply)>,
    error: Option<String>,
    folder_edit: Option<(Option<i64>, String)>,
    detail: Option<Detail>,
    word_edit: Option<(Translation, i64)>,
    examples: examples::Requests,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            folder: DEFAULT_FOLDER,
            folders: Vec::new(),
            records: Vec::new(),
            query: String::new(),
            offset: 0,
            more: false,
            reload: true,
            revision: 0,
            pending: None,
            error: None,
            folder_edit: None,
            detail: None,
            word_edit: None,
            examples: Default::default(),
        }
    }
}
impl Page {
    pub fn view_state(&self) -> ViewState {
        ViewState {
            detail: self.detail.is_some(),
            empty: self.records.is_empty()
                && self.query.trim().is_empty()
                && self.pending.is_none()
                && !self.reload
                && self.error.is_none(),
            working: self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.rows.iter().any(|row| self.examples.busy(&row.entry))),
        }
    }

    pub fn poll_examples(
        &mut self,
        archive: &Archive,
        notifications: &crate::ui::notifications::Notifications,
        language: UiLanguage,
        ctx: &egui::Context,
    ) {
        self.examples.poll(archive, notifications, language, ctx);
    }
    fn request(&mut self, action: Action, archive: &Archive, ctx: &egui::Context) {
        self.error = None;
        self.pending = Some((action.mutates(), archive.vocabulary(action, ctx)));
    }
    fn poll(&mut self, archive: &Archive, ctx: &egui::Context) {
        if archive.vocabulary_revision() != self.revision {
            self.revision = archive.vocabulary_revision();
            self.reload = true;
        }
        if let Some((mutation, result)) = self
            .pending
            .as_ref()
            .and_then(|(mutation, reply)| reply.try_recv().ok().map(|result| (*mutation, result)))
        {
            self.pending = None;
            match result {
                Ok(Output::Page {
                    folders,
                    mut records,
                }) if !self.reload && self.detail.is_none() => {
                    self.more = records.len() > PAGE_SIZE;
                    records.truncate(PAGE_SIZE);
                    self.records = records;
                    self.folders = folders;
                    if !self.folders.iter().any(|folder| folder.id == self.folder) {
                        self.folder = DEFAULT_FOLDER;
                        self.offset = 0;
                        self.reload = true;
                    } else if self.records.is_empty() && self.offset > 0 {
                        self.offset = self.offset.saturating_sub(PAGE_SIZE);
                        self.reload = true;
                    }
                }
                Ok(Output::Detail(rows)) if !self.reload => {
                    if rows.is_empty() {
                        self.back();
                    } else if let Some(detail) = &mut self.detail {
                        if let Some(id) = detail.edit_on_load.take() {
                            self.word_edit = rows
                                .iter()
                                .find(|row| row.id == id)
                                .cloned()
                                .map(|row| (row, self.folder));
                        }
                        detail.rows = rows;
                    }
                }
                Ok(Output::CreatedFolder(id)) => {
                    self.query.clear();
                    self.folder = id;
                    self.offset = 0;
                    self.folder_edit = None;
                    self.back();
                }
                Ok(Output::Updated { folder, entry }) => {
                    self.folder = folder;
                    if let Some(detail) = &mut self.detail {
                        detail.source = entry.source;
                        detail.source_lang = entry.source_lang;
                    }
                    self.word_edit = None;
                    self.reload = true;
                }
                Ok(_) if mutation => {
                    self.folder_edit = None;
                    self.word_edit = None;
                    self.reload = true;
                }
                Err(error) => {
                    self.error = Some(error);
                    self.reload = false;
                }
                _ => {}
            }
        }
        if self.reload && self.pending.is_none() {
            self.reload = false;
            let action = if let Some(detail) = &self.detail {
                Action::Detail {
                    folder: self.folder,
                    source: detail.source.clone(),
                    source_lang: detail.source_lang.clone(),
                }
            } else {
                Action::Page {
                    folder: self.folder,
                    query: self.query.clone(),
                    offset: self.offset,
                }
            };
            self.request(action, archive, ctx);
        }
    }
    fn back(&mut self) {
        self.detail = None;
        self.word_edit = None;
        self.reload = true;
    }
    fn open(&mut self, entry: &Entry, edit: Option<i64>) {
        self.detail = Some(Detail {
            source: entry.source.clone(),
            source_lang: entry.source_lang.clone(),
            rows: Vec::new(),
            edit_on_load: edit,
        });
        self.word_edit = None;
        self.reload = true;
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        archive: &Archive,
        language: UiLanguage,
        capabilities: LanguageCapabilities,
        ask: &mut ask::Ask,
    ) {
        self.poll(archive, ui.ctx());
        let width = ui
            .available_width()
            .min(if self.detail.is_some() { 860.0 } else { 980.0 });
        let inset = ((ui.available_width() - width) * 0.5).max(0.0);
        ui.horizontal_top(|ui| {
            ui.add_space(inset);
            ui.vertical(|ui| {
                ui.set_width(width);
                if self.detail.is_some() {
                    self.show_detail(ui, archive, language, capabilities, ask);
                } else {
                    self.show_list(ui, archive, language);
                }
                if let Some(error) = &self.error {
                    ui.add_space(12.0);
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        });
        if self.reload {
            ui.ctx().request_repaint();
        }
    }
    fn show_list(&mut self, ui: &mut egui::Ui, archive: &Archive, language: UiLanguage) {
        let busy = self.pending.is_some();
        let mut action = None;
        ui.label(
            egui::RichText::new(tr(language, "Vocabulary"))
                .size(28.0)
                .color(theme::text_strong()),
        );
        ui.add_space(18.0);
        ui.horizontal_wrapped(|ui| {
            let old = self.folder;
            let label = self
                .folders
                .iter()
                .find(|folder| folder.id == self.folder)
                .map(|folder| folder.label(language))
                .unwrap_or(tr(language, "Default wordbook"));
            egui::ComboBox::from_id_salt("vocabulary_folder")
                .selected_text(label)
                .width(ui.available_width().min(220.0))
                .show_ui(ui, |ui| {
                    for folder in &self.folders {
                        ui.selectable_value(&mut self.folder, folder.id, folder.label(language));
                    }
                });
            if self.folder != old {
                self.offset = 0;
                self.records.clear();
                self.folder_edit = None;
                self.reload = true;
            }
            ui.add_enabled_ui(!busy, |ui| {
                if components::add_button(ui, tr(language, "New wordbook")).clicked() {
                    self.folder_edit = Some((None, String::new()));
                }
                if components::edit_button(ui, tr(language, "Rename wordbook")).clicked() {
                    self.folder_edit = Some((
                        Some(self.folder),
                        self.folders
                            .iter()
                            .find(|f| f.id == self.folder)
                            .map(|f| f.label(language).to_owned())
                            .unwrap_or_default(),
                    ));
                }
                ui.add_enabled_ui(self.folder != DEFAULT_FOLDER, |ui| {
                    if components::delete_button(ui, tr(language, "Delete wordbook"))
                        .on_hover_text(tr(language, "Words will move to the default wordbook."))
                        .clicked()
                    {
                        action = Some(Action::DeleteFolder(self.folder));
                    }
                });
            });
        });
        if let Some((id, name)) = &mut self.folder_edit {
            let mut cancel = false;
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(tr(language, "Wordbook name"))
                        .desired_width(ui.available_width().min(300.0)),
                );
                let duplicate = self.folders.iter().any(|folder| {
                    Some(folder.id) != *id
                        && folder.label(language).to_lowercase() == name.trim().to_lowercase()
                });
                if ui
                    .add_enabled(
                        !busy && !name.trim().is_empty() && !duplicate,
                        egui::Button::new(tr(language, "Save")),
                    )
                    .clicked()
                {
                    action = Some(match *id {
                        Some(id) => Action::RenameFolder {
                            id,
                            name: name.trim().to_owned(),
                        },
                        None => Action::CreateFolder(name.trim().to_owned()),
                    });
                }
                cancel = ui.button(tr(language, "Cancel")).clicked();
            });
            if cancel {
                self.folder_edit = None;
            }
        }
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(tr(language, "Search"))
                        .desired_width(ui.available_width().min(360.0)),
                )
                .changed()
            {
                self.offset = 0;
                self.reload = true;
            }
            if busy {
                ui.spinner();
            }
        });
        ui.add_space(20.0);
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("vocabulary_words")
            .max_height((ui.available_height() - 42.0).max(120.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.records.is_empty() && !busy {
                    ui.weak(tr(language, "No saved words in this wordbook."));
                }
                for record in &self.records {
                    ui.push_id(record.id, |ui| {
                        egui::Frame::new()
                            .inner_margin(16)
                            .corner_radius(16)
                            .fill(theme::surface_subtle())
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let text_width = (ui.available_width()
                                        - 96.0
                                        - ui.spacing().item_spacing.x * 3.0)
                                        .max(0.0);
                                    let words = ui.allocate_ui_with_layout(
                                        egui::vec2(text_width, 48.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_width(text_width);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&record.entry.source)
                                                        .size(19.0)
                                                        .color(theme::text_strong()),
                                                )
                                                .wrap(),
                                            );
                                            let preview = if record.languages > 1 {
                                                format!(
                                                    "{}  ·  +{}",
                                                    record.entry.translated,
                                                    record.languages - 1
                                                )
                                            } else {
                                                record.entry.translated.clone()
                                            };
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(preview)
                                                        .color(theme::text_weak()),
                                                )
                                                .wrap(),
                                            );
                                        },
                                    );
                                    let response = ui
                                        .interact(
                                            words.response.rect,
                                            ui.id().with("open_word"),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if crate::ui::automation::button_response(
                                        ui,
                                        response.id,
                                        &record.entry.source,
                                        true,
                                        response,
                                    )
                                    .clicked()
                                    {
                                        open = Some((record.entry.clone(), None));
                                    }
                                    if components::copy_button(ui, tr(language, "Copy")).clicked() {
                                        ui.ctx().copy_text(record.entry.translated.clone());
                                    }
                                    ui.add_enabled_ui(!busy, |ui| {
                                        if components::edit_button(ui, tr(language, "Edit"))
                                            .clicked()
                                        {
                                            open = Some((record.entry.clone(), Some(record.id)));
                                        }
                                        if components::delete_button(
                                            ui,
                                            tr(language, "Remove from vocabulary"),
                                        )
                                        .clicked()
                                        {
                                            action = Some(Action::RemoveWord {
                                                folder: self.folder,
                                                source: record.entry.source.clone(),
                                                source_lang: record.entry.source_lang.clone(),
                                            });
                                        }
                                    });
                                });
                            });
                        ui.add_space(10.0);
                    });
                }
            });
        if let Some((entry, edit)) = open {
            self.open(&entry, edit);
        }
        if self.offset > 0 || self.more {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled_ui(!busy && self.offset > 0, |ui| {
                        components::back_button(ui, tr(language, "Previous"))
                    })
                    .inner
                    .clicked()
                {
                    self.offset = self.offset.saturating_sub(PAGE_SIZE);
                    self.reload = true;
                }
                ui.label((self.offset / PAGE_SIZE + 1).to_string());
                if ui
                    .add_enabled_ui(!busy && self.more, |ui| {
                        components::next_button(ui, tr(language, "Next"))
                    })
                    .inner
                    .clicked()
                {
                    self.offset += PAGE_SIZE;
                    self.reload = true;
                }
            });
        }
        if let Some(action) = action {
            self.request(action, archive, ui.ctx());
        }
    }
}
