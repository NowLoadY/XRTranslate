use crate::{
    history::archive::{Archive, DeleteSelection, PAGE_SIZE},
    i18n::{UiLanguage, tr},
    ui::components,
};
use eframe::egui;

pub(super) fn render(app: &mut crate::XRTranslateApp, ui: &mut egui::Ui) {
    let language = app.ui_language;
    crate::ui::layout::flow_row(ui, |ui| {
        ui.heading(tr(language, "Translation history"));
        if components::secondary_button(ui, tr(language, "Back to translation")).clicked() {
            app.history_archive.cancel_delete();
            app.history_archive.open = false;
        }
    });
    ui.add_space(12.0);
    let mut save = app.history_archive.enabled();
    if components::toggle_with_label(
        ui,
        &mut save,
        tr(language, "Save translation history locally"),
    )
    .changed()
    {
        app.history_archive.set_enabled(save);
        app.save_settings();
    }
    ui.weak(tr(
        language,
        "Saving starts when enabled. Turning it off keeps existing records.",
    ));
    ui.add_space(10.0);
    let archive = &mut app.history_archive;
    if !archive.saved {
        ui.weak(tr(
            language,
            "The latest 1,000 translations from this session.",
        ));
    }
    crate::ui::layout::flow_row(ui, |ui| {
        if components::segmented_switch(
            ui,
            &mut archive.saved,
            [tr(language, "This session"), tr(language, "Saved")],
        )
        .changed()
        {
            archive.offset = 0;
            archive.refresh(false);
        }
        if components::secondary_button(ui, tr(language, "Refresh")).clicked() {
            archive.refresh(false);
        }
        if components::secondary_button_enabled(
            ui,
            tr(language, "Delete by date"),
            !archive.deleting(),
        )
        .clicked()
        {
            archive.date_delete_open = !archive.date_delete_open;
        }
    });
    if archive.date_delete_open {
        render_date_deletion(archive, ui, language);
        ui.add_space(8.0);
    }
    let before = archive.query.clone();
    components::search_bar(
        ui,
        &mut archive.query,
        tr(language, "Search original text or translation"),
    );
    if before != archive.query {
        archive.offset = 0;
        archive.refresh(true);
    }
    {
        crate::ui::layout::flow_row(ui, |ui| {
            if components::secondary_button_enabled(
                ui,
                tr(language, "Previous"),
                archive.offset > 0 && !archive.loading,
            )
            .clicked()
            {
                archive.offset = archive.offset.saturating_sub(PAGE_SIZE);
                archive.refresh(false);
            }
            ui.weak(format!("{}", archive.offset / PAGE_SIZE + 1));
            if components::secondary_button_enabled(
                ui,
                tr(language, "Next"),
                archive.has_more && !archive.loading,
            )
            .clicked()
            {
                archive.offset += PAGE_SIZE;
                archive.refresh(false);
            }
            if archive.loading {
                ui.spinner();
            }
            if archive.deleting() {
                ui.spinner();
                ui.weak(tr(language, "Deleting history…"));
            } else if let Some(count) = archive.deleted_count {
                ui.weak(
                    tr(language, "Deleted {count} translations.")
                        .replace("{count}", &count.to_string()),
                );
            }
        });
    }
    let records = &archive.rows;
    let mut delete = None;
    if records.is_empty() && !archive.loading {
        ui.weak(tr(language, "No matching translations"));
    }
    crate::ui::layout::show_variable_virtual_rows(
        ui,
        "translation_archive",
        8.0,
        false,
        |ui| {
            records
                .iter()
                .map(|record| {
                    let galley = ui.painter().layout(
                        format!("{}\n{}", record.source, record.translated),
                        egui::FontId::proportional(14.0),
                        crate::ui::theme::text_strong(),
                        (ui.available_width() - 32.0).max(20.0),
                    );
                    (galley.size().y + 74.0, (record, galley))
                })
                .collect::<Vec<_>>()
        },
        |ui, _, _, (record, galley)| {
            components::card(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::ui::layout::flow_row(ui, |ui| {
                    ui.weak(&record.timestamp);
                    if components::secondary_button(ui, tr(language, "Copy")).clicked() {
                        ui.ctx().copy_text(record.translated.clone());
                    }
                    if components::secondary_button_enabled(
                        ui,
                        tr(language, "Delete"),
                        !archive.deleting(),
                    )
                    .clicked()
                    {
                        let source: String = record.source.chars().take(100).collect();
                        delete = Some((
                            DeleteSelection::Record(record.id.clone()),
                            format!("{}\n{source}", record.timestamp),
                        ));
                    }
                });
                ui.add(egui::Label::new(galley.clone()).selectable(true));
            });
        },
    );
    if let Some((selection, description)) = delete {
        archive.prepare_delete(selection, description, ui.ctx());
    }
    render_delete_confirmation(archive, ui, language);
}

fn render_date_deletion(archive: &mut Archive, ui: &mut egui::Ui, language: UiLanguage) {
    components::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(tr(
            language,
            "Delete all history in this date range, including both days, using local time.",
        ));
        crate::ui::layout::responsive_columns(ui, 2, 170.0, |ui, index| {
            let (label, date) = if index == 0 {
                ("Start date", &mut archive.date_from)
            } else {
                ("End date", &mut archive.date_through)
            };
            ui.label(tr(language, label));
            components::input_field(ui, date, &format!("{} (YYYY-MM-DD)", tr(language, label)));
        });
        ui.add_space(8.0);
        if components::secondary_button_enabled(
            ui,
            tr(language, "Review deletion"),
            !archive.deleting()
                && !archive.date_from.trim().is_empty()
                && !archive.date_through.trim().is_empty(),
        )
        .clicked()
        {
            let from = archive.date_from.trim().to_owned();
            let through = archive.date_through.trim().to_owned();
            let description = format!(
                "{from} — {through}\n{}",
                tr(
                    language,
                    "All translations in this date range, regardless of the search keyword."
                )
            );
            archive.prepare_delete(
                DeleteSelection::Dates { from, through },
                description,
                ui.ctx(),
            );
        }
    });
}

fn render_delete_confirmation(archive: &mut Archive, ui: &mut egui::Ui, language: UiLanguage) {
    let Some(pending) = &archive.pending_delete else {
        return;
    };
    let status = if let Some(error) = &pending.error {
        crate::i18n::tr_dynamic(language, error).into_owned()
    } else if let Some(count) = pending.count {
        tr(language, "{count} translations will be deleted.").replace("{count}", &count.to_string())
    } else {
        tr(language, "Counting matching translations…").to_owned()
    };
    let message = format!(
        "{}\n\n{status}\n\n{}",
        pending.description,
        tr(
            language,
            "Removes matching entries from this session and saved history. Media and meeting tasks are kept. This cannot be undone."
        )
    );
    if let Some(confirmed) = crate::ui::modal::confirm(
        ui.ctx(),
        ui.make_persistent_id("history_delete_confirmation"),
        language,
        tr(language, "Delete translation history?"),
        &message,
        tr(language, "Confirm delete"),
        pending.error.is_none() && pending.count.is_some_and(|count| count > 0),
    ) {
        if confirmed {
            archive.confirm_delete(ui.ctx());
        } else {
            archive.cancel_delete();
        }
    }
}
