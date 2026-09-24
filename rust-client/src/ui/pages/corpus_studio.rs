//! Interactive view of the persistent XR Corpus vocabulary graph.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{self, Color32, CornerRadius, Frame, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use xr_corpus_client::{
    CorpusClient,
    protocol::{
        CORPUS_LANGUAGE_ORDER as LANGUAGE_CODES, CorpusActivation, GraphDomain, GraphEdge,
        GraphEdgeKind, GraphNode, GraphNodeStatePatch as NodeState, GraphPosition as NodePosition,
        GraphSnapshot,
    },
};

use crate::{
    backend::{BackendManager, BackendStart},
    i18n::{UiLanguage, tr},
    ui::{
        components, graph_canvas,
        graph_editor::{
            ForceLayout, ForceNode, GraphEditorState, LayeredLayoutOptions, LayoutNode,
            closest_link, layered_layout, nearest_port,
        },
        graph_style,
    },
};

const NODE_SIZE: Vec2 = Vec2::new(168.0, 66.0);
const MAX_VISIBLE_NODES: usize = 80;
const CORPUS_URL: &str = "http://127.0.0.1:7766";

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
enum CorpusView {
    #[default]
    Graph,
    Layered,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct EdgeKey {
    source_id: String,
    target_id: String,
    kind: GraphEdgeKind,
}

fn edge_key(edge: &GraphEdge) -> EdgeKey {
    EdgeKey {
        source_id: edge.source_id.clone(),
        target_id: edge.target_id.clone(),
        kind: edge.kind,
    }
}

enum Command {
    Refresh,
    PutDomain(GraphDomain),
    DeleteDomain(String),
    PutNode(GraphNode),
    SaveNode(GraphNode),
    DeleteNode(String),
    PutEdge(GraphEdge),
    DeleteEdge(GraphEdge),
    SavePositions(Vec<NodePosition>),
    SetNodeState(NodeState),
}

enum PendingEnabled {
    Domain(String, bool),
    Node(String, bool),
    Edge(EdgeKey, bool),
    Nodes(HashSet<String>, bool),
}

impl Command {
    fn saves_node(&self) -> bool {
        matches!(self, Self::SaveNode(_))
    }
}

async fn execute(client: &CorpusClient, command: &Command) -> Result<GraphSnapshot, String> {
    match command {
        Command::Refresh => client.graph().await,
        Command::PutDomain(domain) => client.save_domain(domain).await,
        Command::DeleteDomain(id) => client.remove_domain(id).await,
        Command::PutNode(node) | Command::SaveNode(node) => client.save_node(node).await,
        Command::DeleteNode(id) => client.remove_node(id).await,
        Command::PutEdge(edge) => client.save_edge(edge).await,
        Command::DeleteEdge(edge) => client.remove_edge(edge).await,
        Command::SavePositions(positions) => client.save_positions(positions).await,
        Command::SetNodeState(state) => client.patch_node_state(state).await,
    }
    .map_err(|error| error.to_string())
}

fn start_worker(ctx: egui::Context) -> (Sender<Command>, Receiver<Result<GraphSnapshot, String>>) {
    let (commands_tx, commands_rx) = unbounded();
    let (results_tx, results_rx) = unbounded();
    std::thread::Builder::new()
        .name("xr-corpus-studio".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = results_tx.send(Err(error.to_string()));
                    ctx.request_repaint();
                    return;
                }
            };
            let client = match CorpusClient::new(CORPUS_URL) {
                Ok(client) => client,
                Err(error) => {
                    let _ = results_tx.send(Err(error.to_string()));
                    ctx.request_repaint();
                    return;
                }
            };
            while let Ok(command) = commands_rx.recv() {
                let result = runtime.block_on(execute(&client, &command));
                if results_tx.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .expect("failed to start XR Corpus Studio worker");
    (commands_tx, results_rx)
}

pub(crate) struct CorpusStudioController {
    snapshot: Option<Arc<GraphSnapshot>>,
    editor: GraphEditorState<String, EdgeKey>,
    view: CorpusView,
    layout: ForceLayout<String>,
    layout_signature: Option<u64>,
    layout_running: bool,
    fit_entire_graph: bool,
    layout_cache: HashMap<CorpusView, HashMap<String, [f32; 2]>>,
    layout_view: CorpusView,
    focus_connections: bool,
    graph_page: usize,
    row_selection: HashSet<String>,
    selected_domain: Option<String>,
    selected_node: Option<String>,
    selected_edge: Option<EdgeKey>,
    node_draft: Option<GraphNode>,
    new_node: bool,
    draft_dirty: bool,
    domain_search: String,
    node_search: String,
    new_domain_title: String,
    domain_title_edit: String,
    adding_domain: bool,
    editing_domain: bool,
    confirm_delete_domain: bool,
    edge_target: String,
    edge_outgoing: bool,
    edge_kind: GraphEdgeKind,
    confirm_delete: bool,
    pending: bool,
    pending_node_save: bool,
    pending_enabled: Option<PendingEnabled>,
    error: Option<String>,
    service_ready: bool,
    service_error: Option<String>,
    next_service_check: Option<Instant>,
    worker_tx: Option<Sender<Command>>,
    worker_rx: Option<Receiver<Result<GraphSnapshot, String>>>,
}

impl Default for CorpusStudioController {
    fn default() -> Self {
        Self {
            snapshot: None,
            editor: GraphEditorState::default(),
            view: CorpusView::default(),
            layout: ForceLayout::default(),
            layout_signature: None,
            layout_running: false,
            fit_entire_graph: false,
            layout_cache: HashMap::new(),
            layout_view: CorpusView::Graph,
            focus_connections: false,
            graph_page: 0,
            row_selection: HashSet::new(),
            selected_domain: None,
            selected_node: None,
            selected_edge: None,
            node_draft: None,
            new_node: false,
            draft_dirty: false,
            domain_search: String::new(),
            node_search: String::new(),
            new_domain_title: String::new(),
            domain_title_edit: String::new(),
            adding_domain: false,
            editing_domain: false,
            confirm_delete_domain: false,
            edge_target: String::new(),
            edge_outgoing: true,
            edge_kind: GraphEdgeKind::Trigger,
            confirm_delete: false,
            pending: false,
            pending_node_save: false,
            pending_enabled: None,
            error: None,
            service_ready: false,
            service_error: None,
            next_service_check: None,
            worker_tx: None,
            worker_rx: None,
        }
    }
}

impl CorpusStudioController {
    fn ensure_started(&mut self, ctx: &egui::Context) {
        if self.worker_tx.is_none() {
            let (tx, rx) = start_worker(ctx.clone());
            self.worker_tx = Some(tx);
            self.worker_rx = Some(rx);
        }
    }

    fn ensure_service(&mut self, backend: &mut BackendManager, ctx: &egui::Context) {
        if self.service_ready || self.service_error.is_some() {
            return;
        }
        let now = Instant::now();
        if let Some(next) = self.next_service_check
            && next > now
        {
            ctx.request_repaint_after(next - now);
            return;
        }
        match backend.prepare_corpus() {
            Ok(BackendStart::Ready) => {
                self.service_ready = true;
                self.next_service_check = None;
                self.ensure_started(ctx);
                self.send(Command::Refresh);
            }
            Ok(BackendStart::Starting(_)) => {
                self.next_service_check = Some(now + Duration::from_millis(250));
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            Err(error) => self.service_error = Some(error),
        }
    }

    fn send(&mut self, command: Command) {
        if self.pending {
            return;
        }
        self.pending_enabled = match &command {
            Command::PutDomain(domain) => {
                Some(PendingEnabled::Domain(domain.id.clone(), domain.enabled))
            }
            Command::PutNode(node) => Some(PendingEnabled::Node(node.id.clone(), node.enabled)),
            Command::PutEdge(edge) => Some(PendingEnabled::Edge(edge_key(edge), edge.enabled)),
            Command::SetNodeState(state) => state
                .enabled
                .map(|enabled| PendingEnabled::Nodes(state.ids.iter().cloned().collect(), enabled)),
            _ => None,
        };
        self.pending_node_save = command.saves_node();
        self.pending = true;
        self.error = None;
        if self
            .worker_tx
            .as_ref()
            .is_none_or(|tx| tx.send(command).is_err())
        {
            self.pending = false;
            self.pending_enabled = None;
            self.revert_immediate_node_fields();
            self.error = Some("XR Corpus worker is unavailable".into());
            self.worker_tx = None;
            self.worker_rx = None;
        }
    }

    fn poll(&mut self) {
        let result = self.worker_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        let Some(result) = result else { return };
        self.pending = false;
        self.pending_enabled = None;
        match result {
            Ok(snapshot) => {
                let ids = snapshot
                    .nodes
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect::<HashSet<_>>();
                self.row_selection.retain(|id| ids.contains(id.as_str()));
                for cache in self.layout_cache.values_mut() {
                    cache.retain(|id, _| ids.contains(id.as_str()));
                }
                if self.snapshot.is_none() && self.selected_domain.is_none() {
                    self.selected_domain = snapshot.domains.first().map(|domain| domain.id.clone());
                    self.domain_title_edit = snapshot
                        .domains
                        .first()
                        .map_or(String::new(), |domain| domain.title.clone());
                }
                if self.pending_node_save {
                    self.draft_dirty = false;
                    self.new_node = false;
                }
                if !self.draft_dirty {
                    self.node_draft = self
                        .selected_node
                        .as_ref()
                        .and_then(|id| snapshot.nodes.iter().find(|node| &node.id == id).cloned());
                }
                if self
                    .selected_domain
                    .as_ref()
                    .is_some_and(|id| !snapshot.domains.iter().any(|d| &d.id == id))
                {
                    self.selected_domain = None;
                    self.domain_title_edit.clear();
                }
                if self
                    .selected_node
                    .as_ref()
                    .is_some_and(|id| !snapshot.nodes.iter().any(|n| &n.id == id))
                {
                    self.selected_node = None;
                    self.node_draft = None;
                    self.new_node = false;
                    self.draft_dirty = false;
                    self.editor.clear_selection();
                }
                if self
                    .selected_edge
                    .as_ref()
                    .is_some_and(|key| !snapshot.edges.iter().any(|edge| edge_key(edge) == *key))
                {
                    self.selected_edge = None;
                    self.editor.clear_selection();
                }
                self.snapshot = Some(Arc::new(snapshot));
                self.error = None;
            }
            Err(error) => {
                self.revert_immediate_node_fields();
                if self.selected_domain.as_ref().is_some_and(|id| {
                    self.snapshot.as_ref().is_some_and(|snapshot| {
                        !snapshot.domains.iter().any(|domain| &domain.id == id)
                    })
                }) {
                    self.select_domain(None);
                }
                self.error = Some(error);
            }
        }
    }

    fn revert_immediate_node_fields(&mut self) {
        if !self.pending_node_save
            && let (Some(draft), Some(snapshot)) = (&mut self.node_draft, &self.snapshot)
            && let Some(saved) = snapshot.nodes.iter().find(|node| node.id == draft.id)
        {
            draft.enabled = saved.enabled;
            draft.activation = saved.activation.clone();
            draft.promptable = saved.promptable;
        }
    }

    fn displayed_domain_enabled(&self, domain: &GraphDomain) -> bool {
        match &self.pending_enabled {
            Some(PendingEnabled::Domain(id, enabled)) if id == &domain.id => *enabled,
            _ => domain.enabled,
        }
    }

    fn displayed_node_enabled(&self, node: &GraphNode) -> bool {
        match &self.pending_enabled {
            Some(PendingEnabled::Node(id, enabled)) if id == &node.id => *enabled,
            Some(PendingEnabled::Nodes(ids, enabled)) if ids.contains(&node.id) => *enabled,
            _ => node.enabled,
        }
    }

    fn displayed_edge_enabled(&self, edge: &GraphEdge) -> bool {
        match &self.pending_enabled {
            Some(PendingEnabled::Edge(key, enabled)) if *key == edge_key(edge) => *enabled,
            _ => edge.enabled,
        }
    }

    fn select_node(&mut self, node: &GraphNode) {
        if self.draft_dirty {
            return;
        }
        self.selected_node = Some(node.id.clone());
        self.selected_edge = None;
        self.node_draft = Some(node.clone());
        self.new_node = false;
        self.draft_dirty = false;
        self.confirm_delete = false;
        self.edge_target.clear();
        self.editor.select_node(node.id.clone(), false);
        if self.view != CorpusView::List && !self.layout.positions.contains_key(&node.id) {
            self.focus_connections = true;
            self.graph_page = 0;
            self.editor.canvas.fit_pending = true;
        }
    }

    fn select_domain(&mut self, domain: Option<&GraphDomain>) {
        if self.draft_dirty {
            return;
        }
        self.selected_domain = domain.map(|domain| domain.id.clone());
        self.focus_connections = false;
        self.graph_page = 0;
        self.layout_signature = None;
        self.row_selection.clear();
        self.domain_title_edit = domain.map_or(String::new(), |domain| domain.title.clone());
        self.selected_node = None;
        self.selected_edge = None;
        self.node_draft = None;
        self.new_node = false;
        self.editing_domain = false;
        self.confirm_delete_domain = false;
        self.confirm_delete = false;
        self.edge_target.clear();
        self.editor.clear_selection();
        self.editor.canvas.fit_pending = true;
    }

    fn create_node(&mut self) {
        if self.draft_dirty {
            return;
        }
        let Some(domain_id) = self.selected_domain.clone().or_else(|| {
            self.snapshot
                .as_ref()?
                .domains
                .first()
                .map(|domain| domain.id.clone())
        }) else {
            return;
        };
        self.new_node = true;
        self.selected_node = None;
        self.selected_edge = None;
        self.editor.clear_selection();
        self.node_draft = Some(GraphNode {
            id: uuid::Uuid::new_v4().to_string(),
            domain_id,
            subdomain: "custom".into(),
            title: String::new(),
            enabled: true,
            promptable: true,
            activation: CorpusActivation::Always,
            priority: 20,
            values: vec![String::new(); LANGUAGE_CODES.len()],
        });
        self.draft_dirty = true;
    }

    fn create_missing_node(&mut self, id: &str, domain_id: Option<&str>) {
        self.create_node();
        if let Some(draft) = &mut self.node_draft {
            draft.id = id.to_owned();
            if self.selected_domain.is_none()
                && let Some(domain_id) = domain_id
            {
                draft.domain_id = domain_id.to_owned();
            }
        }
    }

    fn position_changes(&self, snapshot: &GraphSnapshot) -> Vec<NodePosition> {
        let saved = snapshot
            .positions
            .iter()
            .map(|p| (p.id.as_str(), [p.x, p.y]))
            .collect::<HashMap<_, _>>();
        self.layout_cache
            .get(&self.view)
            .into_iter()
            .flat_map(|cache| cache.iter())
            .filter(|(id, p)| {
                saved.get(id.as_str()) != Some(*p)
                    && snapshot.nodes.iter().any(|node| &node.id == *id)
            })
            .map(|(id, p)| NodePosition {
                id: id.clone(),
                x: p[0],
                y: p[1],
            })
            .collect()
    }

    fn set_node_enabled(&mut self, node: &GraphNode, enabled: bool) {
        if self.selected_node.as_deref() == Some(&node.id)
            && let Some(draft) = &mut self.node_draft
        {
            draft.enabled = enabled;
        }
        self.send(Command::SetNodeState(NodeState {
            ids: vec![node.id.clone()],
            enabled: Some(enabled),
            domain_id: None,
        }));
    }
}

pub(crate) fn render(
    controller: &mut CorpusStudioController,
    backend: &mut BackendManager,
    ui: &mut egui::Ui,
    language: UiLanguage,
) {
    controller.poll();
    controller.ensure_service(backend, ui.ctx());
    ui.scope(|ui| {
        ui.horizontal(|ui| {
            ui.heading(tr(language, "Vocabulary Graph"));
            ui.label(format!(
                "{} · {}",
                controller.snapshot.as_ref().map_or(0, |s| s.nodes.len()),
                tr(language, "terms")
            ));
            if controller.pending {
                ui.spinner();
            }
            if components::secondary_button_enabled(
                ui,
                tr(language, "Refresh"),
                !controller.pending,
            )
            .clicked()
            {
                controller.service_ready = false;
                controller.service_error = None;
                controller.next_service_check = None;
                controller.ensure_service(backend, ui.ctx());
            }
        });
        if let Some(error) = controller
            .service_error
            .as_ref()
            .or(controller.error.as_ref())
        {
            ui.colored_label(graph_style::ERROR_BORDER, error);
        }
        if controller.draft_dirty {
            ui.small(tr(
                language,
                "Save or discard term changes before switching.",
            ));
        }
        ui.add_space(6.0);
        let Some(snapshot) = controller.snapshot.clone() else {
            ui.label(tr(
                language,
                "Waiting for XR Corpus. Use Refresh to retry startup.",
            ));
            return;
        };
        let height = ui.available_height().max(360.0);
        if ui.available_width() < 820.0 {
            egui::ScrollArea::vertical()
                .id_salt("corpus_narrow_page")
                .show(ui, |ui| {
                    egui::CollapsingHeader::new(tr(language, "Domains and terms"))
                        .default_open(true)
                        .show(ui, |ui| {
                            render_browser(controller, &snapshot, ui, language, 230.0)
                        });
                    render_workspace(controller, &snapshot, ui, language, height.min(460.0));
                    egui::CollapsingHeader::new(tr(language, "Term details"))
                        .default_open(true)
                        .show(ui, |ui| {
                            render_inspector(controller, &snapshot, ui, language, 460.0)
                        });
                });
        } else {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(215.0, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("corpus_browser_panel")
                            .max_height(height)
                            .show(ui, |ui| {
                                render_browser(controller, &snapshot, ui, language, height)
                            });
                    },
                );
                let show_inspector =
                    controller.node_draft.is_some() || controller.selected_edge.is_some();
                let canvas_width =
                    (ui.available_width() - if show_inspector { 294.0 } else { 0.0 }).max(280.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(canvas_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| render_workspace(controller, &snapshot, ui, language, height),
                );
                if show_inspector {
                    ui.allocate_ui_with_layout(
                        Vec2::new(284.0, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| render_inspector(controller, &snapshot, ui, language, height),
                    );
                }
            });
        }
    });
}

fn render_browser(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    components::section_heading(ui, tr(language, "Domains"));
    ui.add(
        egui::TextEdit::singleline(&mut controller.domain_search)
            .hint_text(tr(language, "Search domains")),
    );
    egui::ScrollArea::vertical()
        .id_salt("corpus_domains")
        .max_height((height * 0.27).max(100.0))
        .show(ui, |ui| {
            if ui
                .add_enabled_ui(!controller.draft_dirty, |ui| {
                    ui.selectable_label(
                        controller.selected_domain.is_none(),
                        tr(language, "All domains"),
                    )
                })
                .inner
                .clicked()
            {
                controller.select_domain(None);
            }
            for domain in &snapshot.domains {
                if !contains(&domain.id, &controller.domain_search)
                    && !contains(&domain.title, &controller.domain_search)
                {
                    continue;
                }
                ui.push_id(&domain.id, |ui| {
                    ui.horizontal(|ui| {
                        let mut enabled = controller.displayed_domain_enabled(domain);
                        if ui
                            .add_enabled_ui(!controller.pending, |ui| {
                                components::pill_toggle(ui, &mut enabled)
                            })
                            .inner
                            .on_hover_text(tr(language, "Enabled"))
                            .changed()
                        {
                            controller.send(Command::PutDomain(GraphDomain {
                                enabled,
                                ..domain.clone()
                            }));
                        }
                        if ui
                            .add_enabled_ui(!controller.draft_dirty, |ui| {
                                ui.selectable_label(
                                    controller.selected_domain.as_deref() == Some(&domain.id),
                                    &domain.title,
                                )
                            })
                            .inner
                            .on_hover_text(&domain.title)
                            .clicked()
                        {
                            controller.select_domain(Some(domain));
                        }
                        if controller.selected_domain.as_deref() == Some(&domain.id)
                            && !controller.draft_dirty
                        {
                            ui.menu_button("⋯", |ui| {
                                if ui.button(tr(language, "Rename")).clicked() {
                                    controller.editing_domain = true;
                                    controller.confirm_delete_domain = false;
                                }
                                if !snapshot
                                    .nodes
                                    .iter()
                                    .any(|node| node.domain_id == domain.id)
                                    && ui.button(tr(language, "Delete empty domain")).clicked()
                                {
                                    controller.confirm_delete_domain = true;
                                    controller.editing_domain = false;
                                }
                            });
                        }
                    })
                });
            }
        });
    if components::secondary_button(ui, tr(language, "+ Domain")).clicked() {
        controller.adding_domain = !controller.adding_domain;
    }
    if controller.adding_domain {
        ui.add(
            egui::TextEdit::singleline(&mut controller.new_domain_title)
                .hint_text(tr(language, "Domain name"))
                .char_limit(256),
        );
        if components::primary_button_enabled(
            ui,
            tr(language, "Create"),
            !controller.pending
                && !controller.draft_dirty
                && !controller.new_domain_title.trim().is_empty()
                && controller.new_domain_title.chars().count() <= 256,
        )
        .clicked()
        {
            let base = controller
                .new_domain_title
                .split(|character: char| !character.is_alphanumeric())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-")
                .to_lowercase()
                .chars()
                .take(220)
                .collect::<String>();
            let id = if base.is_empty() {
                format!("domain-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
            } else if snapshot.domains.iter().any(|domain| domain.id == base) {
                format!("{base}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
            } else {
                base
            };
            let domain = GraphDomain {
                id,
                title: controller.new_domain_title.trim().to_owned(),
                enabled: true,
            };
            controller.select_domain(Some(&domain));
            controller.send(Command::PutDomain(domain));
            controller.new_domain_title.clear();
            controller.adding_domain = false;
        }
    }
    if let Some(domain) = controller
        .selected_domain
        .as_ref()
        .and_then(|id| snapshot.domains.iter().find(|domain| &domain.id == id))
    {
        if controller.editing_domain {
            ui.add(
                egui::TextEdit::singleline(&mut controller.domain_title_edit)
                    .hint_text(tr(language, "Domain name"))
                    .char_limit(256),
            );
            ui.horizontal(|ui| {
                if components::primary_button_enabled(
                    ui,
                    tr(language, "Rename"),
                    !controller.pending
                        && !controller.domain_title_edit.trim().is_empty()
                        && controller.domain_title_edit.chars().count() <= 256
                        && controller.domain_title_edit.trim() != domain.title,
                )
                .clicked()
                {
                    controller.send(Command::PutDomain(GraphDomain {
                        title: controller.domain_title_edit.trim().to_owned(),
                        ..domain.clone()
                    }));
                    controller.editing_domain = false;
                }
                if components::secondary_button(ui, tr(language, "Cancel")).clicked() {
                    controller.editing_domain = false;
                    controller.domain_title_edit = domain.title.clone();
                }
            });
        }
        if controller.confirm_delete_domain {
            ui.horizontal(|ui| {
                if components::danger_button_enabled(
                    ui,
                    tr(language, "Confirm delete"),
                    !controller.pending,
                )
                .clicked()
                {
                    controller.send(Command::DeleteDomain(domain.id.clone()));
                    controller.confirm_delete_domain = false;
                }
                if components::secondary_button(ui, tr(language, "Cancel")).clicked() {
                    controller.confirm_delete_domain = false;
                }
            });
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        components::section_heading(ui, tr(language, "Terms"));
        if ui
            .add_enabled(
                !snapshot.domains.is_empty() && !controller.draft_dirty,
                egui::Button::new("+"),
            )
            .on_hover_text(tr(language, "New term"))
            .clicked()
        {
            controller.create_node();
        }
    });
    if ui
        .add(
            egui::TextEdit::singleline(&mut controller.node_search)
                .hint_text(tr(language, "Search terms")),
        )
        .changed()
    {
        controller.graph_page = 0;
        controller.focus_connections = false;
        controller.row_selection.clear();
    }
    let node_ids = snapshot
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let selected_domain = controller.selected_domain.as_deref();
    let idle_edges = snapshot
        .edges
        .iter()
        .filter(|edge| {
            (!node_ids.contains(edge.source_id.as_str())
                || !node_ids.contains(edge.target_id.as_str()))
                && selected_domain.is_none_or(|domain_id| {
                    snapshot.nodes.iter().any(|node| {
                        node.domain_id == domain_id
                            && (node.id == edge.source_id || node.id == edge.target_id)
                    })
                })
                && (controller.node_search.trim().is_empty()
                    || contains(&edge.source_id, &controller.node_search)
                    || contains(&edge.target_id, &controller.node_search)
                    || snapshot.nodes.iter().any(|node| {
                        (node.id == edge.source_id || node.id == edge.target_id)
                            && matches_node(node, &controller.node_search)
                    }))
        })
        .collect::<Vec<_>>();
    if controller.view != CorpusView::List {
        egui::ScrollArea::vertical()
            .id_salt("corpus_terms")
            .max_height((height * if idle_edges.is_empty() { 0.53 } else { 0.38 }).max(150.0))
            .show(ui, |ui| {
                let matching = snapshot
                    .nodes
                    .iter()
                    .filter(|node| {
                        controller
                            .selected_domain
                            .as_ref()
                            .is_none_or(|id| &node.domain_id == id)
                    })
                    .filter(|node| matches_node(node, &controller.node_search))
                    .collect::<Vec<_>>();
                for node in matching {
                    let text = if controller.displayed_node_enabled(node) {
                        node_label(node, language).to_owned()
                    } else {
                        format!("{} · {}", node_label(node, language), tr(language, "off"))
                    };
                    ui.push_id(&node.id, |ui| {
                        ui.horizontal(|ui| {
                            let mut enabled = controller.displayed_node_enabled(node);
                            if ui
                                .add_enabled_ui(!controller.pending, |ui| {
                                    components::pill_toggle(ui, &mut enabled)
                                })
                                .inner
                                .on_hover_text(tr(language, "Enabled"))
                                .changed()
                            {
                                controller.set_node_enabled(node, enabled);
                            }
                            let domain_title = domain_label(snapshot, &node.domain_id);
                            if ui
                                .add_enabled_ui(!controller.draft_dirty, |ui| {
                                    ui.selectable_label(
                                        controller.selected_node.as_deref() == Some(&node.id),
                                        text,
                                    )
                                })
                                .inner
                                .on_hover_text(format!(
                                    "{} · {}",
                                    node_label(node, language),
                                    domain_title
                                ))
                                .clicked()
                            {
                                controller.select_node(node);
                            }
                        })
                    });
                }
            });
    }
    if !idle_edges.is_empty() {
        egui::CollapsingHeader::new(format!(
            "{} ({})",
            tr(language, "Idle connections"),
            idle_edges.len()
        ))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("corpus_idle_edges")
                .max_height(150.0)
                .show(ui, |ui| {
                    for edge in idle_edges {
                        ui.push_id(
                            ("idle", &edge.source_id, &edge.target_id, &edge.kind),
                            |ui| {
                                ui.label(format!(
                                    "{} → {}",
                                    endpoint_label(snapshot, &edge.source_id, language),
                                    endpoint_label(snapshot, &edge.target_id, language)
                                ));
                                let source =
                                    snapshot.nodes.iter().find(|node| node.id == edge.source_id);
                                let target =
                                    snapshot.nodes.iter().find(|node| node.id == edge.target_id);
                                if source.is_none()
                                    && components::secondary_button_enabled(
                                        ui,
                                        tr(language, "Create source term"),
                                        !controller.draft_dirty && !controller.pending,
                                    )
                                    .clicked()
                                {
                                    controller.create_missing_node(
                                        &edge.source_id,
                                        target.map(|node| node.domain_id.as_str()),
                                    );
                                }
                                if target.is_none()
                                    && components::secondary_button_enabled(
                                        ui,
                                        tr(language, "Create target term"),
                                        !controller.draft_dirty && !controller.pending,
                                    )
                                    .clicked()
                                {
                                    controller.create_missing_node(
                                        &edge.target_id,
                                        source.map(|node| node.domain_id.as_str()),
                                    );
                                }
                                edge_controls(controller, edge, snapshot, ui, language);
                            },
                        );
                    }
                });
        });
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack
        .to_lowercase()
        .contains(&needle.trim().to_lowercase())
}

fn node_label(node: &GraphNode, language: UiLanguage) -> &str {
    let preferred = preferred_language_index(language);
    [preferred, 1, 0]
        .into_iter()
        .filter_map(|index| node.values.get(index))
        .find(|value| !value.trim().is_empty())
        .or_else(|| node.values.iter().find(|value| !value.trim().is_empty()))
        .map_or(node.title.as_str(), String::as_str)
}

fn language_value(ui: &mut egui::Ui, code: &str, value: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label(RichText::new(code).monospace());
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(220.0)
                .char_limit(512),
        )
        .changed()
    })
    .inner
}

fn preferred_language_index(language: UiLanguage) -> usize {
    match language {
        UiLanguage::Chinese => 0,
        UiLanguage::English => 1,
        UiLanguage::Japanese => 5,
        UiLanguage::Korean => 7,
        UiLanguage::Russian => 6,
    }
}

fn endpoint_label<'a>(snapshot: &'a GraphSnapshot, id: &'a str, language: UiLanguage) -> &'a str {
    snapshot
        .nodes
        .iter()
        .find(|node| node.id == id)
        .map_or(id, |node| node_label(node, language))
}

fn domain_label<'a>(snapshot: &'a GraphSnapshot, id: &'a str) -> &'a str {
    snapshot
        .domains
        .iter()
        .find(|domain| domain.id == id)
        .map_or(id, |domain| domain.title.as_str())
}

fn matches_node(node: &GraphNode, query: &str) -> bool {
    query.trim().is_empty()
        || contains(&node.title, query)
        || contains(&node.id, query)
        || node.values.iter().any(|value| contains(value, query))
}

fn valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id.trim() == id
        && id.chars().count() <= 256
        && !id
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn resolve_edge_target(snapshot: &GraphSnapshot, query: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(node) = snapshot.nodes.iter().find(|node| node.id == query) {
        return Some(node.id.clone());
    }
    let query_lower = query.to_lowercase();
    let mut exact = snapshot.nodes.iter().filter(|node| {
        node.title.trim().to_lowercase() == query_lower
            || node
                .values
                .iter()
                .any(|value| value.trim().to_lowercase() == query_lower)
    });
    if let Some(node) = exact.next() {
        return exact.next().is_none().then(|| node.id.clone());
    }
    valid_node_id(query).then(|| query.to_owned())
}

fn matching_nodes<'a>(
    controller: &CorpusStudioController,
    snapshot: &'a GraphSnapshot,
) -> Vec<&'a GraphNode> {
    let mut nodes = snapshot
        .nodes
        .iter()
        .filter(|node| {
            controller
                .selected_domain
                .as_ref()
                .is_none_or(|id| &node.domain_id == id)
                && matches_node(node, &controller.node_search)
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|a, b| {
        (&a.domain_id, &a.subdomain, &a.title, &a.id).cmp(&(
            &b.domain_id,
            &b.subdomain,
            &b.title,
            &b.id,
        ))
    });
    nodes
}

fn visible_nodes<'a>(
    controller: &CorpusStudioController,
    snapshot: &'a GraphSnapshot,
) -> Vec<&'a GraphNode> {
    if controller.focus_connections
        && let Some(selected) = &controller.selected_node
    {
        let mut ids = HashSet::from([selected.as_str()]);
        for edge in &snapshot.edges {
            if &edge.source_id == selected {
                ids.insert(&edge.target_id);
            }
            if &edge.target_id == selected {
                ids.insert(&edge.source_id);
            }
        }
        let mut nodes = snapshot
            .nodes
            .iter()
            .filter(|node| ids.contains(node.id.as_str()))
            .collect::<Vec<_>>();
        nodes.sort_by_key(|node| (&node.id != selected, &node.domain_id, &node.title));
        nodes
    } else {
        matching_nodes(controller, snapshot)
    }
}

fn connected_nodes(snapshot: &GraphSnapshot) -> HashSet<&str> {
    let ids = snapshot
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    snapshot
        .edges
        .iter()
        .filter(|edge| {
            ids.contains(edge.source_id.as_str()) && ids.contains(edge.target_id.as_str())
        })
        .flat_map(|edge| [edge.source_id.as_str(), edge.target_id.as_str()])
        .collect()
}

fn node_size(node: &GraphNode, connected: &HashSet<&str>) -> Vec2 {
    if !node.promptable {
        Vec2::new(132.0, 40.0)
    } else if !connected.contains(node.id.as_str()) {
        Vec2::new(148.0, 54.0)
    } else {
        NODE_SIZE
    }
}

fn domain_color(id: &str) -> Color32 {
    let hash = id.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    const COLORS: [Color32; 6] = [
        Color32::from_rgb(67, 126, 160),
        Color32::from_rgb(139, 104, 171),
        Color32::from_rgb(66, 140, 112),
        Color32::from_rgb(176, 128, 63),
        Color32::from_rgb(173, 100, 120),
        Color32::from_rgb(69, 140, 149),
    ];
    COLORS[hash as usize % COLORS.len()]
}

fn sync_layout(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    nodes: &[&GraphNode],
    connected: &HashSet<&str>,
    force: bool,
) {
    let ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let edges = snapshot
        .edges
        .iter()
        .filter(|edge| {
            ids.contains(edge.source_id.as_str()) && ids.contains(edge.target_id.as_str())
        })
        .map(|edge| (edge.source_id.clone(), edge.target_id.clone()))
        .collect::<Vec<_>>();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    controller.view.hash(&mut hash);
    for node in nodes {
        (
            &node.id,
            &node.domain_id,
            node.promptable,
            connected.contains(node.id.as_str()),
        )
            .hash(&mut hash);
    }
    edges.hash(&mut hash);
    let signature = hash.finish();
    if !force && controller.layout_signature == Some(signature) {
        return;
    }
    let topology_changed = controller.layout_signature.is_some()
        && controller.layout_view == controller.view
        && controller.layout.positions.len() == nodes.len()
        && nodes
            .iter()
            .all(|node| controller.layout.positions.contains_key(&node.id));
    let mut known = if controller.view == CorpusView::Graph {
        snapshot
            .positions
            .iter()
            .map(|p| (p.id.clone(), [p.x, p.y]))
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    if let Some(cache) = controller.layout_cache.get(&controller.view) {
        known.extend(cache.iter().map(|(id, p)| (id.clone(), *p)));
    }
    let known_rects = nodes
        .iter()
        .filter_map(|node| {
            known.get(&node.id).map(|p| {
                Rect::from_min_size(Pos2::new(p[0], p[1]), node_size(node, connected)).shrink(1.0)
            })
        })
        .collect::<Vec<_>>();
    let restore = !force
        && !topology_changed
        && known_rects.len() == nodes.len()
        && !known_rects.iter().enumerate().any(|(index, rect)| {
            known_rects[index + 1..]
                .iter()
                .any(|other| rect.intersects(*other))
        });
    controller.layout.reset(
        nodes.iter().map(|node| ForceNode {
            id: node.id.clone(),
            size: node_size(node, connected),
            group: snapshot
                .domains
                .iter()
                .position(|domain| domain.id == node.domain_id)
                .unwrap_or(snapshot.domains.len()),
        }),
        edges.iter().cloned(),
    );
    if !force {
        for node in nodes {
            if let Some(p) = known.get(&node.id) {
                controller.layout.positions.insert(node.id.clone(), *p);
            }
        }
    }
    if controller
        .editor
        .wire_from
        .as_ref()
        .is_some_and(|id| !ids.contains(id.as_str()))
        || controller
            .editor
            .wire_from_input
            .as_ref()
            .is_some_and(|id| !ids.contains(id.as_str()))
    {
        controller.editor.cancel_wire();
    }
    controller.layout_view = controller.view;
    controller.layout_running = controller.view == CorpusView::Graph && !restore;
    if restore {
        controller.editor.canvas.fit_pending = true;
    } else if controller.view == CorpusView::Layered {
        // A discovery forest gives cyclic terminology graphs useful layers; all original edges remain visible.
        let mut seen = HashSet::new();
        let mut tree = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        for node in nodes.iter().filter(|node| !node.promptable) {
            seen.insert(node.id.clone());
            queue.push_back(node.id.clone());
        }
        for root in nodes {
            if queue.is_empty() && seen.insert(root.id.clone()) {
                queue.push_back(root.id.clone());
            }
            while let Some(source) = queue.pop_front() {
                for (_, target) in edges.iter().filter(|(from, _)| from == &source) {
                    if seen.insert(target.clone()) {
                        tree.push((source.clone(), target.clone()));
                        queue.push_back(target.clone());
                    }
                }
            }
        }
        controller.layout.positions = layered_layout(
            nodes.iter().map(|node| LayoutNode {
                id: node.id.clone(),
                size: node_size(node, connected),
            }),
            tree,
            LayeredLayoutOptions {
                horizontal_gap: 90.0,
                vertical_gap: 22.0,
                snap: None,
                ..Default::default()
            },
        )
        .into_iter()
        .map(|movement| (movement.node_id, movement.position))
        .collect();
    } else {
        for _ in 0..18 {
            controller.layout_running = controller.layout.step(1.0 / 60.0);
        }
    }
    controller.layout_signature = Some(signature);
    controller.editor.canvas.fit_pending = true;
}

fn render_workspace(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let workspace_top = ui.cursor().top();
    ui.horizontal_wrapped(|ui| {
        for (view, title) in [
            (CorpusView::Graph, "Graph"),
            (CorpusView::Layered, "Hierarchy"),
            (CorpusView::List, "List"),
        ] {
            if ui
                .selectable_label(controller.view == view, tr(language, title))
                .clicked()
                && controller.view != view
            {
                if controller.view == CorpusView::List && controller.selected_node.is_some() {
                    controller.focus_connections = true;
                    controller.graph_page = 0;
                }
                controller.view = view;
                controller.layout_signature = None;
                controller.editor.cancel_wire();
            }
        }
    });
    ui.add_space(4.0);
    if controller.view == CorpusView::List {
        render_list(
            controller,
            snapshot,
            ui,
            language,
            height - (ui.cursor().top() - workspace_top),
        );
        return;
    }
    let mut candidates = visible_nodes(controller, snapshot);
    let center = if controller.focus_connections {
        controller
            .selected_node
            .as_ref()
            .and_then(|id| candidates.iter().position(|node| &node.id == id))
            .map(|index| candidates.remove(index))
    } else {
        None
    };
    let page_size = MAX_VISIBLE_NODES - usize::from(center.is_some());
    let page_count = candidates.len().div_ceil(page_size).max(1);
    controller.graph_page = controller.graph_page.min(page_count - 1);
    let nodes = center
        .into_iter()
        .chain(
            candidates
                .into_iter()
                .skip(controller.graph_page * page_size)
                .take(page_size),
        )
        .collect::<Vec<_>>();
    let connected = connected_nodes(snapshot);
    sync_layout(controller, snapshot, &nodes, &connected, false);
    ui.horizontal_wrapped(|ui| {
        if components::secondary_button(ui, tr(language, "Auto layout")).clicked() {
            sync_layout(controller, snapshot, &nodes, &connected, true);
        }
        if components::secondary_button(ui, tr(language, "Fit graph")).clicked() {
            controller.layout_running = false;
            controller.fit_entire_graph = true;
            controller.editor.canvas.fit_pending = true;
        }
        let changes = controller.position_changes(snapshot);
        if components::secondary_button_enabled(
            ui,
            tr(language, "Save positions"),
            !changes.is_empty() && !controller.layout_running && !controller.pending,
        )
        .clicked()
        {
            controller.send(Command::SavePositions(changes));
        }
        ui.menu_button("⋯", |ui| {
            if ui.button(tr(language, "Restore saved positions")).clicked() {
                controller.layout_cache.insert(
                    controller.view,
                    snapshot
                        .positions
                        .iter()
                        .map(|p| (p.id.clone(), [p.x, p.y]))
                        .collect(),
                );
                controller.layout_signature = None;
                sync_layout(controller, snapshot, &nodes, &connected, false);
                ui.close();
            }
        });
        if controller.layout_running {
            ui.spinner();
        }
        if controller.selected_node.is_some()
            && ui
                .selectable_label(
                    controller.focus_connections,
                    tr(language, "Connections only"),
                )
                .clicked()
        {
            controller.focus_connections = !controller.focus_connections;
            controller.graph_page = 0;
        }
        if page_count > 1 {
            if ui
                .add_enabled(controller.graph_page > 0, egui::Button::new("‹"))
                .clicked()
            {
                controller.graph_page -= 1;
            }
            ui.small(format!("{} / {}", controller.graph_page + 1, page_count));
            if ui
                .add_enabled(
                    controller.graph_page + 1 < page_count,
                    egui::Button::new("›"),
                )
                .clicked()
            {
                controller.graph_page += 1;
            }
        }
    });
    let canvas_height = (height - (ui.cursor().top() - workspace_top) - 34.0).max(240.0);
    Frame::new()
        .fill(graph_style::CANVAS_FILL)
        .stroke(Stroke::new(1.0, graph_style::CANVAS_BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            let (canvas, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), canvas_height),
                Sense::click_and_drag(),
            );
            if controller.editor.wire_active()
                || response.dragged()
                || (response.hovered() && ui.input(|input| input.smooth_scroll_delta.y != 0.0))
            {
                controller.layout_running = false;
            }
            if controller.layout_running {
                controller.layout_running =
                    controller.layout.step(ui.input(|input| input.predicted_dt));
                controller.editor.canvas.fit_pending = true;
                if controller.layout_running {
                    ui.ctx().request_repaint();
                }
            }
            let old_size = controller.editor.canvas.canvas_size;
            controller.editor.canvas.pan += (canvas.size() - old_size) * 0.5;
            controller.editor.canvas.canvas_size = canvas.size();
            if old_size != canvas.size()
                && let Some(id) = &controller.selected_node
                && let Some(node) = nodes.iter().find(|node| &node.id == id)
                && let Some(p) = controller.layout.positions.get(id)
            {
                let center = Vec2::new(p[0], p[1]) + node_size(node, &connected) * 0.5;
                controller.editor.canvas.pan =
                    canvas.size() * 0.5 - center * controller.editor.canvas.zoom;
            }
            if controller.editor.canvas.fit_pending {
                let bounds = nodes
                    .iter()
                    .filter_map(|node| {
                        controller.layout.positions.get(&node.id).map(|p| {
                            Rect::from_min_size(Pos2::new(p[0], p[1]), node_size(node, &connected))
                        })
                    })
                    .reduce(|a, b| a.union(b));
                if let Some(bounds) = bounds {
                    controller
                        .editor
                        .canvas
                        .fit_to_bounds(bounds, canvas.size(), NODE_SIZE);
                    if !controller.fit_entire_graph {
                        controller.editor.canvas.zoom = controller.editor.canvas.zoom.max(0.55);
                    }
                    controller.editor.canvas.pan = canvas.size() * 0.5
                        - bounds.center().to_vec2() * controller.editor.canvas.zoom;
                }
                controller.editor.canvas.fit_pending = false;
                controller.fit_entire_graph = false;
            }
            let canvas_ui = graph_canvas::canvas_viewport(ui, canvas);
            controller
                .editor
                .handle_navigation(canvas, &response, &canvas_ui, true, true);
            if response.dragged_by(egui::PointerButton::Primary)
                && controller.editor.drag_node.is_none()
                && !controller.editor.wire_active()
                && !ui.input(|input| input.key_down(egui::Key::Space))
            {
                controller.editor.canvas.pan += ui.input(|input| input.pointer.delta());
            }
            let rects = nodes
                .iter()
                .filter_map(|node| {
                    controller.layout.positions.get(&node.id).map(|p| {
                        (
                            node.id.clone(),
                            controller.editor.canvas.graph_rect(
                                canvas,
                                controller.editor.display_position(&node.id, *p),
                                node_size(node, &connected),
                            ),
                        )
                    })
                })
                .collect::<HashMap<_, _>>();
            let pointer = canvas_ui
                .ctx()
                .pointer_hover_pos()
                .filter(|p| canvas.contains(*p));
            let hovered_node = pointer.and_then(|p| {
                rects
                    .iter()
                    .find(|(_, rect)| rect.expand(6.0).contains(p))
                    .map(|(id, _)| id.clone())
            });
            let focus = hovered_node.clone().or_else(|| {
                controller
                    .selected_node
                    .clone()
                    .filter(|id| rects.contains_key(id))
            });
            let domains = snapshot
                .domains
                .iter()
                .map(|domain| {
                    (
                        domain.id.as_str(),
                        controller.displayed_domain_enabled(domain),
                    )
                })
                .collect::<HashMap<_, _>>();
            let lookup = nodes
                .iter()
                .map(|node| (node.id.as_str(), *node))
                .collect::<HashMap<_, _>>();
            let active_node = |node: &GraphNode| {
                controller.displayed_node_enabled(node)
                    && domains.get(node.domain_id.as_str()) == Some(&true)
            };
            let all_edges = snapshot
                .edges
                .iter()
                .filter(|edge| {
                    rects.contains_key(&edge.source_id) && rects.contains_key(&edge.target_id)
                })
                .collect::<Vec<_>>();
            // The overview keeps a nearest incoming/outgoing connection for each term.
            let mut nearest = HashMap::<(&str, bool, GraphEdgeKind), (usize, f32)>::new();
            for (index, edge) in all_edges.iter().enumerate() {
                let distance = rects[&edge.source_id]
                    .center()
                    .distance_sq(rects[&edge.target_id].center());
                for key in [
                    (edge.source_id.as_str(), true, edge.kind),
                    (edge.target_id.as_str(), false, edge.kind),
                ] {
                    let entry = nearest.entry(key).or_insert((index, distance));
                    if distance < entry.1 {
                        *entry = (index, distance);
                    }
                }
            }
            let overview = nearest
                .values()
                .map(|(index, _)| *index)
                .collect::<HashSet<_>>();
            let edge_lines = all_edges
                .iter()
                .enumerate()
                .filter(|(index, edge)| {
                    overview.contains(index)
                        || focus
                            .as_ref()
                            .is_some_and(|id| id == &edge.source_id || id == &edge.target_id)
                        || controller.selected_edge.as_ref() == Some(&edge_key(edge))
                })
                .map(|(_, edge)| {
                    let mut points = graph_canvas::node_connection(
                        rects[&edge.source_id],
                        rects[&edge.target_id],
                    );
                    // Separate reciprocal and context connections while keeping endpoints on the node boundary.
                    let direction = points[3] - points[0];
                    let bend = Vec2::new(-direction.y, direction.x).normalized()
                        * if edge.kind == GraphEdgeKind::Context {
                            14.0
                        } else {
                            -7.0
                        };
                    points[1] += bend;
                    points[2] += bend;
                    (*edge, points)
                })
                .collect::<Vec<_>>();
            let hit_edge = if hovered_node.is_none() {
                pointer.and_then(|p| {
                    closest_link(
                        p,
                        edge_lines
                            .iter()
                            .map(|(edge, points)| (edge_key(edge), *points)),
                        6.0,
                    )
                })
            } else {
                None
            };
            let highlighted_edge = hit_edge
                .clone()
                .or_else(|| controller.selected_edge.clone());
            for (edge, points) in &edge_lines {
                let selected = highlighted_edge.as_ref() == Some(&edge_key(edge));
                let incident = focus
                    .as_ref()
                    .is_some_and(|id| id == &edge.source_id || id == &edge.target_id);
                let active = controller.displayed_edge_enabled(edge)
                    && active_node(lookup[edge.source_id.as_str()])
                    && active_node(lookup[edge.target_id.as_str()]);
                let alpha = if selected {
                    1.0
                } else if incident {
                    0.8
                } else if focus.is_some() {
                    0.10
                } else {
                    0.34
                };
                let colors = [&edge.source_id, &edge.target_id].map(|id| {
                    (if selected {
                        graph_style::LINK_SELECTED
                    } else if active {
                        domain_color(&lookup[id.as_str()].domain_id)
                    } else {
                        graph_style::LINK_INACTIVE
                    })
                    .gamma_multiply(alpha)
                });
                graph_canvas::paint_directed_wire(
                    &canvas_ui,
                    *points,
                    if selected {
                        2.2
                    } else if edge.kind == GraphEdgeKind::Context {
                        1.0
                    } else {
                        1.4
                    },
                    colors,
                    edge.kind == GraphEdgeKind::Context || !active,
                );
            }
            if response.clicked() && hovered_node.is_none() && !controller.draft_dirty {
                controller.selected_edge = hit_edge.clone();
                controller.selected_node = None;
                controller.node_draft = None;
                controller.focus_connections = false;
                controller.editor.clear_selection();
                if let Some(key) = &hit_edge {
                    controller.editor.select_link(key.clone(), false);
                }
                controller.editor.cancel_wire();
            }
            let zoom = controller.editor.canvas.zoom;
            for node in &nodes {
                let Some(rect) = rects.get(&node.id).copied() else {
                    continue;
                };
                let selected = controller.selected_node.as_deref() == Some(&node.id);
                let hovered = hovered_node.as_deref() == Some(&node.id);
                let highlighted = selected
                    || controller.editor.drag_node.as_deref() == Some(&node.id)
                    || hovered
                    || highlighted_edge
                        .as_ref()
                        .is_some_and(|edge| edge.source_id == node.id || edge.target_id == node.id);
                let active = controller.displayed_node_enabled(node)
                    && domains.get(node.domain_id.as_str()) == Some(&true);
                let isolated = !connected.contains(node.id.as_str());
                let accent = if active && !isolated {
                    domain_color(&node.domain_id)
                } else {
                    graph_style::NODE_MUTED
                };
                let fill = if active {
                    accent.gamma_multiply(if node.promptable { 0.13 } else { 0.06 })
                } else {
                    Color32::from_gray(239)
                };
                let rounding = CornerRadius::same(if node.promptable {
                    8
                } else {
                    (rect.height() * 0.5) as u8
                });
                canvas_ui.painter().rect_filled(rect, rounding, fill);
                let stroke = Stroke::new(
                    if highlighted { 2.0 } else { 1.0 },
                    if highlighted {
                        graph_style::LINK_SELECTED
                    } else {
                        accent.gamma_multiply(0.45)
                    },
                );
                if isolated && !highlighted {
                    let path = [
                        rect.left_top(),
                        rect.right_top(),
                        rect.right_bottom(),
                        rect.left_bottom(),
                        rect.left_top(),
                    ];
                    canvas_ui
                        .painter()
                        .extend(egui::Shape::dashed_line(&path, stroke, 4.0, 4.0));
                } else {
                    canvas_ui.painter().rect_stroke(
                        rect,
                        rounding,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                let text_rect = rect.shrink(9.0 * zoom);
                let painter = canvas_ui.painter().with_clip_rect(text_rect);
                let mut title_job = egui::text::LayoutJob::simple(
                    node_label(node, language).to_owned(),
                    egui::FontId::proportional((13.0 * zoom).max(11.0)),
                    graph_style::NODE_TEXT,
                    text_rect.width(),
                );
                title_job.wrap.max_rows = 1;
                let title = painter.layout_job(title_job);
                painter.galley(
                    if node.promptable {
                        text_rect.min
                    } else {
                        Pos2::new(text_rect.left(), rect.center().y - title.size().y * 0.5)
                    },
                    title,
                    graph_style::NODE_TEXT,
                );
                if node.promptable {
                    painter.text(
                        Pos2::new(text_rect.left(), text_rect.bottom()),
                        egui::Align2::LEFT_BOTTOM,
                        node.values
                            .get(if preferred_language_index(language) == 1 {
                                0
                            } else {
                                1
                            })
                            .filter(|value| {
                                !value.is_empty() && value.as_str() != node_label(node, language)
                            })
                            .map_or(node.subdomain.as_str(), String::as_str),
                        egui::FontId::proportional((10.0 * zoom).max(8.0)),
                        graph_style::NODE_MUTED,
                    );
                }
                let body = canvas_ui
                    .interact(
                        rect.shrink2(Vec2::new(9.0, 0.0)),
                        canvas_ui.make_persistent_id(("corpus_node", &node.id)),
                        if controller.draft_dirty {
                            Sense::hover()
                        } else {
                            Sense::click_and_drag()
                        },
                    )
                    .on_hover_text(format!(
                        "{}\n{} · {}",
                        node_label(node, language),
                        domain_label(snapshot, &node.domain_id),
                        tr(
                            language,
                            if node.promptable {
                                "Prompt term"
                            } else {
                                "Trigger term"
                            }
                        )
                    ));
                if body.clicked() {
                    controller.layout_running = false;
                    controller.select_node(node);
                }
                if !controller.pending && !controller.draft_dirty && body.drag_started() {
                    controller.layout_running = false;
                    controller.editor.begin_node_drag(
                        node.id.clone(),
                        controller
                            .layout
                            .positions
                            .iter()
                            .map(|(id, p)| (id.clone(), *p)),
                    );
                }
                if controller.editor.drag_node.as_deref() == Some(&node.id) {
                    if body.dragged() {
                        controller
                            .editor
                            .update_node_drag(body.total_drag_delta().unwrap_or_default());
                    }
                }
                if highlighted || controller.editor.wire_active() {
                    let out = rect.right_center();
                    let input = rect.left_center();
                    canvas_ui.painter().circle_filled(out, 6.0, accent);
                    canvas_ui.painter().text(
                        out,
                        egui::Align2::CENTER_CENTER,
                        "+",
                        egui::FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                    canvas_ui
                        .painter()
                        .circle_filled(input, 5.0, graph_style::CANVAS_FILL);
                    canvas_ui
                        .painter()
                        .circle_stroke(input, 5.0, Stroke::new(1.5, accent));
                    let output = canvas_ui
                        .interact(
                            Rect::from_center_size(out, Vec2::splat(18.0)),
                            canvas_ui.make_persistent_id(("corpus_out", &node.id)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_text(tr(language, "Add trigger connection"));
                    let input = canvas_ui.interact(
                        Rect::from_center_size(input, Vec2::splat(18.0)),
                        canvas_ui.make_persistent_id(("corpus_in", &node.id)),
                        Sense::click_and_drag(),
                    );
                    if !controller.pending && !controller.draft_dirty {
                        if output.drag_started()
                            || output.clicked()
                            || input.drag_started()
                            || input.clicked()
                        {
                            controller.layout_running = false;
                        }
                        let commit = controller
                            .editor
                            .interact_output_port(&output, node.id.clone())
                            .or_else(|| {
                                controller
                                    .editor
                                    .interact_input_port(&input, node.id.clone(), None)
                            });
                        if let Some(commit) = commit
                            && let Some(target_id) = commit.to
                            && commit.from != target_id
                        {
                            controller.send(Command::PutEdge(GraphEdge {
                                source_id: commit.from,
                                target_id,
                                kind: GraphEdgeKind::Trigger,
                                enabled: true,
                            }));
                        }
                    }
                }
            }
            if controller.editor.drag_node.is_some()
                && canvas_ui.input(|input| input.pointer.any_released())
            {
                let dragged = controller.editor.drag_node.clone();
                for movement in controller.editor.finish_node_drag(None) {
                    controller
                        .layout
                        .set_position(&movement.node_id, movement.position);
                }
                if let Some(node) =
                    dragged.and_then(|id| snapshot.nodes.iter().find(|node| node.id == id))
                {
                    controller.select_node(node);
                }
            }
            if !controller.pending
                && canvas_ui.input(|input| input.pointer.any_released())
                && let Some(pointer) = pointer
            {
                let target = nearest_port(
                    pointer,
                    rects
                        .iter()
                        .map(|(id, rect)| (id.clone(), rect.left_center())),
                    18.0,
                );
                if let Some(target) = target
                    && controller
                        .editor
                        .wire_from
                        .as_ref()
                        .is_some_and(|id| id != &target)
                    && let Some(commit) = controller.editor.finish_wire(Some(target.clone()))
                {
                    controller.send(Command::PutEdge(GraphEdge {
                        source_id: commit.from,
                        target_id: target,
                        kind: GraphEdgeKind::Trigger,
                        enabled: true,
                    }));
                }
                let source = nearest_port(
                    pointer,
                    rects
                        .iter()
                        .map(|(id, rect)| (id.clone(), rect.right_center())),
                    18.0,
                );
                if let Some(source) = source
                    && controller
                        .editor
                        .wire_from_input
                        .as_ref()
                        .is_some_and(|id| id != &source)
                    && let Some(commit) = controller.editor.finish_reverse_wire(source)
                    && let Some(target_id) = commit.to
                {
                    controller.send(Command::PutEdge(GraphEdge {
                        source_id: commit.from,
                        target_id,
                        kind: GraphEdgeKind::Trigger,
                        enabled: true,
                    }));
                }
            }
            if let Some(pointer) = pointer {
                let wire = controller
                    .editor
                    .wire_from
                    .as_ref()
                    .and_then(|id| rects.get(id))
                    .map(|rect| (rect.right_center(), pointer))
                    .or_else(|| {
                        controller
                            .editor
                            .wire_from_input
                            .as_ref()
                            .and_then(|id| rects.get(id))
                            .map(|rect| (pointer, rect.left_center()))
                    });
                if let Some((from, to)) = wire {
                    graph_canvas::paint_directed_wire(
                        &canvas_ui,
                        graph_canvas::bezier_points(from, to),
                        1.5,
                        [graph_style::LINK_SELECTED; 2],
                        true,
                    );
                }
            }
            if canvas_ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                controller.editor.cancel_wire();
            }
            if nodes.is_empty() {
                canvas_ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    tr(language, "No matching terms"),
                    egui::FontId::proportional(14.0),
                    graph_style::MUTED,
                );
            }
        });
    controller
        .layout_cache
        .entry(controller.view)
        .or_default()
        .extend(
            controller
                .layout
                .positions
                .iter()
                .map(|(id, p)| (id.clone(), *p)),
        );
    ui.horizontal_wrapped(|ui| {
        ui.small(format!("{} {}", nodes.len(), tr(language, "terms")));
        ui.separator();
        ui.small(format!(
            "▰ {}  ·  ◯ {}",
            tr(language, "Prompt term"),
            tr(language, "Trigger term")
        ));
        ui.small(format!(
            "→ {}  ·  ⇢ {}",
            tr(language, "Trigger"),
            tr(language, "Context condition")
        ));
    });
}

fn render_list(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let nodes = matching_nodes(controller, snapshot);
    ui.horizontal_wrapped(|ui| {
        let mut all = !nodes.is_empty()
            && nodes
                .iter()
                .all(|node| controller.row_selection.contains(&node.id));
        if ui.checkbox(&mut all, tr(language, "Select all")).changed() {
            for node in &nodes {
                if all {
                    controller.row_selection.insert(node.id.clone());
                } else {
                    controller.row_selection.remove(&node.id);
                }
            }
        }
        ui.small(format!(
            "{} / {}",
            controller.row_selection.len(),
            nodes.len()
        ));
        let can_edit =
            !controller.row_selection.is_empty() && !controller.pending && !controller.draft_dirty;
        for (enabled, label) in [(true, "Enable selected"), (false, "Disable selected")] {
            if components::secondary_button_enabled(ui, tr(language, label), can_edit).clicked() {
                controller.send(Command::SetNodeState(NodeState {
                    ids: controller.row_selection.iter().cloned().collect(),
                    enabled: Some(enabled),
                    domain_id: None,
                }));
            }
        }
        ui.add_enabled_ui(can_edit, |ui| {
            ui.menu_button(tr(language, "Move to domain"), |ui| {
                for domain in &snapshot.domains {
                    if ui.button(&domain.title).clicked() {
                        controller.send(Command::SetNodeState(NodeState {
                            ids: controller.row_selection.iter().cloned().collect(),
                            enabled: None,
                            domain_id: Some(domain.id.clone()),
                        }));
                        controller.row_selection.clear();
                        ui.close();
                    }
                }
            });
        });
    });
    let primary = preferred_language_index(language);
    let secondary = if primary == 1 { 0 } else { 1 };
    let connections =
        snapshot
            .edges
            .iter()
            .fold(HashMap::<&str, usize>::new(), |mut counts, edge| {
                *counts.entry(&edge.source_id).or_default() += 1;
                *counts.entry(&edge.target_id).or_default() += 1;
                counts
            });
    egui::ScrollArea::horizontal()
        .id_salt("corpus_list_horizontal")
        .show(ui, |ui| {
            ui.set_min_width(600.0);
            use egui_extras::{Column, TableBuilder};
            TableBuilder::new(ui)
                .id_salt("corpus_term_table")
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(24.0))
                .columns(Column::remainder().at_least(120.0), 2)
                .column(Column::initial(110.0).at_least(90.0))
                .column(Column::auto().at_least(60.0))
                .column(Column::auto().at_least(60.0))
                .max_scroll_height(height - 72.0)
                .header(28.0, |mut header| {
                    for label in [
                        "",
                        "Terms",
                        "Translations",
                        "Domain",
                        "Connections",
                        "Enabled",
                    ] {
                        header.col(|ui| {
                            ui.strong(tr(language, label));
                        });
                    }
                })
                .body(|body| {
                    body.rows(34.0, nodes.len(), |mut row| {
                        let node = nodes[row.index()];
                        row.col(|ui| {
                            let mut checked = controller.row_selection.contains(&node.id);
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    controller.row_selection.insert(node.id.clone());
                                } else {
                                    controller.row_selection.remove(&node.id);
                                }
                            }
                        });
                        row.col(|ui| {
                            if ui
                                .add_enabled(
                                    !controller.draft_dirty,
                                    egui::Button::selectable(
                                        controller.selected_node.as_deref() == Some(&node.id),
                                        node_label(node, language),
                                    ),
                                )
                                .clicked()
                            {
                                controller.select_node(node);
                            }
                        });
                        row.col(|ui| {
                            ui.label(node.values.get(secondary).map_or("", String::as_str));
                        });
                        row.col(|ui| {
                            ui.colored_label(
                                domain_color(&node.domain_id),
                                domain_label(snapshot, &node.domain_id),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                connections
                                    .get(node.id.as_str())
                                    .copied()
                                    .unwrap_or(0)
                                    .to_string(),
                            );
                        });
                        row.col(|ui| {
                            let mut enabled = controller.displayed_node_enabled(node);
                            if ui
                                .add_enabled_ui(!controller.pending, |ui| {
                                    components::pill_toggle(ui, &mut enabled)
                                })
                                .inner
                                .changed()
                            {
                                controller.set_node_enabled(node, enabled);
                            }
                        });
                    });
                });
        });
}

fn render_inspector(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let focused_edge = controller
        .selected_edge
        .as_ref()
        .and_then(|key| snapshot.edges.iter().find(|edge| edge_key(edge) == *key));
    let close = ui
        .horizontal(|ui| {
            components::section_heading(
                ui,
                tr(
                    language,
                    if focused_edge.is_some() {
                        "Connection"
                    } else if controller.new_node {
                        "New term"
                    } else {
                        "Term details"
                    },
                ),
            );
            ui.add_enabled(!controller.draft_dirty, egui::Button::new("×"))
                .on_hover_text(tr(language, "Close"))
                .clicked()
        })
        .inner;
    if close {
        controller.selected_node = None;
        controller.selected_edge = None;
        controller.node_draft = None;
        controller.focus_connections = false;
        controller.editor.clear_selection();
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("corpus_inspector")
        .max_height(height - 28.0)
        .show(ui, |ui| {
            if controller.pending {
                ui.disable();
            }
            if let Some(edge) = focused_edge {
                ui.label(format!("{} → {}", endpoint_label(snapshot, &edge.source_id, language), endpoint_label(snapshot, &edge.target_id, language)));
                ui.label(if edge.kind == GraphEdgeKind::Context {
                    tr(language, "Context condition")
                } else {
                    tr(language, "Trigger")
                });
                edge_controls(controller, edge, snapshot, ui, language);
                return;
            }
            let Some(mut draft) = controller.node_draft.clone() else {
                ui.label(tr(language, "Select a term or create a new one."));
                return;
            };
            let mut changed = false;
            ui.label(tr(language, "Domain"));
            let old_domain = draft.domain_id.clone();
            egui::ComboBox::from_id_salt("corpus_node_domain")
                .selected_text(
                    snapshot
                        .domains
                        .iter()
                        .find(|d| d.id == draft.domain_id)
                        .map_or(draft.domain_id.as_str(), |d| d.title.as_str()),
                )
                .show_ui(ui, |ui| {
                    for domain in &snapshot.domains {
                        ui.selectable_value(&mut draft.domain_id, domain.id.clone(), &domain.title);
                    }
                });
            changed |= draft.domain_id != old_domain;
            ui.label(RichText::new(tr(language, "Translations")).strong());
            draft.values.resize(LANGUAGE_CODES.len(), String::new());
            let primary = preferred_language_index(language);
            let secondary = if primary == 1 { 0 } else { 1 };
            for index in [primary, secondary] {
                changed |= language_value(ui, LANGUAGE_CODES[index], &mut draft.values[index]);
            }
            egui::CollapsingHeader::new(tr(language, "Other languages"))
                .id_salt(("corpus_other_languages", &draft.id))
                .show(ui, |ui| {
                    for (index, code) in LANGUAGE_CODES.iter().enumerate() {
                        if index == primary || index == secondary { continue; }
                        changed |= language_value(ui, code, &mut draft.values[index]);
                    }
                });
            ui.horizontal(|ui| {
                ui.label(tr(language, "Enabled"));
                let mut enabled = controller.displayed_node_enabled(&draft);
                if ui.add_enabled_ui(!controller.pending, |ui| {
                    components::pill_toggle(ui, &mut enabled)
                }).inner.changed() {
                    draft.enabled = enabled;
                    if let Some(original) = snapshot.nodes.iter().find(|node| node.id == draft.id) {
                        controller.send(Command::PutNode(GraphNode { enabled, ..original.clone() }));
                    } else {
                        changed = true;
                    }
                }
            });
            ui.label(tr(language, "Activation"));
            let old_activation = draft.activation.clone();
            egui::ComboBox::from_id_salt("corpus_activation")
                .selected_text(if draft.activation == CorpusActivation::Always {
                    tr(language, "Always active")
                } else {
                    tr(language, "Activated by trigger")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut draft.activation,
                        CorpusActivation::Always,
                        tr(language, "Always active"),
                    );
                    ui.selectable_value(
                        &mut draft.activation,
                        CorpusActivation::OnEvidence,
                        tr(language, "Activated by trigger"),
                    );
                });
            if draft.activation != old_activation {
                if let Some(original) = snapshot.nodes.iter().find(|node| node.id == draft.id) {
                    controller.send(Command::PutNode(GraphNode {
                        activation: draft.activation.clone(), ..original.clone()
                    }));
                } else {
                    changed = true;
                }
            }
            let domain_enabled = |id: &str| {
                snapshot.domains.iter().find(|domain| domain.id == id)
                    .is_some_and(|domain| controller.displayed_domain_enabled(domain))
            };
            let node_enabled = |node: &GraphNode| {
                controller.displayed_node_enabled(node) && domain_enabled(&node.domain_id)
            };
            let target_enabled = node_enabled(&draft);
            let has_incoming_trigger = target_enabled && snapshot.edges.iter().any(|edge| {
                edge.target_id == draft.id && edge.kind == GraphEdgeKind::Trigger
                    && controller.displayed_edge_enabled(edge)
                    && snapshot.nodes.iter().find(|node| node.id == edge.source_id)
                        .is_some_and(node_enabled)
            });
            if draft.activation == CorpusActivation::Always && has_incoming_trigger {
                ui.small(tr(language, "Incoming triggers do not gate this term"));
            } else if draft.activation != CorpusActivation::Always && target_enabled && !has_incoming_trigger {
                ui.small(tr(language, "This term is waiting for an enabled trigger connection."));
            }
            egui::CollapsingHeader::new(tr(language, "Advanced settings"))
                .id_salt(("corpus_advanced", &draft.id))
                .show(ui, |ui| {
                    ui.label(tr(language, "Source title / note"));
                    changed |= ui.add(egui::TextEdit::singleline(&mut draft.title).char_limit(256)).changed();
                    ui.label(tr(language, "Subdomain"));
                    changed |= ui.add(egui::TextEdit::singleline(&mut draft.subdomain).char_limit(256)).changed();
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Include in prompts"));
                        let mut promptable = draft.promptable;
                        if components::pill_toggle(ui, &mut promptable).changed() {
                            draft.promptable = promptable;
                            if let Some(original) = snapshot.nodes.iter().find(|node| node.id == draft.id) {
                                controller.send(Command::PutNode(GraphNode { promptable, ..original.clone() }));
                            } else {
                                changed = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr(language, "Priority"));
                        changed |= ui.add(egui::DragValue::new(&mut draft.priority)
                            .range(-100..=100)).changed();
                    });
                    ui.label(tr(language, "Term ID"));
                    if controller.new_node {
                        changed |= ui.add(egui::TextEdit::singleline(&mut draft.id).char_limit(256)).changed();
                        ui.small(tr(language, "Use a missing term ID to activate its waiting connections."));
                        if snapshot.nodes.iter().any(|node| node.id == draft.id) {
                            ui.colored_label(graph_style::ERROR_BORDER, tr(language, "Term ID already exists"));
                        }
                    } else {
                        ui.monospace(&draft.id);
                    }
                });
            if changed {
                controller.draft_dirty = true;
            }
            controller.node_draft = Some(draft.clone());
            let valid_id = valid_node_id(&draft.id)
                && (!controller.new_node || !snapshot.nodes.iter().any(|node| node.id == draft.id));
            let valid_values = draft.values.iter().all(|value| {
                !value.contains([',', '\r', '\n']) && value.chars().count() <= 512
            });
            let valid_details = valid_node_id(&draft.subdomain)
                && draft.title.chars().count() <= 256
                && !draft.title.contains(['\r', '\n']);
            if !valid_values {
                ui.colored_label(
                    graph_style::ERROR_BORDER,
                    tr(language, "Language values cannot contain commas or newlines, or exceed 512 characters."),
                );
            }
            if !valid_details {
                ui.colored_label(
                    graph_style::ERROR_BORDER,
                    tr(language, "Check title and subdomain length and characters."),
                );
            }
            let valid = valid_id
                && (controller.new_node || !draft.title.trim().is_empty())
                && valid_details
                && !draft.domain_id.is_empty()
                && valid_values
                && draft.values.iter().any(|value| !value.trim().is_empty());
            ui.horizontal(|ui| {
                if components::primary_button_enabled(ui, tr(language, "Save term"),
                    !controller.pending && valid && controller.draft_dirty).clicked()
                {
                    if draft.title.trim().is_empty() {
                        draft.title = node_label(&draft, language).trim().chars().take(256).collect();
                    }
                    controller.selected_node = Some(draft.id.clone());
                    controller.send(Command::SaveNode(draft.clone()));
                }
                if controller.draft_dirty
                    && components::secondary_button(ui, tr(language, "Discard")).clicked() {
                    controller.node_draft = snapshot
                        .nodes
                        .iter()
                        .find(|node| node.id == draft.id)
                        .cloned();
                    controller.new_node = false;
                    controller.draft_dirty = false;
                }
            });
            if !controller.new_node {
                if !controller.confirm_delete {
                    if components::danger_button_enabled(ui, tr(language, "Delete term"),
                        !controller.pending && !controller.draft_dirty).clicked()
                    {
                        controller.confirm_delete = true;
                    }
                } else {
                    ui.colored_label(
                        graph_style::ERROR_BORDER,
                        tr(language, "Delete this term? Connections will remain idle until a matching term is added."),
                    );
                    if components::danger_button_enabled(ui, tr(language, "Confirm delete"),
                        !controller.pending && !controller.draft_dirty).clicked()
                    {
                        controller.send(Command::DeleteNode(draft.id.clone()));
                        controller.confirm_delete = false;
                    }
                    if components::secondary_button(ui, tr(language, "Cancel")).clicked() {
                        controller.confirm_delete = false;
                    }
                }
                ui.separator();
                ui.label(RichText::new(tr(language, "Connections")).strong());
                ui.add(egui::TextEdit::singleline(&mut controller.edge_target)
                    .hint_text(tr(language, "Search term or enter ID")));
                if !controller.edge_target.is_empty() {
                    let suggestions = snapshot.nodes.iter()
                        .filter(|node| matches_node(node, &controller.edge_target))
                        .take(4).collect::<Vec<_>>();
                    for node in suggestions {
                        if ui.small_button(format!("{} · {}", node_label(node, language), domain_label(snapshot, &node.domain_id))).clicked() {
                            controller.edge_target = node.id.clone();
                        }
                    }
                }
                egui::ComboBox::from_id_salt("corpus_edge_direction")
                    .selected_text(if controller.edge_outgoing {
                        tr(language, "This term → other term")
                    } else {
                        tr(language, "Other term → this term")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut controller.edge_outgoing, true,
                            tr(language, "This term → other term"));
                        ui.selectable_value(&mut controller.edge_outgoing, false,
                            tr(language, "Other term → this term"));
                    });
                egui::ComboBox::from_id_salt("corpus_edge_kind")
                    .selected_text(if controller.edge_kind == GraphEdgeKind::Context {
                        tr(language, "Context condition")
                    } else {
                        tr(language, "Trigger")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut controller.edge_kind,
                            GraphEdgeKind::Trigger,
                            tr(language, "Trigger"),
                        );
                        ui.selectable_value(
                            &mut controller.edge_kind,
                            GraphEdgeKind::Context,
                            tr(language, "Context condition"),
                        );
                    });
                let resolved_target = resolve_edge_target(snapshot, &controller.edge_target)
                    .filter(|id| id != &draft.id);
                let missing_target = resolved_target.as_ref().is_some_and(|id| {
                    !snapshot.nodes.iter().any(|node| &node.id == id)
                });
                let add_label = if missing_target {
                    tr(language, "Add idle connection")
                } else {
                    tr(language, "Add connection")
                };
                if components::primary_button_enabled(ui, add_label,
                    !controller.pending && resolved_target.is_some()).clicked()
                {
                    let other = resolved_target.expect("enabled Add connection has a target");
                    controller.send(Command::PutEdge(GraphEdge {
                        source_id: if controller.edge_outgoing { draft.id.clone() } else { other.clone() },
                        target_id: if controller.edge_outgoing { other } else { draft.id.clone() },
                        kind: controller.edge_kind,
                        enabled: true,
                    }));
                    controller.edge_target.clear();
                }
                egui::ScrollArea::vertical().id_salt("corpus_term_edges").max_height(180.0)
                    .show(ui, |ui| {
                        for edge in snapshot.edges.iter()
                            .filter(|edge| edge.source_id == draft.id || edge.target_id == draft.id) {
                            let source_exists = snapshot.nodes.iter().any(|node| node.id == edge.source_id);
                            let target_exists = snapshot.nodes.iter().any(|node| node.id == edge.target_id);
                            ui.label(format!("{} → {} · {}",
                                endpoint_label(snapshot, &edge.source_id, language),
                                endpoint_label(snapshot, &edge.target_id, language),
                                if source_exists && target_exists {
                                    if edge.kind == GraphEdgeKind::Context { tr(language, "Context condition") }
                                    else { tr(language, "Trigger") }
                                } else { tr(language, "Waiting for missing term") }));
                            edge_controls(controller, edge, snapshot, ui, language);
                        }
                    });
            }
        });
}

fn edge_controls(
    controller: &mut CorpusStudioController,
    edge: &GraphEdge,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
) {
    ui.push_id((&edge.source_id, &edge.target_id, &edge.kind), |ui| {
        let missing = !snapshot.nodes.iter().any(|node| node.id == edge.source_id)
            || !snapshot.nodes.iter().any(|node| node.id == edge.target_id);
        if missing {
            ui.colored_label(
                graph_style::MUTED,
                tr(language, "Idle until both terms exist"),
            );
        }
        ui.horizontal(|ui| {
            ui.label(tr(language, "Enabled"));
            let mut enabled = controller.displayed_edge_enabled(edge);
            if ui
                .add_enabled_ui(!controller.pending, |ui| {
                    components::pill_toggle(ui, &mut enabled)
                })
                .inner
                .changed()
            {
                controller.send(Command::PutEdge(GraphEdge {
                    enabled,
                    ..edge.clone()
                }));
            }
            if components::secondary_button_enabled(ui, tr(language, "Remove"), !controller.pending)
                .clicked()
            {
                controller.send(Command::DeleteEdge(edge.clone()));
                controller.selected_edge = None;
            }
        });
    });
}
