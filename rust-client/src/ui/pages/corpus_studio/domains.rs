//! Searchable domain choices shared by term editing and bulk moves.
use super::*;

pub(super) fn choices(
    ui: &mut egui::Ui,
    snapshot: &GraphSnapshot,
    language: UiLanguage,
    search: &mut String,
) -> Option<String> {
    ui.set_max_width(340.0_f32.min(ui.ctx().content_rect().width() - 40.0));
    ui.add(
        egui::TextEdit::singleline(search)
            .desired_width(ui.available_width())
            .hint_text(tr(language, "Search domains")),
    );
    let mut domains: Vec<_> = snapshot
        .domains
        .iter()
        .map(|domain| (domain, domain_path_label(snapshot, &domain.id, language)))
        .filter(|(domain, path)| contains(path, search) || contains(&domain.id, search))
        .collect();
    domains.sort_by(|a, b| a.1.cmp(&b.1));
    let mut choice = None;
    egui::ScrollArea::vertical()
        .id_salt("domain_choices")
        .max_height(240.0)
        .show(ui, |ui| {
            for (domain, path) in &domains {
                if ui
                    .add(egui::Button::new(path).frame(false).wrap())
                    .clicked()
                {
                    choice = Some(domain.id.clone());
                    ui.close();
                }
            }
            if domains.is_empty() {
                ui.weak(tr(language, "No matching domains"));
            }
        });
    choice
}

pub(super) fn picker(
    ui: &mut egui::Ui,
    snapshot: &GraphSnapshot,
    language: UiLanguage,
    selected: &mut String,
    search: &mut String,
) -> bool {
    let label = if selected.is_empty() {
        tr(language, "Choose a domain").to_owned()
    } else {
        domain_path_label(snapshot, selected, language)
    };
    let mut changed = false;
    egui::menu::MenuButton::new(label)
        .config(
            egui::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
        )
        .ui(ui, |ui| {
            if let Some(id) = choices(ui, snapshot, language, search) {
                changed = *selected != id;
                *selected = id;
                search.clear();
            }
        });
    changed
}
