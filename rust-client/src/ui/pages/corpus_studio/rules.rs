//! Incoming rules are edited from the term they affect; no endpoint IDs or direction selectors.
use super::*;

pub(super) fn incoming(snapshot: &GraphSnapshot, id: &str) -> Vec<GraphEdge> {
    snapshot
        .edges
        .iter()
        .filter(|edge| edge.target_id == id)
        .cloned()
        .collect()
}

pub(super) fn has_cue(snapshot: &GraphSnapshot, edges: &[GraphEdge]) -> bool {
    edges.iter().any(|edge| {
        edge.kind == GraphEdgeKind::Trigger
            && edge.enabled
            && snapshot.nodes.iter().any(|node| {
                node.id == edge.source_id
                    && node.enabled
                    && domain_path(snapshot, &node.domain_id)
                        .iter()
                        .all(|domain| domain.enabled)
            })
    })
}

pub(super) fn label(node: &GraphNode) -> &'static str {
    if !node.promptable {
        "Cue word only"
    } else if node.activation == CorpusActivation::Always {
        "Match directly"
    } else {
        "Use cue words"
    }
}

pub(super) fn render(
    ui: &mut egui::Ui,
    snapshot: &GraphSnapshot,
    language: UiLanguage,
    node: &mut GraphNode,
    form: &mut editor::TermForm,
) -> bool {
    let mut changed = false;
    ui.weak(tr(language, label(node)));
    egui::CollapsingHeader::new(tr(language, "When to use this term"))
        .id_salt(("term_rules", &node.id))
        .default_open(node.activation == CorpusActivation::OnEvidence)
        .show(ui, |ui| {
            changed |= ui.radio_value(&mut node.activation, CorpusActivation::Always,
                tr(language, "Match directly")).changed();
            changed |= ui.radio_value(&mut node.activation, CorpusActivation::OnEvidence,
                tr(language, "Use cue words")).changed();
            if node.activation == CorpusActivation::Always {
                ui.weak(tr(language, "Available without cue words. Relevant terms are selected within the model budget."));
            } else {
                ui.weak(tr(language, "Any cue word can activate this term. Recent conversation can keep it active."));
                changed |= connections(ui, snapshot, language, node, &mut form.incoming_edges,
                    &mut form.cue_search, GraphEdgeKind::Trigger);
                if !has_cue(snapshot, &form.incoming_edges) {
                    ui.colored_label(graph_style::ERROR_BORDER,
                        tr(language, "Choose an enabled cue word, or use direct matching."));
                }
                egui::CollapsingHeader::new(tr(language, "Additional context (optional)"))
                    .id_salt(("term_context", &node.id))
                    .show(ui, |ui| {
                        ui.weak(tr(language, "Activation also needs one of these words in the same input."));
                        changed |= connections(ui, snapshot, language, node, &mut form.incoming_edges,
                            &mut form.context_search, GraphEdgeKind::Context);
                    });
            }
        });
    changed
}

fn connections(
    ui: &mut egui::Ui,
    snapshot: &GraphSnapshot,
    language: UiLanguage,
    node: &GraphNode,
    edges: &mut Vec<GraphEdge>,
    search: &mut String,
    kind: GraphEdgeKind,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, edge) in edges
        .iter_mut()
        .enumerate()
        .filter(|(_, edge)| edge.kind == kind)
    {
        ui.push_id(("term_rule", index), |ui| {
            ui.horizontal_wrapped(|ui| {
                let source = snapshot
                    .nodes
                    .iter()
                    .find(|source| source.id == edge.source_id);
                let label = source.map_or(tr(language, "Missing term"), |source| {
                    node_label(source, language)
                });
                changed |= ui.checkbox(&mut edge.enabled, label).changed();
                if components::delete_button(ui, tr(language, "Remove")).clicked() {
                    remove = Some(index);
                }
            });
            if let Some(source) = snapshot
                .nodes
                .iter()
                .find(|source| source.id == edge.source_id)
            {
                ui.weak(domain_path_label(snapshot, &source.domain_id, language));
                if !source.enabled
                    || domain_path(snapshot, &source.domain_id)
                        .iter()
                        .any(|d| !d.enabled)
                {
                    ui.weak(tr(language, "This cue word or its domain is disabled."));
                }
            }
        });
    }
    if let Some(index) = remove {
        edges.remove(index);
        changed = true;
    }
    ui.add(
        egui::TextEdit::singleline(search)
            .desired_width(ui.available_width())
            .hint_text(tr(language, "Search words to add")),
    );
    if !search.trim().is_empty() {
        let mut candidates: Vec<_> = snapshot
            .nodes
            .iter()
            .filter(|candidate| {
                candidate.id != node.id
                    && matches_node(candidate, search)
                    && !edges
                        .iter()
                        .any(|edge| edge.kind == kind && edge.source_id == candidate.id)
            })
            .collect();
        candidates.sort_by_key(|candidate| {
            (
                candidate.domain_id != node.domain_id,
                !candidate.enabled,
                candidate.title.as_str(),
            )
        });
        if candidates.is_empty() {
            ui.weak(tr(language, "No matching terms"));
        }
        egui::ScrollArea::vertical()
            .id_salt(("rule_choices", kind))
            .max_height(160.0)
            .show(ui, |ui| {
                for candidate in candidates {
                    let text = format!(
                        "{} · {}",
                        node_label(candidate, language),
                        domain_path_label(snapshot, &candidate.domain_id, language)
                    );
                    if ui
                        .add(egui::Button::new(text).frame(false).wrap())
                        .clicked()
                    {
                        edges.push(GraphEdge {
                            source_id: candidate.id.clone(),
                            target_id: node.id.clone(),
                            kind,
                            enabled: true,
                        });
                        search.clear();
                        changed = true;
                    }
                }
            });
    }
    changed
}
