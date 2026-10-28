//! Paste a column of words or two spreadsheet columns into one domain.
use super::*;

pub(super) struct Draft {
    domain: String,
    search: String,
    languages: [usize; 2],
    text: String,
    dirty: bool,
    nodes: Vec<GraphNode>,
    duplicates: usize,
    invalid_lines: Vec<usize>,
}

impl Draft {
    pub fn new(domain: Option<String>, language: UiLanguage) -> Self {
        let primary = preferred_language_index(language);
        Self {
            domain: domain.unwrap_or_default(),
            search: String::new(),
            languages: [primary, if primary == 1 { 0 } else { 1 }],
            text: String::new(),
            dirty: true,
            nodes: Vec::new(),
            duplicates: 0,
            invalid_lines: Vec::new(),
        }
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn count(&self) -> usize {
        self.nodes.len()
    }

    fn prepare(&mut self, snapshot: &GraphSnapshot) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.nodes.clear();
        self.invalid_lines.clear();
        self.duplicates = 0;
        let [a, b] = self.languages;
        let mut pairs = HashSet::new();
        let mut words = HashSet::new();
        for node in snapshot
            .nodes
            .iter()
            .filter(|node| node.promptable && node.domain_id == self.domain)
        {
            let word = node.values[a].trim().to_lowercase();
            words.insert(word.clone());
            pairs.insert((word, node.values[b].trim().to_lowercase()));
        }
        for (line, text) in self.text.lines().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let columns: Vec<_> = text.split('\t').map(str::trim).collect();
            if columns.len() > 2
                || columns[0].is_empty()
                || columns
                    .iter()
                    .any(|s| s.contains(',') || s.chars().count() > 512)
            {
                self.invalid_lines.push(line + 1);
                continue;
            }
            let original = columns[0];
            let translated = columns.get(1).copied().unwrap_or("");
            let key = (original.to_lowercase(), translated.to_lowercase());
            if (translated.is_empty() && words.contains(&key.0)) || !pairs.insert(key.clone()) {
                self.duplicates += 1;
                continue;
            }
            words.insert(key.0);
            let mut values = vec![String::new(); LANGUAGE_CODES.len()];
            values[a] = original.to_owned();
            values[b] = translated.to_owned();
            self.nodes.push(GraphNode {
                id: uuid::Uuid::new_v4().to_string(),
                domain_id: self.domain.clone(),
                title: original.chars().take(256).collect(),
                enabled: true,
                promptable: true,
                activation: CorpusActivation::Always,
                inherit_domain: true,
                recognition_aliases: Default::default(),
                priority: 20,
                values,
            });
        }
    }
}

pub(super) fn render(
    controller: &mut CorpusStudioController,
    ctx: &egui::Context,
    language: UiLanguage,
) {
    let Some(mut draft) = controller.import.take() else {
        return;
    };
    let Some(snapshot) = controller.snapshot.clone() else {
        return;
    };
    let mut save = false;
    let ready = std::cell::Cell::new(false);
    let close = crate::ui::modal::dialog(
        ctx,
        egui::Id::unique("import_terms"),
        language,
        tr(language, "Paste terms"),
        |ui| {
            ui.add_enabled_ui(!controller.pending, |ui| {
                if let Some(error) = &controller.error { components::error_notice(ui, language, error); }
                ui.label(tr(language, "Choose a domain, then paste one word per line or two columns from a spreadsheet."));
                draft.dirty |= domains::picker(ui, &snapshot, language, &mut draft.domain, &mut draft.search);
                ui.weak(domain_rules::summary(&snapshot, &draft.domain, language));
                ui.horizontal_wrapped(|ui| {
                    for position in 0..2 {
                        egui::ComboBox::from_id_salt(("import_language", position))
                            .selected_text(editor::language_name(language, draft.languages[position])).show_ui(ui, |ui| {
                                for index in 0..LANGUAGE_CODES.len() {
                                    if index != draft.languages[1-position] {
                                        draft.dirty |= ui.selectable_value(&mut draft.languages[position], index,
                                            editor::language_name(language, index)).changed();
                                    }
                                }
                            });
                    }
                });
                draft.dirty |= ui.add(egui::TextEdit::multiline(&mut draft.text).desired_rows(8)
                    .desired_width(ui.available_width()).hint_text(tr(language, "Word [Tab] Translation"))).changed();
                draft.prepare(&snapshot);
                ui.label(format!("{} {} · {} {}", draft.nodes.len(), tr(language, "New terms"),
                    draft.duplicates, tr(language, "Duplicates skipped")));
                if !draft.invalid_lines.is_empty() {
                    ui.colored_label(graph_style::ERROR_BORDER, format!("{}: {}",
                        tr(language, "Check these lines (one or two columns, no commas)"),
                        draft.invalid_lines.iter().take(8).map(usize::to_string).collect::<Vec<_>>().join(", ")));
                }
                ui.weak(tr(language, "All imported terms follow the domain rules. Existing translations are kept."));
                ready.set(!draft.nodes.is_empty() && draft.invalid_lines.is_empty()
                    && snapshot.domains.iter().any(|d| d.id == draft.domain));
                for node in draft.nodes.iter().take(4) {
                    ui.small(format!("{} → {}", node.values[draft.languages[0]], node.values[draft.languages[1]]));
                }
            });
        },
        |ui| {
            save = components::primary_button_enabled(
                ui,
                tr(language, "Import terms"),
                ready.get() && !controller.pending,
            )
            .clicked();
        },
    );
    if save {
        controller.send(Command::ImportTerms(draft.nodes.clone()));
    }
    if !close || controller.pending {
        controller.import = Some(draft);
    }
}
