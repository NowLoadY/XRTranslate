//! One draft owns a term, its domain and incoming rules until the user saves.
use super::*;

#[derive(Clone, Default)]
pub(super) struct TermForm {
    pub incoming_edges: Vec<GraphEdge>,
    pub new_domain: Option<GraphDomain>,
    pub languages: Option<[usize; 2]>,
    pub focus_input: bool,
    domain_search: String,
    duplicate_query: Option<(String, Vec<String>, usize)>,
    duplicates: Vec<usize>,
    pub cue_search: String,
    pub context_search: String,
    pub aliases: aliases::Draft,
}

impl TermForm {
    fn matching_words(&mut self, node: &GraphNode, snapshot: &GraphSnapshot) -> &[usize] {
        let query = (
            node.domain_id.clone(),
            node.values
                .iter()
                .map(|s| s.trim().to_lowercase())
                .collect::<Vec<_>>(),
            snapshot.nodes.len(),
        );
        if self.duplicate_query.as_ref() != Some(&query) {
            let values: Vec<_> = query
                .1
                .iter()
                .enumerate()
                .filter(|(_, v)| !v.is_empty())
                .collect();
            self.duplicates = snapshot
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, other)| {
                    other.promptable && other.id != node.id
                        && other.domain_id == node.domain_id
                        && values.iter().any(|(i, value)| {
                            other
                                .values
                                .get(*i)
                                .is_some_and(|v| v.trim().to_lowercase() == **value)
                        })
                })
                .map(|(i, _)| i)
                .take(3)
                .collect();
            self.duplicate_query = Some(query);
        }
        &self.duplicates
    }

    pub fn load_rules(&mut self, snapshot: &GraphSnapshot, id: &str) {
        self.incoming_edges = rules::incoming(snapshot, id);
    }
}

enum Action {
    Save(bool),
    Discard,
    Close,
    Existing(String),
    Related,
}

impl CorpusStudioController {
    pub(super) fn edit_trigger(&mut self, snapshot: &GraphSnapshot, source: &str, target: &str) {
        if self.pending || self.draft_dirty || source == target {
            return;
        }
        let Some(node) = snapshot.nodes.iter().find(|node| node.id == target) else {
            return;
        };
        self.select_node(node);
        self.term_form
            .incoming_edges
            .retain(|edge| edge.kind != GraphEdgeKind::Trigger || edge.source_id != source);
        self.term_form.incoming_edges.push(GraphEdge {
            source_id: source.to_owned(),
            target_id: target.to_owned(),
            kind: GraphEdgeKind::Trigger,
            enabled: true,
        });
        self.node_draft.as_mut().unwrap().activation = CorpusActivation::OnEvidence;
        self.node_draft.as_mut().unwrap().inherit_domain = false;
        self.draft_dirty = true;
    }

    fn create_related(&mut self, source: &GraphNode) {
        self.create_node(Some(&source.domain_id));
        let Some(draft) = &mut self.node_draft else {
            return;
        };
        draft.activation = CorpusActivation::OnEvidence;
        draft.inherit_domain = false;
        self.term_form.incoming_edges.push(GraphEdge {
            source_id: source.id.clone(),
            target_id: draft.id.clone(),
            kind: GraphEdgeKind::Trigger,
            enabled: true,
        });
    }
}

pub(super) fn render_inspector(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let Some(mut node) = controller.node_draft.clone() else {
        return;
    };
    node.values.resize(LANGUAGE_CODES.len(), String::new());
    let mut form = controller.term_form.clone();
    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled_ui(!controller.pending && !controller.draft_dirty, |ui| {
                components::back_button(ui, tr(language, "Back"))
            })
            .inner
            .clicked()
        {
            action = Some(Action::Close);
        }
        components::section_heading(
            ui,
            tr(
                language,
                if controller.new_node {
                    "Add term"
                } else {
                    "Term details"
                },
            ),
        );
    });
    egui::ScrollArea::vertical().id_salt(("corpus_inspector", &node.id))
        .max_height((height - 36.0).max(80.0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!controller.pending, |ui| {
            let mut changed = false;
            ui.strong(tr(language, "Domain"));
            if let Some(domain) = &mut form.new_domain {
                ui.label(tr(language, "New domain"));
                changed |= ui.add(egui::TextEdit::singleline(&mut domain.title)
                    .id_salt("new_term_domain").desired_width(ui.available_width())
                    .hint_text(tr(language, "Domain name")).char_limit(256)).changed();
                let existing = snapshot.domains.iter().find(|existing|
                    existing.parent_id.is_none() && existing.title.trim().to_lowercase() == domain.title.trim().to_lowercase());
                if let Some(existing) = existing {
                    if ui.button(tr(language, "Use existing domain")).clicked() {
                        node.domain_id = existing.id.clone();
                        form.new_domain = None;
                        changed = true;
                    }
                } else {
                    ui.weak(tr(language, "The domain is created when you save this term."));
                }
                if ui.small_button(tr(language, "Choose existing domain")).clicked() {
                    form.new_domain = None;
                    changed = true;
                }
            } else {
                changed |= domains::picker(ui, snapshot, language, &mut node.domain_id, &mut form.domain_search);
                if ui.small_button(tr(language, "Create a domain")).clicked() {
                    form.new_domain = Some(GraphDomain {
                        id: uuid::Uuid::new_v4().to_string(), title: String::new(),
                        enabled: true, parent_id: None, cue_words: None,
                    });
                    changed = true;
                }
                if !node.domain_id.is_empty() && !domain_effective_enabled(controller, snapshot, &node.domain_id) {
                    ui.weak(tr(language, "This domain is disabled. Saved terms apply after it is enabled."));
                }
            }
            ui.add_space(10.0);
            ui.strong(tr(language, "Word and translations"));
            let primary = preferred_language_index(language);
            let languages = form.languages.get_or_insert([primary, if primary == 1 { 0 } else { 1 }]);
            for position in 0..2 {
                ui.push_id(("term_language", position), |ui| {
                    egui::ComboBox::from_id_salt("language").selected_text(language_name(language, languages[position]))
                        .show_ui(ui, |ui| {
                            for index in 0..LANGUAGE_CODES.len() {
                                if index != languages[1 - position] {
                                    ui.selectable_value(&mut languages[position], index, language_name(language, index));
                                }
                            }
                        });
                    let response = ui.add(egui::TextEdit::singleline(&mut node.values[languages[position]])
                        .id_salt("value").desired_width(ui.available_width()).char_limit(512)
                        .hint_text(tr(language, if position == 0 { "Enter a word or phrase" } else { "Translation (optional)" })));
                    if position == 0 && form.focus_input {
                        response.request_focus();
                        form.focus_input = false;
                    }
                    changed |= response.changed();
                });
            }
            egui::CollapsingHeader::new(tr(language, "Other languages")).id_salt(("term_languages", &node.id))
                .show(ui, |ui| {
                    for index in 0..LANGUAGE_CODES.len() {
                        if languages.contains(&index) { continue; }
                        ui.label(language_name(language, index));
                        changed |= ui.add(egui::TextEdit::singleline(&mut node.values[index])
                            .desired_width(ui.available_width()).char_limit(512)).changed();
                    }
                });
            changed |= form.aliases.render(ui, &mut node, language);
            if form.new_domain.is_none() {
                let matches = form.matching_words(&node, snapshot);
                if !matches.is_empty() {
                    ui.weak(tr(language, "This domain already contains a matching word."));
                    for &index in matches {
                        let other = &snapshot.nodes[index];
                        let text = format!("{} · {}", tr(language, "Edit existing term"), node_label(other, language));
                        if ui.add(egui::Button::new(text).wrap()).clicked() { action = Some(Action::Existing(other.id.clone())); }
                    }
                }
            }
            ui.add_space(8.0);
            changed |= ui.checkbox(&mut node.inherit_domain, tr(language, "Follow domain rules")).changed();
            if node.inherit_domain {
                ui.weak(tr(language, "Adding or moving a term into a domain is enough. No word-to-word connections are needed."));
                if form.new_domain.is_none() {
                    ui.weak(domain_rules::summary(snapshot, &node.domain_id, language));
                }
            } else {
                changed |= rules::render(ui, snapshot, language, &mut node, &mut form);
            }
            egui::CollapsingHeader::new(tr(language, "Advanced settings")).id_salt(("term_advanced", &node.id))
                .show(ui, |ui| {
                    changed |= ui.checkbox(&mut node.enabled, tr(language, "Enabled")).changed();
                    changed |= ui.checkbox(&mut node.promptable, tr(language, "Include in prompts")).changed();
                    if !node.promptable { ui.weak(tr(language, "Cue words activate other terms without being offered as translations.")); }
                    ui.label(tr(language, "Source title / note"));
                    changed |= ui.add(egui::TextEdit::singleline(&mut node.title)
                        .desired_width(ui.available_width()).char_limit(256)).changed();
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Priority"));
                        changed |= ui.add(egui::DragValue::new(&mut node.priority).range(-100..=100)).changed();
                    });
                    ui.weak(format!("{}: {}", tr(language, "Term ID"), node.id));
                });
            controller.draft_dirty |= changed;
            let valid_values = node.values.iter().all(|value|
                !value.contains([',', '\r', '\n']) && value.chars().count() <= 512);
            if !valid_values {
                ui.colored_label(graph_style::ERROR_BORDER,
                    tr(language, "Language values cannot contain commas or newlines, or exceed 512 characters."));
            }
            let valid_aliases = aliases::valid(&node);
            if !valid_aliases {
                ui.colored_label(graph_style::ERROR_BORDER, tr(language, "Each misrecognition needs a correct word in the same language; use one phrase per line, without commas, up to 512 characters."));
            }
            let domain_valid = form.new_domain.as_ref().map_or_else(
                || snapshot.domains.iter().any(|domain| domain.id == node.domain_id),
                |domain| !domain.title.trim().is_empty() && !domain.title.contains(['\r','\n'])
                    && !snapshot.domains.iter().any(|existing| existing.parent_id.is_none()
                        && existing.title.trim().to_lowercase() == domain.title.trim().to_lowercase()));
            let valid = domain_valid && valid_values && valid_aliases && node.values.iter().any(|v| !v.trim().is_empty())
                && valid_node_id(&node.id) && (!controller.new_node || !snapshot.nodes.iter().any(|n| n.id == node.id))
                && !node.title.contains(['\r','\n']) && node.title.chars().count() <= 256;
            ui.add_space(10.0);
            let can_save = valid && controller.draft_dirty && !controller.pending;
            ui.horizontal_wrapped(|ui| {
                if components::primary_button_enabled(ui, tr(language, "Save term"), can_save).clicked() {
                    action = Some(Action::Save(false));
                }
                if controller.new_node && components::secondary_button_enabled(ui,
                    tr(language, "Save and add another"), can_save).clicked() {
                    action = Some(Action::Save(true));
                }
                if controller.draft_dirty && components::secondary_button(ui,
                    tr(language, if controller.new_node { "Cancel" } else { "Discard" })).clicked() {
                    action = Some(Action::Discard);
                }
            });
            if can_save && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
                action = Some(Action::Save(false));
            }
            if !controller.new_node && !controller.draft_dirty {
                ui.add_space(8.0);
                egui::CollapsingHeader::new(tr(language, "Connections")).id_salt(("outgoing_rules", &node.id))
                    .show(ui, |ui| {
                        if components::secondary_button(ui, tr(language, "Add related term")).clicked() { action = Some(Action::Related); }
                        ui.weak(tr(language, "Creates a term in this domain, with this word as its cue."));
                        for edge in snapshot.edges.iter().filter(|edge| edge.source_id == node.id) {
                            ui.label(format!("{} → {} · {}", node_label(&node, language),
                                endpoint_label(snapshot, &edge.target_id, language),
                                tr(language, if edge.kind == GraphEdgeKind::Trigger { "Cue word" } else { "Context condition" })));
                            edge_controls(controller, edge, snapshot, ui, language);
                        }
                    });
                if components::danger_button_enabled(ui, tr(language, "Delete term"), !controller.pending).clicked() {
                    controller.confirm_delete = true;
                }
            }
        });
    });
    controller.node_draft = Some(node.clone());
    controller.term_form = form.clone();
    match action {
        Some(Action::Save(another)) => {
            node.values
                .iter_mut()
                .for_each(|value| *value = value.trim().to_owned());
            node.title = node.title.trim().to_owned();
            if node.title.is_empty() {
                node.title = node_label(&node, language).chars().take(256).collect();
            }
            if let Some(domain) = &mut form.new_domain {
                domain.title = domain.title.trim().to_owned();
                node.domain_id = domain.id.clone();
            }
            controller.selected_node = Some(node.id.clone());
            controller.node_draft = Some(node.clone());
            controller.send(Command::SaveTerm(
                GraphTermEdit {
                    node,
                    domain: form.new_domain,
                    incoming_edges: form.incoming_edges,
                },
                another,
            ));
        }
        Some(Action::Discard) => {
            controller.draft_dirty = false;
            if let Some(original) = snapshot.nodes.iter().find(|n| n.id == node.id) {
                controller.select_node(original);
            } else {
                close(controller);
            }
        }
        Some(Action::Close) => close(controller),
        Some(Action::Existing(id)) => {
            controller.draft_dirty = false;
            if let Some(existing) = snapshot.nodes.iter().find(|n| n.id == id) {
                controller.select_node(existing);
            }
        }
        Some(Action::Related) => controller.create_related(&node),
        None => {}
    }
}

fn close(controller: &mut CorpusStudioController) {
    controller.selected_node = None;
    controller.selected_edge = None;
    controller.node_draft = None;
    controller.term_form = TermForm::default();
    controller.new_node = false;
    controller.draft_dirty = false;
    controller.focus_connections = false;
    controller.editor.clear_selection();
}

pub(super) fn language_name(language: UiLanguage, index: usize) -> String {
    const NAMES: [&str; 16] = [
        "Chinese",
        "English",
        "French",
        "Portuguese",
        "Spanish",
        "Japanese",
        "Russian",
        "Korean",
        "Thai",
        "Italian",
        "German",
        "Vietnamese",
        "Indonesian",
        "Polish",
        "Czech",
        "Dutch",
    ];
    format!(
        "{} · {}",
        LANGUAGE_CODES[index].to_uppercase(),
        tr(language, NAMES[index])
    )
}
