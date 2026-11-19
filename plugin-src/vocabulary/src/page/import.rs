use super::{Action, Archive, Capabilities, Page, UiLanguage, components, egui, tr};
use xrtranslate_client_core::{bookmarks::import_rows, file_dialog::FileDialog};
use xrtranslate_engine::language::{LANGUAGES, SupportedLanguage};

pub(super) struct Draft {
    text: String,
    languages: [SupportedLanguage; 2],
    count: usize,
    invalid_line: Option<usize>,
}

impl Draft {
    pub(super) fn new(cap: &Capabilities) -> Self {
        let en = SupportedLanguage::parse("en").unwrap();
        let zh = SupportedLanguage::parse("zh").unwrap();
        let target = cap
            .target_language
            .split(',')
            .find_map(SupportedLanguage::parse)
            .unwrap_or(en);
        let source = SupportedLanguage::parse(&cap.source_language)
            .filter(|lang| lang.base_code() != target.base_code())
            .unwrap_or(if target == en { zh } else { en });
        Self {
            text: String::new(),
            languages: [source, target],
            count: 0,
            invalid_line: None,
        }
    }
    fn preview(&mut self) {
        self.count = 0;
        self.invalid_line = None;
        for row in import_rows(&self.text) {
            match row {
                Ok(_) => self.count += 1,
                Err(line) => {
                    self.invalid_line = Some(line);
                    break;
                }
            }
        }
    }
}

impl Page {
    pub(super) fn show_import(
        &mut self,
        ctx: &egui::Context,
        archive: &Archive,
        language: UiLanguage,
    ) {
        let Some(mut draft) = self.import.take() else {
            return;
        };
        let busy = self.pending.is_some();
        let mut submit = false;
        let ready = std::cell::Cell::new(false);
        let close = crate::ui::modal::dialog(
            ctx,
            egui::Id::unique("wordbook_import"),
            language,
            tr(language, "Import words"),
            |ui| {
                if let Some(folder) = self.folders.iter().find(|f| f.id == self.folder) {
                    ui.strong(folder.label(language));
                }
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (index, label) in ["Source language", "Target language"]
                            .into_iter()
                            .enumerate()
                        {
                            ui.label(tr(language, label));
                            let name =
                                crate::language_label(language, draft.languages[index].code());
                            components::combobox_ui(
                                ui,
                                ("wordbook_import_lang", index),
                                name,
                                |ui| {
                                    for &lang in LANGUAGES {
                                        if lang.base_code()
                                            != draft.languages[1 - index].base_code()
                                        {
                                            components::selectable_value(
                                                ui,
                                                &mut draft.languages[index],
                                                lang,
                                                crate::language_label(language, lang.code()),
                                            );
                                        }
                                    }
                                },
                            );
                        }
                    });
                    ui.label(tr(
                        language,
                        "One word per line. A second column can contain a translation.",
                    ));
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut draft.text)
                            .desired_rows(6)
                            .desired_width(ui.available_width())
                            .hint_text(tr(language, "Word [Tab] Translation")),
                    );
                    let mut changed = ui_director::egui::text_response(
                        ui,
                        tr(language, "Import words"),
                        &mut draft.text,
                        response,
                    )
                    .changed();
                    let pick = components::secondary_button(ui, tr(language, "Open TXT or TSV…"))
                        .clicked();
                    if let Some(path) = FileDialog::new()
                        .add_filter("TXT / TSV", &["txt", "tsv"])
                        .pick_file(ctx, "wordbook_import_file", pick)
                    {
                        match std::fs::read_to_string(path) {
                            Ok(text) => {
                                draft.text = text;
                                changed = true;
                            }
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                    if changed {
                        draft.preview();
                    }
                    if let Some(line) = draft.invalid_line {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("{} {line}", tr(language, "Check line")),
                        );
                    } else if draft.count > 0 {
                        ui.weak(format!("{} {}", draft.count, tr(language, "terms")));
                    }
                    if let Some(error) = &self.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    ready.set(draft.invalid_line.is_none() && draft.count > 0);
                });
            },
            |ui| {
                submit = components::primary_button_enabled(
                    ui,
                    tr(language, "Import words"),
                    !busy && ready.get(),
                )
                .clicked();
            },
        );
        if submit {
            self.request(
                Action::Import {
                    folder: self.folder,
                    source_lang: draft.languages[0].code().into(),
                    target_lang: draft.languages[1].code().into(),
                    text: draft.text.clone(),
                },
                archive,
                ctx,
            );
        }
        if !close || self.pending.is_some() {
            self.import = Some(draft);
        }
    }
}
