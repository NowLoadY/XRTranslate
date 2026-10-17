use super::{
    Page,
    examples::{self, Step},
    more_button,
};
use crate::{
    ask,
    history::archive::{
        Archive,
        vocabulary::{Action, Translation},
    },
    i18n::{UiLanguage, tr, tr_dynamic},
    ui::{components, theme},
};
use eframe::egui;
use xrtranslate_engine::language::{LanguageCapabilities, SupportedLanguage};

impl Page {
    pub(super) fn show_detail(
        &mut self,
        ui: &mut egui::Ui,
        archive: &Archive,
        language: UiLanguage,
        capabilities: LanguageCapabilities,
        ask: &mut ask::Ask,
    ) {
        let detail = self.detail.take().unwrap();
        let mut back = false;
        ui.horizontal(|ui| {
            back = components::back_button(ui, tr(language, "Back to vocabulary")).clicked();
            ui.weak(tr(language, "Vocabulary"));
            if self.pending.is_some() {
                ui.spinner();
            }
        });
        ui.add_space(24.0);
        language_label(ui, &detail.source_lang, language);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&detail.source)
                    .size(32.0)
                    .color(theme::text_strong()),
            )
            .wrap(),
        );
        ui.add_space(28.0);
        let busy = self.pending.is_some();
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("vocabulary_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in &detail.rows {
                    ui.push_id(row.id, |ui| {
                        egui::Frame::new()
                            .inner_margin(20)
                            .corner_radius(18)
                            .fill(theme::surface_subtle())
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                language_label(ui, &row.entry.target_lang, language);
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let width = (ui.available_width()
                                        - 64.0
                                        - ui.spacing().item_spacing.x * 2.0)
                                        .max(0.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(width, 32.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_width(width);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&row.entry.translated)
                                                        .size(23.0)
                                                        .color(theme::text_strong()),
                                                )
                                                .wrap(),
                                            );
                                        },
                                    );
                                    if components::copy_button(ui, tr(language, "Copy translation"))
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(row.entry.translated.clone());
                                    }
                                    ui.add_enabled_ui(!busy, |ui| {
                                        let more = more_button(ui, language);
                                        egui::Popup::menu(&more).show(|ui| {
                                            if ui.button(tr(language, "Edit translation")).clicked()
                                            {
                                                self.word_edit = Some((row.clone(), self.folder));
                                                ui.close();
                                            }
                                            if ui
                                                .button(tr(language, "Remove this translation"))
                                                .clicked()
                                            {
                                                action = Some(Action::Remove(row.id));
                                                ui.close();
                                            }
                                        });
                                    });
                                });
                                if self
                                    .word_edit
                                    .as_ref()
                                    .is_some_and(|(edit, _)| edit.id == row.id)
                                {
                                    ui.add_space(18.0);
                                    if let Some(update) = self.show_editor(ui, language, busy) {
                                        action = Some(update);
                                    }
                                } else {
                                    self.show_example(ui, row, language, capabilities, ask, busy);
                                }
                            });
                        ui.add_space(18.0);
                    });
                }
            });
        self.detail = Some(detail);
        if back {
            self.back();
        }
        if let Some(action) = action {
            self.request(action, archive, ui.ctx());
        }
    }

    fn show_example(
        &mut self,
        ui: &mut egui::Ui,
        row: &Translation,
        language: UiLanguage,
        capabilities: LanguageCapabilities,
        ask: &mut ask::Ask,
        busy: bool,
    ) {
        let example = &row.example;
        if !example.sentence.is_empty() {
            ui.add_space(22.0);
            ui.weak(tr(language, "Example sentence"));
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new(&example.sentence).size(18.0)).wrap());
        }
        if !example.back_translation.is_empty() {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&example.back_translation)
                        .size(17.0)
                        .color(theme::text_weak()),
                )
                .wrap(),
            );
        }
        let running = self.examples.busy(&row.entry);
        let supported = examples::supported(&row.entry, capabilities);
        ui.add_space(18.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!busy && !running && supported, |ui| {
                if components::icon_button(
                    ui,
                    tr(
                        language,
                        if example.sentence.is_empty() {
                            "Generate example"
                        } else {
                            "Generate another example"
                        },
                    ),
                    egui::include_image!("../../../resources/icons/example.svg"),
                )
                .clicked()
                {
                    self.examples
                        .start(row, Step::Sentence, capabilities, ask, ui.ctx());
                }
                if !example.sentence.is_empty()
                    && example.back_translation.is_empty()
                    && components::icon_button(
                        ui,
                        tr(language, "View example translation"),
                        egui::include_image!("../../../resources/icons/translation.svg"),
                    )
                    .clicked()
                {
                    self.examples
                        .start(row, Step::Translation, capabilities, ask, ui.ctx());
                }
            });
            if running {
                ui.spinner();
            }
        });
        if !supported {
            ui.add_space(8.0);
            ui.weak(tr(
                language,
                "The current model does not support this language pair.",
            ));
        }
    }

    fn show_editor(
        &mut self,
        ui: &mut egui::Ui,
        language: UiLanguage,
        busy: bool,
    ) -> Option<Action> {
        let (row, folder) = self.word_edit.as_mut()?;
        let mut action = None;
        ui.weak(tr(language, "Original"));
        ui.add(
            egui::TextEdit::multiline(&mut row.entry.source)
                .desired_rows(1)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(10.0);
        ui.weak(tr(language, "Translation"));
        ui.add(
            egui::TextEdit::multiline(&mut row.entry.translated)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(12.0);
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            let label = self
                .folders
                .iter()
                .find(|item| item.id == *folder)
                .map(|item| item.label(language))
                .unwrap_or(tr(language, "Default wordbook"));
            egui::ComboBox::from_id_salt("edit_word_folder")
                .selected_text(label)
                .width(ui.available_width().min(220.0))
                .show_ui(ui, |ui| {
                    for item in &self.folders {
                        ui.selectable_value(folder, item.id, item.label(language));
                    }
                });
            let valid =
                !row.entry.source.trim().is_empty() && !row.entry.translated.trim().is_empty();
            if components::secondary_button_enabled(ui, tr(language, "Save"), !busy && valid)
                .clicked()
            {
                action = Some(Action::Update {
                    id: row.id,
                    folder: *folder,
                    entry: row.entry.clone(),
                });
            }
            cancel =
                components::secondary_button_enabled(ui, tr(language, "Cancel"), !busy).clicked();
        });
        if cancel {
            self.word_edit = None;
        }
        action
    }
}

fn language_label(ui: &mut egui::Ui, tag: &str, language: UiLanguage) {
    let name = SupportedLanguage::parse(tag)
        .map(|value| value.name())
        .unwrap_or(tag);
    ui.label(
        egui::RichText::new(tr_dynamic(language, name))
            .size(12.0)
            .color(theme::text_weak()),
    );
}
