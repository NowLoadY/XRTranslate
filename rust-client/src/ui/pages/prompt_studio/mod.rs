mod canvas;
mod history;
mod navigation;
mod runtime_preview;
mod style;

pub use history::PromptStudioHistory;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke,
    UiBuilder, Vec2,
};
use std::collections::{HashMap, HashSet};
use xrtranslate_prompt::{
    PromptCondition, PromptExecutionTrace, PromptGraphDomain, PromptLink, PromptMessageRole,
    PromptNode, PromptNodeGraph, PromptNodeKind, PromptNodePage, PromptProviderTarget,
    PromptTemplateLibrary, PromptTemplateProfile, PromptVariable, TranslationPromptBlock,
    compose_input_indexes,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeTitleEdit {
    pub node_id: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct PromptStudioController {
    domain: PromptGraphDomain,
    selected_id: String,
    draft: Option<PromptTemplateProfile>,
    dirty: bool,
    history: PromptStudioHistory,
    drag_start_profile: Option<PromptTemplateProfile>,
    editing_title: Option<NodeTitleEdit>,
    title_edit_start_profile: Option<PromptTemplateProfile>,
    text_edit_start_profile: Option<PromptTemplateProfile>,
    wire_from: Option<String>,
    wire_from_input: Option<(String, u8)>,
    pan: Vec2,
    zoom: f32,
    fit_pending: bool,
    selected_nodes: HashSet<String>,
    selected_links: HashSet<PromptLinkKey>,
    drag_node: Option<String>,
    drag_origins: HashMap<String, [f32; 2]>,
    box_select_start: Option<Pos2>,
    box_select_current: Option<Pos2>,
    canvas_size: Vec2,
    add_node_center: Option<[f32; 2]>,
    active_provider: PromptProviderTarget,
    branch_filters: HashMap<PromptCondition, Option<bool>>,
    branch_hidden_nodes: HashSet<String>,
    runtime_trace: Option<PromptExecutionTrace>,
    wire_base_zoom: Option<f32>,
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
            wire_from: None,
            wire_from_input: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            fit_pending: true,
            selected_nodes: HashSet::new(),
            selected_links: HashSet::new(),
            drag_node: None,
            drag_origins: HashMap::new(),
            box_select_start: None,
            box_select_current: None,
            canvas_size: Vec2::new(960.0, 540.0),
            add_node_center: None,
            active_provider,
            branch_filters: HashMap::new(),
            branch_hidden_nodes: HashSet::new(),
            runtime_trace: None,
            wire_base_zoom: None,
        }
    }

    pub fn sync_provider(&mut self, target: PromptProviderTarget) {
        let domain = domain_for_target(target);
        if self.domain != domain {
            self.switch_domain(domain);
        }
        self.select_provider(target);
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
                .is_some_and(|draft| draft.id != selected.id)
        {
            self.draft = Some(selected.clone());
            self.dirty = false;
            self.fit_pending = true;
            self.history.clear();
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
        if self.dirty {
            return;
        }
        self.selected_id = id;
        self.draft = library
            .profiles
            .iter()
            .find(|profile| profile.id == self.selected_id)
            .cloned();
        self.history.clear();
        self.cleanup_transient_state();
        self.sync_current_branch_filters();
        self.fit_pending = true;
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
        self.cleanup_transient_state();
        self.sync_current_branch_filters();
        self.fit_pending = true;
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
        self.fit_pending = true;
        self.runtime_trace = None;
    }

    pub fn set_draft(&mut self, profile: PromptTemplateProfile) {
        self.selected_id = profile.id.clone();
        self.draft = Some(profile);
        self.dirty = true;
        self.fit_pending = true;
        self.history.clear();
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
        self.wire_from = None;
        self.wire_from_input = None;
        self.drag_node = None;
        self.drag_origins.clear();
        self.drag_start_profile = None;
        self.editing_title = None;
        self.title_edit_start_profile = None;
        self.text_edit_start_profile = None;
        self.box_select_start = None;
        self.box_select_current = None;
        self.wire_base_zoom = None;
        if let Some(draft) = &self.draft {
            let valid_node_ids: HashSet<&str> =
                draft.graph.nodes.iter().map(|n| n.id.as_str()).collect();
            self.selected_nodes
                .retain(|id| valid_node_ids.contains(id.as_str()));
            self.selected_links.retain(|key| {
                draft
                    .graph
                    .links
                    .iter()
                    .any(|l| l.from == key.from && l.to == key.to && l.input == key.input)
            });
        } else {
            self.selected_nodes.clear();
            self.selected_links.clear();
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn finish_wire(&mut self) -> Option<String> {
        self.wire_from_input = None;
        self.wire_from.take()
    }

    fn new_node_position(&self, graph: &PromptNodeGraph) -> [f32; 2] {
        let center = (self.canvas_size * 0.5 - self.pan) / self.zoom;
        self.new_node_position_near(graph, [center.x, center.y])
    }

    fn new_node_position_near(&self, graph: &PromptNodeGraph, center: [f32; 2]) -> [f32; 2] {
        let candidate_size = Vec2::new(540.0, 220.0);
        let mut candidate = [
            ((center[0] - candidate_size.x * 0.5) / 16.0).round() * 16.0,
            ((center[1] - candidate_size.y * 0.5) / 16.0).round() * 16.0,
        ];

        while graph
            .nodes
            .iter()
            .filter(|node| self.node_is_visible(node))
            .any(|node| {
                graph_space_rect(candidate, candidate_size)
                    .expand(8.0)
                    .intersects(graph_space_rect(node.position, node_size(node)).expand(8.0))
            })
        {
            candidate[0] += 32.0;
            candidate[1] += 32.0;
        }
        candidate
    }

    fn select_provider(&mut self, target: PromptProviderTarget) {
        if self.active_provider == target {
            return;
        }
        self.active_provider = target;
        self.branch_filters.clear();
        self.branch_hidden_nodes.clear();
        self.selected_nodes.clear();
        self.selected_links.clear();
        self.wire_from = None;
        self.wire_from_input = None;
        self.box_select_start = None;
        self.box_select_current = None;
        self.fit_pending = true;
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
        self.branch_hidden_nodes =
            branch_hidden_nodes(graph, self.active_provider, &self.branch_filters);
        self.selected_nodes
            .retain(|id| !self.branch_hidden_nodes.contains(id));
        self.selected_links.retain(|link| {
            !self.branch_hidden_nodes.contains(&link.from)
                && !self.branch_hidden_nodes.contains(&link.to)
        });
    }

    fn sync_current_branch_filters(&mut self) {
        if let Some(graph) = self.draft.as_ref().map(|draft| draft.graph.clone()) {
            self.sync_branch_filters(&graph);
        } else {
            self.branch_filters.clear();
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
        self.fit_pending = true;
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
            PromptNodeKind::Switch { condition } if seen.insert(condition) => Some(condition),
            _ => None,
        })
        .collect()
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
                && matches!(node.kind, PromptNodeKind::Switch { condition: value } if value == condition)
        }) {
            condition_switches.insert(node.id.clone());
            for link in graph.links.iter().filter(|link| link.to == node.id) {
                let ancestors = if link.input == 0 {
                    &mut false_ancestors
                } else if link.input == 1 {
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

#[derive(Clone, Debug)]
pub enum PromptStudioAction {
    SwitchDomain(PromptGraphDomain),
    SelectProfile(String),
    CreateProfile(PromptTemplateProfile),
    DeleteProfile(String),
    SaveProfile(PromptTemplateProfile),
    ActivateProfile(PromptTemplateProfile),
    CloneProfile(PromptTemplateProfile),
    ExportProfile(PromptTemplateProfile),
    ImportProfile,
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
        render_domain_tabs(snapshot, ui, language, &mut actions);
        ui.add_space(5.0);
        render_header(snapshot, controller, ui, language, &mut actions);
        ui.add_space(6.0);
        canvas::render_graph_editor(snapshot, controller, ui, language, &mut actions);
        actions
    })
    .inner
}

fn render_domain_tabs(
    snapshot: &PromptStudioSnapshot,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) {
    Frame::new()
        .fill(style::BAR_FILL)
        .stroke(Stroke::new(1.0, style::BAR_BORDER))
        .corner_radius(CornerRadius::same(1))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::tr(language, "PROMPT STUDIO"))
                        .font(egui::FontId::monospace(10.0))
                        .color(style::INK)
                        .strong(),
                );
                ui.separator();
                for (domain, label, description) in [
                    (
                        PromptGraphDomain::Translation,
                        "TRANSLATION PROMPTS",
                        "Translation provider pages for every recognition mode",
                    ),
                    (
                        PromptGraphDomain::Asr,
                        "ASR PROMPTS",
                        "Recognition instruction and context-bias pages",
                    ),
                ] {
                    if ui
                        .selectable_label(
                            snapshot.domain == domain,
                            crate::i18n::tr(language, label),
                        )
                        .on_hover_text(description)
                        .clicked()
                    {
                        actions.push(PromptStudioAction::SwitchDomain(domain));
                    }
                }
            });
        });
}

fn render_header(
    snapshot: &PromptStudioSnapshot,
    controller: &mut PromptStudioController,
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    actions: &mut Vec<PromptStudioAction>,
) {
    Frame::new()
        .fill(style::BAR_FILL)
        .stroke(Stroke::new(1.0, style::BAR_BORDER))
        .corner_radius(CornerRadius::same(1))
        .inner_margin(Margin::symmetric(9, 5))
        .show(ui, |ui| {
            crate::ui::layout::flow_row(ui, |ui| {
                ui.label(
                    RichText::new(crate::i18n::tr(
                        language,
                        match snapshot.domain {
                            PromptGraphDomain::Translation => "TRANSLATION PAGES",
                            PromptGraphDomain::Asr => "ASR PAGES",
                        },
                    ))
                    .font(egui::FontId::monospace(10.0))
                    .color(style::MUTED),
                );
                ui.add_space(5.0);
                egui::ComboBox::from_id_salt("prompt_design_select")
                    .width(180.0)
                    .selected_text(crate::i18n::tr_dynamic(language, &snapshot.draft.name))
                    .show_ui(ui, |ui| {
                        for profile in &snapshot.profiles {
                            let name = crate::i18n::tr_dynamic(language, &profile.name);
                            let label = if profile.id == snapshot.active_id {
                                format!("{}  {}", name, crate::i18n::tr(language, "ACTIVE"))
                            } else {
                                name.into_owned()
                            };
                            if ui
                                .selectable_label(profile.id == snapshot.selected_id, label)
                                .clicked()
                            {
                                save_before_switch(snapshot, controller, actions);
                                controller.select_profile_from_snapshot(
                                    profile.id.clone(),
                                    &snapshot.profiles,
                                );
                                actions.push(PromptStudioAction::SelectProfile(profile.id.clone()));
                            }
                        }
                    });
                if small_outline_button(
                    ui,
                    crate::i18n::tr(language, "NEW GRAPH"),
                    crate::i18n::tr(language, "Create complete provider graph"),
                )
                .clicked()
                {
                    let profile = new_profile();
                    actions.push(PromptStudioAction::CreateProfile(profile.clone()));
                    controller.set_draft(profile);
                }
                if small_outline_button(
                    ui,
                    crate::i18n::tr(language, "IMPORT"),
                    crate::i18n::tr(language, "Import graph project file"),
                )
                .clicked()
                {
                    actions.push(PromptStudioAction::ImportProfile);
                }
                if small_outline_button(
                    ui,
                    crate::i18n::tr(language, "EXPORT"),
                    crate::i18n::tr(language, "Export graph project file"),
                )
                .clicked()
                {
                    if let Some(profile) = controller.draft.clone() {
                        actions.push(PromptStudioAction::ExportProfile(profile));
                    }
                }
                crate::ui::layout::flow_group(ui, 72.0, |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if controller.is_dirty() {
                            if style::command_button(ui, crate::i18n::tr(language, "SAVE *"), true)
                                .clicked()
                            {
                                if let Some(profile) = controller.draft.clone() {
                                    actions.push(if profile.id == snapshot.active_id {
                                        PromptStudioAction::ActivateProfile(profile)
                                    } else {
                                        PromptStudioAction::SaveProfile(profile)
                                    });
                                    controller.dirty = false;
                                }
                            }
                        } else {
                            status_chip(ui, crate::i18n::tr(language, "SAVED"));
                        }
                    });
                });
            })
        });
}

fn save_before_switch(
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

fn node_size(node: &PromptNode) -> Vec2 {
    Vec2::new(
        runtime_preview::base_width(&node.kind) + runtime_preview::WIDTH,
        node.layout_height().max(156.0),
    )
}

fn graph_space_rect(position: [f32; 2], size: Vec2) -> Rect {
    Rect::from_min_size(Pos2::new(position[0], position[1]), size)
}

fn node_display_label(node: &PromptNode) -> String {
    if !node.label.trim().is_empty() && node.label != "COMPOSE TEXT" {
        return node.label.trim().to_uppercase();
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
            return truncate_preview(&summary, 32).to_uppercase();
        }
    }
    if node.label.trim().is_empty() {
        block_or_kind_label(&node.kind).into()
    } else {
        node.label.trim().to_uppercase()
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
        PromptNodeKind::Compose { .. } => "COMPOSE TEXT",
        PromptNodeKind::Switch { condition } => condition_name(*condition),
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

fn condition_name(condition: PromptCondition) -> &'static str {
    match condition {
        PromptCondition::SourceIsAuto => "SOURCE IS AUTO",
        PromptCondition::HasReferenceContext => "HAS REFERENCE CONTEXT",
        PromptCondition::HasRecognitionContext => "HAS RECOGNITION CONTEXT",
        PromptCondition::IsPseudoStreaming => "IS PSEUDO-STREAMING",
    }
}

fn input_socket_label(node: &PromptNode, input: u8) -> String {
    match &node.kind {
        PromptNodeKind::Switch {
            condition: PromptCondition::SourceIsAuto,
        } if input == 0 => "EXPLICIT".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::SourceIsAuto,
        } => "AUTO".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::HasReferenceContext,
        } if input == 0 => "NO CONTEXT".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::HasReferenceContext,
        } => "WITH CONTEXT".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::HasRecognitionContext,
        } if input == 0 => "NO CONTEXT".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::HasRecognitionContext,
        } => "WITH CONTEXT".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::IsPseudoStreaming,
        } if input == 0 => "ORDINARY".into(),
        PromptNodeKind::Switch {
            condition: PromptCondition::IsPseudoStreaming,
        } => "PSEUDO-STREAMING".into(),
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

fn status_chip(ui: &mut egui::Ui, text: &str) {
    Frame::new()
        .fill(Color32::from_gray(224))
        .stroke(Stroke::new(1.0, style::BAR_BORDER))
        .corner_radius(CornerRadius::same(1))
        .inner_margin(Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .font(egui::FontId::monospace(9.0))
                    .color(style::INK),
            );
        });
}

fn small_outline_button(ui: &mut egui::Ui, text: &str, tooltip: &str) -> egui::Response {
    style::command_button(ui, text, false).on_hover_text(tooltip)
}

fn small_icon_button(ui: &mut egui::Ui, text: &str, tooltip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(egui::FontId::monospace(12.0))
                .color(style::MUTED),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(1))
        .min_size(Vec2::new(20.0, 20.0)),
    )
    .on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered_branch_graph() -> (PromptNodeGraph, String, String, String, String) {
        let mut graph = PromptNodeGraph::empty();
        let shared = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::CurrentInput,
            [0.0, 0.0],
        );
        let ordinary = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "ordinary {0}".into(),
            [250.0, 0.0],
        );
        let pseudo = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "pseudo {0}".into(),
            [250.0, 200.0],
        );
        let switch = graph.add_switch(
            PromptNodePage::OpenAiCompatible,
            PromptCondition::IsPseudoStreaming,
            [500.0, 100.0],
        );
        let downstream = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "selected {0}".into(),
            [750.0, 100.0],
        );
        assert!(graph.connect(&shared, &ordinary, 0));
        assert!(graph.connect(&shared, &pseudo, 0));
        assert!(graph.connect(&ordinary, &switch, 0));
        assert!(graph.connect(&pseudo, &switch, 1));
        assert!(graph.connect(&switch, &downstream, 0));
        (graph, shared, ordinary, pseudo, switch)
    }

    #[test]
    fn new_node_position_avoids_the_existing_node_bounds() {
        let controller = PromptStudioController::default();
        let mut graph = PromptNodeGraph::empty();
        let first = controller.new_node_position(&graph);
        graph.add_compose(PromptNodePage::OpenAiCompatible, "{0}".into(), first);

        let second = controller.new_node_position(&graph);
        let existing = &graph.nodes[0];

        assert!(
            !graph_space_rect(second, Vec2::new(320.0, 220.0))
                .expand(8.0)
                .intersects(graph_space_rect(existing.position, node_size(existing)).expand(8.0))
        );
    }

    #[test]
    fn custom_compose_title_follows_its_first_text_line() {
        let mut graph = PromptNodeGraph::empty();
        graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "Translate {0} naturally.\nIgnored second line".into(),
            [0.0, 0.0],
        );

        assert_eq!(node_display_label(&graph.nodes[0]), "TRANSLATE NATURALLY");

        graph.nodes[0].label = "Context-aware translation".into();
        assert_eq!(
            node_display_label(&graph.nodes[0]),
            "CONTEXT-AWARE TRANSLATION"
        );
    }

    #[test]
    fn a_new_profile_contains_both_provider_pages() {
        let profile = new_profile();

        assert!(!profile.read_only);
        assert!(profile.graph.validate_for_activation().is_ok());
        for target in [
            PromptProviderTarget::OpenAiCompatible,
            PromptProviderTarget::Hunyuan,
            PromptProviderTarget::AsrInstruction,
            PromptProviderTarget::AsrContextBias,
        ] {
            assert!(profile.graph.nodes.iter().any(|node| {
                matches!(
                    node.kind,
                    PromptNodeKind::Request { target: value, .. } if value == target
                )
            }));
        }
    }

    #[test]
    fn provider_tabs_show_matching_provider_nodes_only() {
        let mut controller = PromptStudioController::default();
        let graph = PromptNodeGraph::builtin_default();

        assert!(
            controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "openai-reference-context")
                    .unwrap()
            )
        );
        assert!(
            !controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "hunyuan-reference-context")
                    .unwrap()
            )
        );
        assert!(
            controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "openai-request")
                    .unwrap()
            )
        );
        assert!(
            !controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "hunyuan-request")
                    .unwrap()
            )
        );

        controller.select_provider(PromptProviderTarget::Hunyuan);

        assert!(
            !controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "openai-reference-context")
                    .unwrap()
            )
        );
        assert!(
            controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "hunyuan-reference-context")
                    .unwrap()
            )
        );
        assert!(
            !controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "openai-request")
                    .unwrap()
            )
        );
        assert!(
            controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "hunyuan-request")
                    .unwrap()
            )
        );
    }

    #[test]
    fn branch_filters_are_discovered_from_switches_on_the_active_page() {
        let mut controller = PromptStudioController::default();
        let graph = PromptNodeGraph::builtin_default();

        controller.sync_branch_filters(&graph);

        let conditions = controller.branch_conditions();
        assert!(conditions.contains(&PromptCondition::IsPseudoStreaming));
        assert!(conditions.contains(&PromptCondition::SourceIsAuto));
        assert!(conditions.contains(&PromptCondition::HasReferenceContext));
        assert!(!conditions.contains(&PromptCondition::HasRecognitionContext));
    }

    #[test]
    fn pseudo_branch_filter_hides_only_the_ordinary_branch() {
        let (graph, shared, ordinary, pseudo, switch) = filtered_branch_graph();
        let mut controller = PromptStudioController::default();
        controller.sync_branch_filters(&graph);

        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(true));

        assert!(
            !controller
                .node_is_visible(graph.nodes.iter().find(|node| node.id == ordinary).unwrap())
        );
        for id in [shared, pseudo, switch] {
            assert!(
                controller.node_is_visible(graph.nodes.iter().find(|node| node.id == id).unwrap())
            );
        }
        assert!(controller.node_is_visible(
            graph
                .nodes
                .iter()
                .find(|node| matches!(node.kind, PromptNodeKind::Compose { ref text } if text.starts_with("selected")))
                .unwrap()
        ));
    }

    #[test]
    fn ordinary_branch_filter_hides_only_the_pseudo_branch() {
        let (graph, shared, ordinary, pseudo, switch) = filtered_branch_graph();
        let mut controller = PromptStudioController::default();
        controller.sync_branch_filters(&graph);

        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(false));

        assert!(
            !controller.node_is_visible(graph.nodes.iter().find(|node| node.id == pseudo).unwrap())
        );
        for id in [shared, ordinary, switch] {
            assert!(
                controller.node_is_visible(graph.nodes.iter().find(|node| node.id == id).unwrap())
            );
        }
    }

    #[test]
    fn builtin_mode_filter_keeps_common_request_and_selected_compose_nodes() {
        let graph = PromptNodeGraph::builtin_default();
        let mut controller = PromptStudioController::default();
        controller.sync_branch_filters(&graph);

        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(true));

        for id in [
            "openai-explicit-instruction-pseudo-streaming",
            "openai-explicit-instruction",
            "openai-request",
        ] {
            assert!(
                controller.node_is_visible(graph.nodes.iter().find(|node| node.id == id).unwrap())
            );
        }
        assert!(
            !controller.node_is_visible(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == "openai-explicit-instruction-ordinary")
                    .unwrap()
            )
        );
    }

    #[test]
    fn provider_switch_discards_filters_for_unavailable_conditions() {
        let (mut graph, _, _, _, _) = filtered_branch_graph();
        let hunyuan_false =
            graph.add_compose(PromptNodePage::Hunyuan, "explicit".into(), [0.0, 400.0]);
        let hunyuan_true =
            graph.add_compose(PromptNodePage::Hunyuan, "auto".into(), [250.0, 400.0]);
        let hunyuan_switch = graph.add_switch(
            PromptNodePage::Hunyuan,
            PromptCondition::SourceIsAuto,
            [500.0, 400.0],
        );
        assert!(graph.connect(&hunyuan_false, &hunyuan_switch, 0));
        assert!(graph.connect(&hunyuan_true, &hunyuan_switch, 1));
        let mut profile = new_profile();
        profile.graph = graph;
        let mut controller = PromptStudioController::default();
        controller.draft = Some(profile);
        controller.sync_current_branch_filters();
        let graph = controller.draft.as_ref().unwrap().graph.clone();
        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(true));

        controller.select_provider(PromptProviderTarget::Hunyuan);

        assert_eq!(
            controller.branch_conditions(),
            vec![PromptCondition::SourceIsAuto]
        );
        assert!(
            !controller
                .branch_filters
                .contains_key(&PromptCondition::IsPseudoStreaming)
        );
    }

    #[test]
    fn multiple_branch_filters_hide_each_opposite_branch() {
        let (mut graph, shared, ordinary, pseudo, _) = filtered_branch_graph();
        let explicit = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "explicit source".into(),
            [0.0, 500.0],
        );
        let auto = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "auto source".into(),
            [250.0, 500.0],
        );
        let source_switch = graph.add_switch(
            PromptNodePage::OpenAiCompatible,
            PromptCondition::SourceIsAuto,
            [500.0, 500.0],
        );
        assert!(graph.connect(&explicit, &source_switch, 0));
        assert!(graph.connect(&auto, &source_switch, 1));
        let mut controller = PromptStudioController::default();
        controller.sync_branch_filters(&graph);

        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(true));
        controller.set_branch_filter(&graph, PromptCondition::SourceIsAuto, Some(true));

        for id in [ordinary, explicit] {
            assert!(
                !controller.node_is_visible(graph.nodes.iter().find(|node| node.id == id).unwrap())
            );
        }
        for id in [shared, pseudo, auto, source_switch] {
            assert!(
                controller.node_is_visible(graph.nodes.iter().find(|node| node.id == id).unwrap())
            );
        }
    }

    #[test]
    fn changing_branch_visibility_does_not_mutate_the_graph_or_dirty_state() {
        let (graph, _, _, _, _) = filtered_branch_graph();
        let fingerprint = graph.fingerprint();
        let mut profile = new_profile();
        profile.graph = graph;
        let mut controller = PromptStudioController::default();
        controller.draft = Some(profile);
        controller.dirty = false;
        controller.sync_current_branch_filters();
        let graph = controller.draft.as_ref().unwrap().graph.clone();

        controller.set_branch_filter(&graph, PromptCondition::IsPseudoStreaming, Some(true));

        assert_eq!(
            controller.draft.as_ref().unwrap().graph.fingerprint(),
            fingerprint
        );
        assert!(!controller.is_dirty());
        assert!(!controller.can_undo());
    }

    #[test]
    fn runtime_provider_selects_the_initial_graph_page() {
        let mut controller = PromptStudioController::for_provider(PromptProviderTarget::Hunyuan);
        assert_eq!(controller.active_provider, PromptProviderTarget::Hunyuan);

        controller.sync_provider(PromptProviderTarget::OpenAiCompatible);
        assert_eq!(
            controller.active_provider,
            PromptProviderTarget::OpenAiCompatible
        );
    }

    #[test]
    fn controller_undo_and_redo_tracks_draft_mutations() {
        let mut controller = PromptStudioController::default();
        let mut profile = new_profile();
        profile.name = "Initial".to_string();
        controller.set_draft(profile.clone());

        assert!(!controller.can_undo());
        assert!(!controller.can_redo());

        // Simulate a mutation
        let before = profile.clone();
        let mut mutated = profile.clone();
        mutated.name = "Renamed".to_string();
        mutated.graph.add_variable(
            PromptNodePage::OpenAiCompatible,
            PromptVariable::CurrentInput,
            [50.0, 50.0],
        );
        controller.draft = Some(mutated.clone());
        controller.push_history(before.clone());

        assert!(controller.can_undo());
        assert!(!controller.can_redo());

        // Undo
        controller.undo();
        assert_eq!(controller.draft.as_ref().unwrap().name, "Initial");
        assert!(controller.can_redo());
        assert!(!controller.can_undo());

        // Redo
        controller.redo();
        assert_eq!(controller.draft.as_ref().unwrap().name, "Renamed");
        assert!(controller.can_undo());
        assert!(!controller.can_redo());
    }

    #[test]
    fn domain_switch_keeps_the_same_unified_draft_and_history() {
        let library = PromptTemplateLibrary::default();
        let mut controller = PromptStudioController::default();
        let mut profile = controller.snapshot(&library).draft;
        profile.read_only = false;
        let mut changed = profile.clone();
        changed.name = "Unified draft".into();
        controller.draft = Some(changed);
        controller.push_history(profile.clone());

        controller.switch_domain(PromptGraphDomain::Asr);

        assert_eq!(controller.snapshot(&library).draft.name, "Unified draft");
        assert!(controller.can_undo());
        assert_eq!(
            controller.active_provider,
            PromptProviderTarget::AsrInstruction
        );
    }

    #[test]
    fn controller_start_and_cancel_title_editing() {
        let mut controller = PromptStudioController::default();
        let profile = new_profile();
        controller.set_draft(profile.clone());

        controller.start_editing_title(
            "node-1".to_string(),
            "My Node".to_string(),
            profile.clone(),
        );
        assert_eq!(
            controller.editing_title,
            Some(NodeTitleEdit {
                node_id: "node-1".to_string(),
                text: "My Node".to_string(),
            })
        );
        assert_eq!(controller.title_edit_start_profile, Some(profile));

        controller.cancel_editing_title();
        assert!(controller.editing_title.is_none());
        assert!(controller.title_edit_start_profile.is_none());
    }

    #[test]
    fn adding_and_connecting_custom_compose_to_translation_context_succeeds() {
        let mut profile = new_profile();
        let compose_id = profile.graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "Write prompt text here: {0}".into(),
            [150.0, 150.0],
        );
        assert!(
            profile
                .graph
                .connect(&compose_id, "openai-reference-context", 0)
        );
        assert!(
            profile.graph.links.iter().any(|l| l.from == compose_id
                && l.to == "openai-reference-context"
                && l.input == 0)
        );
    }

    #[test]
    fn wire_dragging_near_edge_triggers_auto_pan_and_dynamic_zoom() {
        let context = egui::Context::default();
        let mut controller = PromptStudioController::default();
        controller.zoom = 1.0;
        controller.pan = Vec2::ZERO;

        let canvas = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(800.0, 600.0));

        // When pointer is near right edge (e.g. x = 880, canvas right is 900)
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 768.0)));
        input.predicted_dt = 1.0 / 60.0;
        input
            .events
            .push(egui::Event::PointerMoved(Pos2::new(880.0, 300.0)));

        let mut output = context.run_ui(input, |ui| {
            navigation::update_wire_dragging_navigation(&mut controller, canvas, ui, true);
        });
        output.textures_delta.clear();

        // Pan should have moved left (pan.x < 0) to reveal nodes to the right, and zoom out slightly
        assert!(controller.pan.x < 0.0);
        assert!(controller.zoom < 1.0);
        assert_eq!(controller.wire_base_zoom, Some(1.0));
    }
}
