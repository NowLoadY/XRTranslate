//! Shared entry surface: start an action first, revisit recent work below it.
use super::*;
use crate::i18n::{UiLanguage, tr};

pub(crate) fn library_start(
    ui: &mut Ui,
    icon: egui::ImageSource<'_>,
    title: &str,
    description: &str,
    actions: impl FnOnce(&mut Ui),
) {
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_space(8.0);
        ui.horizontal_top(|ui| {
            ui.add(
                egui::Image::new(icon)
                    .fit_to_exact_size(Vec2::splat(32.0))
                    .tint(theme::text_strong()),
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                ui.add(egui::Label::new(egui::RichText::new(title).size(21.0).strong()).wrap());
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(description).color(theme::text_weak()))
                        .wrap(),
                );
                ui.add_space(12.0);
                ui.horizontal_wrapped(actions);
            });
        });
        ui.add_space(8.0);
    });
    ui.add_space(16.0);
}

pub(crate) fn library_navigation(
    ui: &mut Ui,
    language: UiLanguage,
    expanded: &mut bool,
    query: &mut String,
    count: usize,
    search_hint: &str,
) -> usize {
    ui.horizontal_wrapped(|ui| {
        ui.strong(tr(
            language,
            if *expanded {
                "All items"
            } else {
                "Recent items"
            },
        ));
        if count > 3
            && secondary_button(
                ui,
                tr(language, if *expanded { "Show recent" } else { "View all" }),
            )
            .clicked()
        {
            *expanded = !*expanded;
        }
    });
    search_bar(ui, query, search_hint);
    ui.add_space(10.0);
    if *expanded || !query.trim().is_empty() {
        usize::MAX
    } else {
        3
    }
}
