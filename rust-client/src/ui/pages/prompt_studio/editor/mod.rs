pub(super) mod access;
mod canvas;
mod history;
mod navigation;
mod runtime_preview;
mod style;
mod style_panel;

use super::PromptStudioAction;
pub use history::PromptStudioHistory;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, UiBuilder,
    Vec2,
};
use std::collections::{HashMap, HashSet};
use xrtranslate_prompt::{
    PromptCondition, PromptExecutionTrace, PromptGraphDomain, PromptLink, PromptMessageRole,
    PromptNode, PromptNodeGraph, PromptNodeKind, PromptNodePage, PromptProviderTarget,
    PromptSystemValue, PromptTemplateLibrary, PromptTemplateProfile, PromptTextComparison,
    PromptVariable, TranslationPromptBlock, compose_input_indexes, condition_label,
    system_value_label,
};

const NODE_WIDTH: f32 = 220.0;
const NODE_HEADER_HEIGHT: f32 = 28.0;
const SOCKET_RADIUS: f32 = 5.0;
const GRAPH_ACCENT: Color32 = style::GRAPH_ACCENT;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PromptLinkKey {
    pub from: String,
    pub to: String,
    pub input: u8,
}

type PromptGraphEditorState =
    crate::ui::graph_editor::GraphEditorState<String, PromptLinkKey, String, (String, u8)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeTitleEdit {
    pub node_id: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PromptStudioController {
    domain: PromptGraphDomain,
    selected_id: String,
    draft: Option<PromptTemplateProfile>,
    dirty: bool,
    history: PromptStudioHistory,
    drag_start_profile: Option<PromptTemplateProfile>,
    editing_title: Option<NodeTitleEdit>,
    title_edit_start_profile: Option<PromptTemplateProfile>,
    text_edit_start_profile: Option<PromptTemplateProfile>,
    editor: PromptGraphEditorState,
    active_provider: PromptProviderTarget,
    branch_filters: HashMap<PromptCondition, Option<bool>>,
    text_branch_filters: HashMap<String, Option<String>>,
    branch_hidden_nodes: HashSet<String>,
    runtime_trace: Option<PromptExecutionTrace>,
    overview_positions: HashMap<String, [f32; 2]>,
    style_panel: style_panel::StylePanel,
}

impl Default for PromptStudioController {
    fn default() -> Self {
        Self::for_provider(PromptProviderTarget::OpenAiCompatible)
    }
}

impl PromptStudioController {
    pub fn for_provider(active_provider: PromptProviderTarget) -> Self {
        Self {
            domain: domain_for_target(active_provider),
            selected_id: String::new(),
            draft: None,
            dirty: false,
            history: PromptStudioHistory::default(),
            drag_start_profile: None,
            editing_title: None,
            title_edit_start_profile: None,
            text_edit_start_profile: None,
            editor: PromptGraphEditorState::default(),
            active_provider,
            branch_filters: HashMap::new(),
            text_branch_filters: HashMap::new(),
            branch_hidden_nodes: HashSet::new(),
            runtime_trace: None,
            overview_positions: HashMap::new(),
            style_panel: style_panel::StylePanel::default(),
        }
    }

    pub fn sync_provider(&mut self, target: PromptProviderTarget) {
        let domain = domain_for_target(target);
        if self.domain != domain {
            self.switch_domain(domain);
        }
        self.select_provider(target);
    }

    pub fn open_style_panel(
        &mut self,
        target: PromptProviderTarget,
        library: &PromptTemplateLibrary,
    ) {
        self.sync_provider(target);
        self.style_panel.collapsed = false;
        self.select_profile(library.active_id.clone(), library);
    }

    pub fn snapshot(&mut self, library: &PromptTemplateLibrary) -> PromptStudioSnapshot {
        let profiles = &library.profiles;
        if self.selected_id.is_empty()
            || !profiles
                .iter()
                .any(|profile| profile.id == self.selected_id)
        {
            self.selected_id = library.active_id.clone();
        }

        let selected = profiles
            .iter()
            .find(|profile| profile.id == self.selected_id)
            .cloned()
            .or_else(|| profiles.first().cloned())
            .unwrap_or_else(default_profile);
        if self.draft.is_none()
            || self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.id != selected.id || (!self.dirty && draft != &selected))
        {
            self.draft = Some(selected.clone());
            self.dirty = false;
            self.history.clear();
            self.editor.reset_for_graph(selected.id.clone());
            self.cleanup_transient_state();
            self.sync_current_branch_filters();
        }

        PromptStudioSnapshot {
            domain: self.domain,
            profiles: profiles.to_vec(),
            active_id: library.active_id.clone(),
            selected_id: self.selected_id.clone(),
            draft: self.draft.clone().unwrap_or(selected),
        }
    }

    pub fn select_profile(&mut self, id: String, library: &PromptTemplateLibrary) {
        self.select_profile_from_snapshot(id, &library.profiles);
    }

    /// An explicit card choice has already been committed and supersedes the editor snapshot.
    pub fn accept_style_selection(&mut self, id: String, library: &PromptTemplateLibrary) {
        self.dirty = false;
        self.select_profile(id, library);
    }

    pub fn select_profile_from_snapshot(&mut self, id: String, profiles: &[PromptTemplateProfile]) {
        if self.dirty {
            return;
        }
        self.selected_id = id;
        self.draft = profiles
            .iter()
            .find(|profile| profile.id == self.selected_id)
            .cloned();
        self.history.clear();
        self.editor.reset_for_graph(self.selected_id.clone());
        self.cleanup_transient_state();
        self.sync_current_branch_filters();
    }

    pub fn domain(&self) -> PromptGraphDomain {
        self.domain
    }

    pub fn switch_domain(&mut self, domain: PromptGraphDomain) {
        if self.domain == domain {
            return;
        }
        self.domain = domain;
        self.active_provider = default_target_for_domain(domain);
        self.cleanup_transient_state();
        self.sync_current_branch_filters();
        self.canvas.fit_pending = true;
        self.runtime_trace = None;
    }

    pub fn set_draft(&mut self, profile: PromptTemplateProfile) {
        self.selected_id = profile.id.clone();
        self.draft = Some(profile);
        self.dirty = true;
        self.history.clear();
        self.editor.replace_graph(self.selected_id.clone());
        self.cleanup_transient_state();
        self.sync_current_branch_filters();
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_runtime_trace(&mut self, trace: Option<PromptExecutionTrace>) {
        self.runtime_trace = trace;
    }

    pub fn active_provider(&self) -> PromptProviderTarget {
        self.active_provider
    }

    pub fn push_history(&mut self, before: PromptTemplateProfile) {
        self.history.push(before);
        self.dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        let read_only = self.draft.as_ref().map_or(true, |d| d.read_only);
        self.history.can_undo(read_only)
    }

    pub fn can_redo(&self) -> bool {
        let read_only = self.draft.as_ref().map_or(true, |d| d.read_only);
        self.history.can_redo(read_only)
    }

    pub fn undo(&mut self) {
        let Some(current) = self.draft.clone() else {
            return;
        };
        if let Some(previous) = self.history.undo(current) {
            self.draft = Some(previous);
            self.dirty = true;
            self.cleanup_transient_state();
            self.sync_current_branch_filters();
        }
    }

    pub fn redo(&mut self) {
        let Some(current) = self.draft.clone() else {
            return;
        };
        if let Some(next) = self.history.redo(current) {
            self.draft = Some(next);
            self.dirty = true;
            self.cleanup_transient_state();
            self.sync_current_branch_filters();
        }
    }

    pub fn start_editing_title(
        &mut self,
        node_id: String,
        initial_text: String,
        before: PromptTemplateProfile,
    ) {
        self.editing_title = Some(NodeTitleEdit {
            node_id,
            text: initial_text,
        });
        self.title_edit_start_profile = Some(before);
    }

    pub fn cancel_editing_title(&mut self) {
        self.editing_title = None;
        self.title_edit_start_profile = None;
    }

    pub fn cleanup_transient_state(&mut self) {
        self.editor.clear_interaction();
        self.drag_start_profile = None;
        self.editing_title = None;
        self.title_edit_start_profile = None;
        self.text_edit_start_profile = None;
        if let Some(draft) = &self.draft {
            let valid_node_ids = draft
                .graph
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<HashSet<_>>();
            let valid_links = draft
                .graph
                .links
                .iter()
                .map(|link| PromptLinkKey {
                    from: link.from.clone(),
                    to: link.to.clone(),
                    input: link.input,
                })
                .collect::<HashSet<_>>();
            self.editor
                .selected_nodes
                .retain(|id| valid_node_ids.contains(id));
            self.editor
                .selected_links
                .retain(|link| valid_links.contains(link));
        } else {
            self.editor.clear_selection();
        }
    }

    fn node_position(&self, node: &PromptNode) -> [f32; 2] {
        self.display_position(
            &node.id,
            self.overview_positions
                .get(&node.id)
                .copied()
                .unwrap_or(node.position),
        )
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn new_node_position(&self, graph: &PromptNodeGraph) -> [f32; 2] {
        self.new_node_position_near(graph, self.add_node_center)
    }

    fn new_node_position_near(
        &self,
        graph: &PromptNodeGraph,
        center: impl Into<Option<[f32; 2]>>,
    ) -> [f32; 2] {
        let candidate_size = Vec2::new(540.0, 220.0);
        self.editor.new_node_position(
            graph
                .nodes
                .iter()
                .filter(|node| self.node_is_visible(node))
                .map(|node| graph_space_rect(node.position, node_size(graph, node))),
            candidate_size,
            center.into(),
            16.0,
        )
    }

    fn select_provider(&mut self, target: PromptProviderTarget) {
        if self.active_provider == target {
            return;
        }
        self.active_provider = target;
        self.branch_filters.clear();
        self.text_branch_filters.clear();
        self.branch_hidden_nodes.clear();
        self.editor.clear_interaction();
        self.editor.clear_selection();
        self.editor.canvas.fit_pending = true;
        self.sync_current_branch_filters();
    }

    fn node_is_visible(&self, node: &PromptNode) -> bool {
        node.page.is_visible_on(self.active_provider)
            && !self.branch_hidden_nodes.contains(&node.id)
    }

    fn sync_branch_filters(&mut self, graph: &PromptNodeGraph) {
        let conditions = branch_conditions_on_page(graph, self.active_provider);
        let available = conditions.iter().copied().collect::<HashSet<_>>();
        self.branch_filters
            .retain(|condition, _| available.contains(condition));
        for condition in conditions {
            self.branch_filters.entry(condition).or_insert(None);
        }
        let text_sources = text_branch_sources_on_page(graph, self.active_provider);
        self.text_branch_filters
            .retain(|source, _| text_sources.iter().any(|(id, _)| id == source));
        for (source, _) in &text_sources {
            self.text_branch_filters
                .entry(source.clone())
                .or_insert(None);
        }
        self.branch_hidden_nodes =
            branch_hidden_nodes(graph, self.active_provider, &self.branch_filters);
        self.branch_hidden_nodes.extend(text_branch_hidden_nodes(
            graph,
            self.active_provider,
            &self.text_branch_filters,
        ));
        let hidden = self.branch_hidden_nodes.clone();
        self.editor.selected_nodes.retain(|id| !hidden.contains(id));
        self.editor
            .selected_links
            .retain(|link| !hidden.contains(&link.from) && !hidden.contains(&link.to));
    }

    fn sync_current_branch_filters(&mut self) {
        if let Some(graph) = self.draft.as_ref().map(|draft| draft.graph.clone()) {
            self.sync_branch_filters(&graph);
        } else {
            self.branch_filters.clear();
            self.text_branch_filters.clear();
            self.branch_hidden_nodes.clear();
        }
    }

    fn branch_conditions(&self) -> Vec<PromptCondition> {
        const ORDER: [PromptCondition; 4] = [
            PromptCondition::IsPseudoStreaming,
            PromptCondition::SourceIsAuto,
            PromptCondition::HasReferenceContext,
            PromptCondition::HasRecognitionContext,
        ];
        ORDER
            .into_iter()
            .filter(|condition| self.branch_filters.contains_key(condition))
            .collect()
    }

    fn branch_filter(&self, condition: PromptCondition) -> Option<bool> {
        self.branch_filters.get(&condition).copied().flatten()
    }

    fn set_branch_filter(
        &mut self,
        graph: &PromptNodeGraph,
        condition: PromptCondition,
        branch: Option<bool>,
    ) {
        let Some(filter) = self.branch_filters.get_mut(&condition) else {
            return;
        };
        if *filter == branch {
            return;
        }
        *filter = branch;
        self.sync_branch_filters(graph);
        self.canvas.fit_pending = true;
    }

    fn text_branch_filters(&self, graph: &PromptNodeGraph) -> Vec<(String, String, Vec<String>)> {
        text_branch_sources_on_page(graph, self.active_provider)
            .into_iter()
            .filter_map(|(source_id, cases)| {
                let label = graph
                    .nodes
                    .iter()
                    .find(|node| node.id == source_id)
                    .map(node_display_label)?;
                Some((source_id, label, cases))
            })
            .collect()
    }

    fn text_branch_filter(&self, source_id: &str) -> Option<&str> {
        self.text_branch_filters
            .get(source_id)
            .and_then(Option::as_deref)
    }

    fn set_text_branch_filter(
        &mut self,
        graph: &PromptNodeGraph,
        source_id: &str,
        value: Option<String>,
    ) {
        let Some(filter) = self.text_branch_filters.get_mut(source_id) else {
            return;
        };
        if *filter == value {
            return;
        }
        *filter = value;
        self.sync_branch_filters(graph);
        self.canvas.fit_pending = true;
    }
}

impl std::ops::Deref for PromptStudioController {
    type Target = PromptGraphEditorState;

    fn deref(&self) -> &Self::Target {
        &self.editor
    }
}

impl std::ops::DerefMut for PromptStudioController {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor
    }
}

fn branch_conditions_on_page(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
) -> Vec<PromptCondition> {
    let mut seen = HashSet::new();
    graph
        .nodes
        .iter()
        .filter(|node| node.page.is_visible_on(target))
        .filter_map(|node| match node.kind {
            PromptNodeKind::ConditionValue { condition } if seen.insert(condition) => {
                Some(condition)
            }
            _ => None,
        })
        .collect()
}

fn text_branch_sources_on_page(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
) -> Vec<(String, Vec<String>)> {
    let mut sources = Vec::<(String, Vec<String>)>::new();
    for node in graph.nodes.iter().filter(|node| {
        node.page.is_visible_on(target) && matches!(node.kind, PromptNodeKind::TextSwitch)
    }) {
        let Some(source) = graph
            .links
            .iter()
            .find(|link| link.to == node.id && link.input == 0)
            .map(|link| link.from.clone())
        else {
            continue;
        };
        let Some(cases) = graph.text_switch_cases(&node.id) else {
            continue;
        };
        if let Some((_, existing)) = sources.iter_mut().find(|(id, _)| id == &source) {
            for case in &cases {
                if !existing.contains(case) {
                    existing.push(case.clone());
                }
            }
        } else {
            sources.push((source, cases));
        }
    }
    sources
}

fn text_branch_hidden_nodes(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
    filters: &HashMap<String, Option<String>>,
) -> HashSet<String> {
    let mut hidden = HashSet::new();
    for (source_id, selected) in filters {
        let Some(selected) = selected else {
            continue;
        };
        for node in graph.nodes.iter().filter(|node| {
            node.page.is_visible_on(target)
                && matches!(node.kind, PromptNodeKind::TextSwitch)
                && graph
                    .links
                    .iter()
                    .any(|link| link.to == node.id && link.input == 0 && link.from == *source_id)
        }) {
            let Some(cases) = graph.text_switch_cases(&node.id) else {
                continue;
            };
            let Some(selected_input) = cases
                .iter()
                .position(|case| case == selected)
                .map(|index| index as u8 + 1)
            else {
                continue;
            };
            let mut selected_ancestors = HashSet::new();
            let mut opposite_ancestors = HashSet::new();
            for link in graph
                .links
                .iter()
                .filter(|link| link.to == node.id && link.input > 0)
            {
                if link.input == selected_input {
                    collect_upstream_ancestors(graph, target, &link.from, &mut selected_ancestors);
                } else {
                    collect_upstream_ancestors(graph, target, &link.from, &mut opposite_ancestors);
                }
            }
            hidden.extend(
                opposite_ancestors
                    .difference(&selected_ancestors)
                    .filter(|id| *id != &node.id)
                    .cloned(),
            );
        }
    }
    hidden
}

fn branch_hidden_nodes(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
    filters: &HashMap<PromptCondition, Option<bool>>,
) -> HashSet<String> {
    let mut hidden = HashSet::new();
    for (&condition, &selected_branch) in filters {
        let Some(selected_branch) = selected_branch else {
            continue;
        };
        let mut false_ancestors = HashSet::new();
        let mut true_ancestors = HashSet::new();
        let mut condition_switches = HashSet::new();
        for node in graph.nodes.iter().filter(|node| {
            node.page.is_visible_on(target)
                && matches!(node.kind, PromptNodeKind::Switch { .. })
                && graph.links.iter().any(|link| {
                    link.to == node.id
                        && link.input == 0
                        && graph.nodes.iter().any(|source| {
                            source.id == link.from
                                && matches!(source.kind, PromptNodeKind::ConditionValue { condition: value } if value == condition)
                        })
                })
        }) {
            condition_switches.insert(node.id.clone());
            for link in graph.links.iter().filter(|link| link.to == node.id) {
                let ancestors = if link.input == 1 {
                    &mut false_ancestors
                } else if link.input == 2 {
                    &mut true_ancestors
                } else {
                    continue;
                };
                collect_upstream_ancestors(graph, target, &link.from, ancestors);
            }
        }

        let (selected, opposite) = if selected_branch {
            (&true_ancestors, &false_ancestors)
        } else {
            (&false_ancestors, &true_ancestors)
        };
        hidden.extend(
            opposite
                .difference(selected)
                .filter(|id| !condition_switches.contains(*id))
                .cloned(),
        );
    }
    hidden
}

fn collect_upstream_ancestors(
    graph: &PromptNodeGraph,
    target: PromptProviderTarget,
    start: &str,
    ancestors: &mut HashSet<String>,
) {
    let mut pending = vec![start.to_owned()];
    while let Some(id) = pending.pop() {
        let Some(node) = graph.nodes.iter().find(|node| node.id == id) else {
            continue;
        };
        if !node.page.is_visible_on(target) || !ancestors.insert(id.clone()) {
            continue;
        }
        pending.extend(
            graph
                .links
                .iter()
                .filter(|link| link.to == id)
                .map(|link| link.from.clone()),
        );
    }
}

#[derive(Clone, Debug)]
pub struct PromptStudioSnapshot {
    pub domain: PromptGraphDomain,
    pub profiles: Vec<PromptTemplateProfile>,
    pub active_id: String,
    pub selected_id: String,
    pub draft: PromptTemplateProfile,
}

pub fn render(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
) -> Vec<PromptStudioAction> {
    ui.scope(|ui| {
        style::apply(ui);
        let mut actions = Vec::new();
        render_heading(snapshot, controller, ui, language, &mut actions);
        ui.add_space(6.0);
        canvas::render_graph_editor(snapshot, controller, ui, language, &mut actions);
        actions
    })
    .inner
}

fn render_heading(
    snapshot: &PromptStudioSnapshot,
    controller: &PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) {
    use crate::i18n::tr;
    ui.horizontal_wrapped(|ui| {
        ui.heading(tr(language, "Prompt Studio"));
        ui.separator();
        for (domain, label) in [
            (PromptGraphDomain::Translation, "Translation"),
            (PromptGraphDomain::Asr, "ASR prompts"),
        ] {
            if crate::ui::graph_style::tab(ui, tr(language, label), snapshot.domain == domain)
                .clicked()
            {
                actions.push(PromptStudioAction::SwitchDomain(domain));
            }
        }
        if !controller.is_dirty() {
            ui.weak(tr(language, "Saved"));
        }
    });
}

fn render_profile_picker(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) {
    let name = controller
        .draft
        .as_ref()
        .map_or(&snapshot.draft.name, |draft| &draft.name)
        .clone();
    crate::ui::components::combobox_ui_with_width(
        ui,
        "prompt_design_select",
        crate::i18n::tr_dynamic(language, &name),
        Some(180.0),
        |ui| {
            for profile in &snapshot.profiles {
                let name = crate::i18n::tr_dynamic(language, &profile.name);
                let label = if profile.id == snapshot.active_id {
                    format!("{name}  ✓")
                } else {
                    name.into_owned()
                };
                if ui
                    .selectable_label(profile.id == snapshot.selected_id, label)
                    .clicked()
                {
                    save_before_switch(snapshot, controller, actions);
                    controller.select_profile_from_snapshot(profile.id.clone(), &snapshot.profiles);
                    actions.push(PromptStudioAction::SelectProfile(profile.id.clone()));
                }
            }
        },
    );
}

pub(super) fn save_before_switch(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    actions: &mut Vec<PromptStudioAction>,
) {
    if !controller.is_dirty() {
        return;
    }
    if let Some(profile) = controller.draft.clone() {
        actions.push(if profile.id == snapshot.active_id {
            PromptStudioAction::ActivateProfile(profile)
        } else {
            PromptStudioAction::SaveProfile(profile)
        });
        controller.dirty = false;
    }
}

fn available_blocks() -> Vec<(&'static str, TranslationPromptBlock)> {
    vec![
        ("Language order", TranslationPromptBlock::LanguageOrder),
        ("Terminology", TranslationPromptBlock::Terminology),
        (
            "Recent turns",
            TranslationPromptBlock::RecentTurns { limit: Some(3) },
        ),
        (
            "Previous revision",
            TranslationPromptBlock::PreviousRevision,
        ),
        (
            "Surrounding source",
            TranslationPromptBlock::SurroundingSource,
        ),
        (
            "Custom instruction",
            TranslationPromptBlock::CustomText {
                text: "Keep the translation natural and preserve the speaker's tone.".into(),
            },
        ),
    ]
}

fn node_size(graph: &PromptNodeGraph, node: &PromptNode) -> Vec2 {
    let height = match &node.kind {
        PromptNodeKind::Input {
            block: TranslationPromptBlock::CustomText { .. },
        }
        | PromptNodeKind::Compose { .. } => {
            // Text scrolls inside its card; socket rows determine the required height.
            let inputs = graph.compose_input_socket_indexes(&node.id).len();
            128.0_f32.max(72.0 + inputs as f32 * 25.0)
        }
        PromptNodeKind::Switch { .. }
        | PromptNodeKind::TextSwitch
        | PromptNodeKind::Request { .. }
        | PromptNodeKind::TextComparison { .. } => graph
            .node_layout_height(&node.id)
            .unwrap_or_else(|| node.layout_height()),
        PromptNodeKind::SystemValue { .. } | PromptNodeKind::ConditionValue { .. } => 112.0,
        _ => 84.0,
    };
    Vec2::new(runtime_preview::base_width(&node.kind), height)
}

/// Compact the read-only overview without changing the saved graph or execution fingerprint.
fn compact_positions(
    graph: &PromptNodeGraph,
    visible: impl Fn(&PromptNode) -> bool,
) -> HashMap<String, [f32; 2]> {
    let mut columns = std::collections::BTreeMap::<i32, Vec<&PromptNode>>::new();
    for node in graph.nodes.iter().filter(|node| visible(node)) {
        columns
            .entry(node.position[0].round() as i32)
            .or_default()
            .push(node);
    }
    for nodes in columns.values_mut() {
        nodes.sort_by(|a, b| a.position[1].total_cmp(&b.position[1]));
    }
    let column_height = |nodes: &Vec<&PromptNode>| {
        nodes
            .iter()
            .map(|node| node_size(graph, node).y + 24.0)
            .sum::<f32>()
            - 24.0
    };
    let height = columns.values().map(column_height).fold(0.0_f32, f32::max);
    let mut positions = HashMap::new();
    let mut x = 0.0;
    for nodes in columns.values() {
        let mut y = (height - column_height(nodes)) * 0.5;
        let mut width = 0.0_f32;
        for node in nodes {
            positions.insert(node.id.clone(), [x, y]);
            let size = node_size(graph, node);
            width = width.max(size.x);
            y += size.y + 24.0;
        }
        x += width + 96.0;
    }
    positions
}

fn compact_graph(graph: &mut PromptNodeGraph) {
    let mut positions = HashMap::new();
    for target in [
        PromptProviderTarget::OpenAiCompatible,
        PromptProviderTarget::Hunyuan,
        PromptProviderTarget::AsrInstruction,
        PromptProviderTarget::AsrContextBias,
    ] {
        for (id, position) in compact_positions(graph, |node| node.page.is_visible_on(target)) {
            positions.entry(id).or_insert(position);
        }
    }
    for node in &mut graph.nodes {
        if let Some(position) = positions.get(&node.id) {
            node.position = *position;
        }
    }
}

fn presentation_label(label: &str) -> String {
    if label.chars().any(char::is_lowercase) {
        return label.to_owned();
    }
    let mut text = label.to_lowercase();
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    text.replace("openai", "OpenAI")
        .replace("asr", "ASR")
        .replace("tts", "TTS")
}

fn graph_space_rect(position: [f32; 2], size: Vec2) -> Rect {
    Rect::from_min_size(Pos2::new(position[0], position[1]), size)
}

fn node_display_label(node: &PromptNode) -> String {
    if !node.label.trim().is_empty() && node.label != "COMPOSE TEXT" {
        return node.label.trim().to_owned();
    }
    if let PromptNodeKind::Compose { text } = &node.kind {
        let mut summary = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("COMPOSE TEXT")
            .to_owned();
        for input in 0..=PromptNodeGraph::MAX_COMPOSE_INPUT_INDEX {
            summary = summary.replace(&format!("{{{input}}}"), "");
        }
        let summary = summary.trim_matches(|character: char| {
            character.is_ascii_whitespace() || character.is_ascii_punctuation()
        });
        let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
        if !summary.is_empty() {
            return truncate_preview(&summary, 32);
        }
    }
    if node.label.trim().is_empty() {
        block_or_kind_label(&node.kind).into()
    } else {
        node.label.trim().to_owned()
    }
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    let mut preview = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        preview.push_str(" …");
    }
    preview
}

fn block_or_kind_label(kind: &PromptNodeKind) -> &'static str {
    match kind {
        PromptNodeKind::Input { block } => block.preview_name(),
        PromptNodeKind::Variable { variable } => variable_name(*variable),
        PromptNodeKind::SystemValue { value } => system_value_label(*value),
        PromptNodeKind::ConditionValue { condition } => condition_label(*condition),
        PromptNodeKind::BoolValue { .. } => "BOOLEAN VALUE",
        PromptNodeKind::TextComparison { .. } => "TEXT COMPARISON",
        PromptNodeKind::Compose { .. } => "COMPOSE TEXT",
        PromptNodeKind::Switch { .. } => "CONDITIONAL SWITCH",
        PromptNodeKind::TextSwitch => "TEXT BRANCH SELECTOR",
        PromptNodeKind::Request { .. } => "PROVIDER REQUEST",
    }
}

fn default_profile() -> PromptTemplateProfile {
    PromptTemplateLibrary::default()
        .profiles
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(new_profile_fallback)
}

fn new_profile_fallback() -> PromptTemplateProfile {
    PromptTemplateLibrary::default()
        .profiles
        .into_iter()
        .next()
        .unwrap_or_else(|| PromptTemplateProfile {
            id: format!("custom-{}", uuid::Uuid::new_v4()),
            name: "Untitled design".into(),
            description: String::new(),
            graph: PromptNodeGraph::builtin_default(),
            read_only: false,
        })
}

fn new_profile() -> PromptTemplateProfile {
    let builtin = default_profile();
    let mut profile = PromptTemplateLibrary::editable_copy_of(
        &builtin,
        format!("custom-{}", uuid::Uuid::new_v4()),
    );
    profile.name = "Untitled design".into();
    profile.description = String::new();
    compact_graph(&mut profile.graph);
    profile
}

fn domain_for_target(target: PromptProviderTarget) -> PromptGraphDomain {
    match target {
        PromptProviderTarget::OpenAiCompatible | PromptProviderTarget::Hunyuan => {
            PromptGraphDomain::Translation
        }
        PromptProviderTarget::AsrInstruction | PromptProviderTarget::AsrContextBias => {
            PromptGraphDomain::Asr
        }
    }
}

fn default_target_for_domain(domain: PromptGraphDomain) -> PromptProviderTarget {
    match domain {
        PromptGraphDomain::Translation => PromptProviderTarget::OpenAiCompatible,
        PromptGraphDomain::Asr => PromptProviderTarget::AsrInstruction,
    }
}

fn variable_name(variable: PromptVariable) -> &'static str {
    match variable {
        PromptVariable::SourceLanguage => "SOURCE LANGUAGE",
        PromptVariable::TargetLanguage => "TARGET LANGUAGE",
        PromptVariable::CurrentInput => "CURRENT INPUT",
        PromptVariable::RecognitionContext => "RECOGNITION CONTEXT",
        PromptVariable::RecognitionMode => "RECOGNITION MODE",
    }
}

fn input_socket_label(graph: &PromptNodeGraph, node: &PromptNode, input: u8) -> String {
    match &node.kind {
        PromptNodeKind::Switch { .. } if input == 0 => "CONDITION · BOOL".into(),
        PromptNodeKind::Switch { .. } if input == 1 => "FALSE · TEXT".into(),
        PromptNodeKind::Switch { .. } => "TRUE · TEXT".into(),
        PromptNodeKind::TextComparison { .. } => "TEXT".into(),
        PromptNodeKind::TextSwitch if input == 0 => "VALUE · TEXT".into(),
        PromptNodeKind::TextSwitch => graph
            .text_switch_cases(&node.id)
            .unwrap_or_default()
            .get(usize::from(input.saturating_sub(1)))
            .map_or_else(|| "CASE · TEXT".into(), |case| format!("{case} · TEXT")),
        PromptNodeKind::Compose { .. } => format!("{{{input}}}"),
        PromptNodeKind::Request { roles, .. } => roles
            .get(usize::from(input))
            .map(|role| match role {
                PromptMessageRole::System => "SYSTEM",
                PromptMessageRole::User => "USER",
            })
            .map_or_else(|| "MESSAGE".into(), |role| format!("{} {role}", input + 1)),
        _ => String::new(),
    }
}
