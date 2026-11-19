use super::*;

fn switch(
    controller: &mut CorpusStudioController,
    domain: &GraphDomain,
    ui: &mut egui::Ui,
    language: UiLanguage,
    label: bool,
) {
    ui.push_id(&domain.id, |ui| {
        let mut enabled = controller.displayed_domain_enabled(domain);
        let response = ui
            .add_enabled_ui(!controller.pending, |ui| {
                if label {
                    components::toggle_with_label(
                        ui,
                        &mut enabled,
                        tr(language, "Use for speech recognition"),
                    )
                } else {
                    components::pill_toggle(ui, &mut enabled)
                }
            })
            .inner;
        if response
            .on_hover_text(tr(language, "Use for speech recognition"))
            .changed()
        {
            controller.send(Command::PutDomain(GraphDomain {
                enabled,
                ..domain.clone()
            }));
        }
    });
}

pub(super) fn render(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    domain: &GraphDomain,
    ui: &mut egui::Ui,
    language: UiLanguage,
) {
    let root = domain.id == WORDBOOK_DOMAIN_ID;
    ui.horizontal_wrapped(|ui| {
        if components::back_button(
            ui,
            tr(language, if root { "All domains" } else { "Vocabulary" }),
        )
        .clicked()
        {
            let parent = domain
                .parent_id
                .as_ref()
                .and_then(|id| snapshot.domains.iter().find(|d| d.id == *id));
            controller.select_domain(parent);
            controller.node_search.clear();
        }
        ui.strong(domain_label(domain, language).as_ref());
        switch(controller, domain, ui, language, true);
    });
    if root {
        let mut books: Vec<_> = snapshot
            .domains
            .iter()
            .filter(|d| d.parent_id.as_deref() == Some(WORDBOOK_DOMAIN_ID))
            .collect();
        ui.weak(tr(
            language,
            if books.is_empty() {
                "Save or import words to see your wordbooks here."
            } else if !controller.displayed_domain_enabled(domain) {
                "Vocabulary recognition is paused."
            } else {
                "Choose the wordbooks to use as recognition hints."
            },
        ));
        books.sort_by_key(|d| (&d.title, &d.id));
        egui::ScrollArea::vertical()
            .id_salt("recognition_wordbooks")
            .show(ui, |ui| {
                for book in books {
                    ui.horizontal(|ui| {
                        switch(controller, book, ui, language, false);
                        if components::button(ui, domain_label(book, language).as_ref()).clicked() {
                            controller.select_domain(Some(book));
                            controller.node_search.clear();
                        }
                        ui.weak(format!(
                            "{} {}",
                            controller
                                .term_index
                                .counts
                                .get(&book.id)
                                .map_or(0, |c| c.0),
                            tr(language, "terms")
                        ));
                    });
                }
            });
    } else {
        if let Some(parent) = snapshot.domains.iter().find(|d| d.id == WORDBOOK_DOMAIN_ID)
            && !controller.displayed_domain_enabled(parent)
        {
            ui.weak(tr(language, "Vocabulary recognition is paused."));
        }
        ui.add(
            egui::TextEdit::singleline(&mut controller.node_search)
                .hint_text(tr(language, "Search terms")),
        );
        let rows = controller
            .term_index
            .rows(snapshot, Some(&domain.id), &controller.node_search);
        if rows.is_empty() {
            ui.weak(tr(language, "No matching terms"));
        }
        egui::ScrollArea::vertical()
            .id_salt(("recognition_words", &domain.id))
            .show_rows(ui, 27.0, rows.len(), |ui, range| {
                for &index in &rows[range] {
                    let node = &snapshot.nodes[index];
                    ui.horizontal(|ui| {
                        if components::button(ui, &node.title).clicked() {
                            controller.select_node(node);
                        }
                        let translations = node
                            .values
                            .iter()
                            .filter(|v| !v.is_empty() && *v != &node.title)
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.add(egui::Label::new(RichText::new(translations).weak()).truncate());
                    });
                }
            });
    }
}
