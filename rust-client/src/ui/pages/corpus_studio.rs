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
        GraphEdgeKind, GraphNode, GraphNodeStatePatch as NodeState, GraphSnapshot, VRCX_DOMAIN_ID,
    },
};

use crate::{
    backend::{BackendManager, BackendStart},
    i18n::{UiLanguage, tr},
    ui::{
        components, graph_canvas,
        graph_editor::{
            ForceLayout, ForceNode, GraphEditorState, LayeredLayoutOptions, LayoutNode,
            layered_layout, nearest_port,
        },
        graph_style,
    },
};

const MAX_NODE_SIZE: Vec2 = Vec2::new(218.0, 34.0);
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

#[derive(Default)]
struct GraphScene {
    nodes: Vec<usize>,
    sizes: Vec<Vec2>,
    edges: Vec<(usize, usize, usize)>,
    connected: Vec<bool>,
    domains: Vec<Option<usize>>,
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
            let client = match runtime.block_on(CorpusClient::connect(CORPUS_URL)) {
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
    scene_key: Option<u64>,
    scene: Arc<GraphScene>,
    fit_layout: bool,
    focus_connections: bool,
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
            scene_key: None,
            scene: Arc::new(GraphScene::default()),
            fit_layout: true,
            focus_connections: false,
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
                self.scene_key = None;
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
            self.editor.canvas.fit_pending = true;
        }
    }

    fn select_domain(&mut self, domain: Option<&GraphDomain>) {
        if self.draft_dirty {
            return;
        }
        self.selected_domain = domain.map(|domain| domain.id.clone());
        self.focus_connections = false;
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
        if controller.node_draft.is_some() && ui.available_width() < 700.0 {
            egui::ScrollArea::vertical()
                .id_salt("corpus_narrow_page")
                .show(ui, |ui| {
                    render_workspace(controller, &snapshot, ui, language, height * 0.6);
                    render_inspector(controller, &snapshot, ui, language, 440.0);
                });
        } else {
            ui.horizontal_top(|ui| {
                let show_inspector = controller.node_draft.is_some();
                let width =
                    (ui.available_width() - if show_inspector { 294.0 } else { 0.0 }).max(280.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        render_workspace(controller, &snapshot, ui, language, height);
                    },
                );
                if show_inspector {
                    ui.allocate_ui_with_layout(
                        Vec2::new(284.0, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            render_inspector(controller, &snapshot, ui, language, height);
                        },
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
        .max_height((height - 180.0).clamp(160.0, 380.0))
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
                ui.close();
            }
            for domain in snapshot
                .domains
                .iter()
                .filter(|d| d.id == VRCX_DOMAIN_ID)
                .chain(snapshot.domains.iter().filter(|d| d.id != VRCX_DOMAIN_ID))
            {
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
                            ui.close();
                        }
                        if domain.id != VRCX_DOMAIN_ID
                            && controller.selected_domain.as_deref() == Some(&domain.id)
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

fn node_size(node: &GraphNode, language: UiLanguage) -> Vec2 {
    let text_width = node_label(node, language)
        .chars()
        .fold(0.0_f32, |width, ch| {
            width
                + match ch {
                    ' ' | 'i' | 'l' | 'I' | '.' | ',' | '!' | '\'' | ':' | ';' => 4.0,
                    'm' | 'w' | 'M' | 'W' | '@' => 10.0,
                    ch if ch.is_ascii() => 7.0,
                    _ => 13.0,
                }
        });
    Vec2::new(
        (text_width + if node.promptable { 28.0 } else { 40.0 }).clamp(72.0, MAX_NODE_SIZE.x),
        MAX_NODE_SIZE.y,
    )
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
    language: UiLanguage,
) {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (
        controller.view,
        &controller.selected_domain,
        &controller.node_search,
        controller.focus_connections,
        preferred_language_index(language),
        controller
            .selected_node
            .as_ref()
            .filter(|_| controller.focus_connections),
    )
        .hash(&mut hash);
    let key = hash.finish();
    if controller.scene_key == Some(key) {
        return;
    }
    let mut nodes = visible_nodes(controller, snapshot);
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let sizes = nodes
        .iter()
        .map(|node| node_size(node, language))
        .collect::<Vec<_>>();
    let indexes = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect::<HashMap<_, _>>();
    let all_ids = snapshot
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .collect::<HashSet<_>>();
    let mut connected = vec![false; nodes.len()];
    let edges = snapshot
        .edges
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if all_ids.contains(e.source_id.as_str()) && all_ids.contains(e.target_id.as_str()) {
                for id in [&e.source_id, &e.target_id] {
                    if let Some(&index) = indexes.get(id.as_str()) {
                        connected[index] = true;
                    }
                }
            }
            Some((
                *indexes.get(e.source_id.as_str())?,
                *indexes.get(e.target_id.as_str())?,
                i,
            ))
        })
        .collect::<Vec<_>>();
    let domains = nodes
        .iter()
        .map(|n| snapshot.domains.iter().position(|d| d.id == n.domain_id))
        .collect::<Vec<_>>();
    let mut topology = std::collections::hash_map::DefaultHasher::new();
    controller.view.hash(&mut topology);
    for (node, size) in nodes.iter().zip(&sizes) {
        (&node.id, &node.domain_id, size.x.to_bits()).hash(&mut topology);
    }
    edges.hash(&mut topology);
    let signature = topology.finish();
    if controller.layout_signature != Some(signature) {
        controller.editor.cancel_wire();
        let links = edges
            .iter()
            .map(|&(a, b, _)| (nodes[a].id.clone(), nodes[b].id.clone()))
            .collect::<Vec<_>>();
        controller.layout.reset(
            nodes.iter().enumerate().map(|(i, node)| ForceNode {
                id: node.id.clone(),
                size: sizes[i],
                group: domains[i].unwrap_or(snapshot.domains.len()),
            }),
            links.iter().cloned(),
        );
        controller.layout_running = controller.view == CorpusView::Graph;
        if controller.view == CorpusView::Layered {
            // A discovery forest gives cyclic terminology graphs useful layers.
            let mut seen = HashSet::new();
            let mut tree = Vec::new();
            let mut queue = std::collections::VecDeque::new();
            for node in nodes.iter().filter(|node| !node.promptable) {
                seen.insert(node.id.clone());
                queue.push_back(node.id.clone());
            }
            for root in &nodes {
                if queue.is_empty() && seen.insert(root.id.clone()) {
                    queue.push_back(root.id.clone());
                }
                while let Some(source) = queue.pop_front() {
                    for (_, target) in links.iter().filter(|(from, _)| from == &source) {
                        if seen.insert(target.clone()) {
                            tree.push((source.clone(), target.clone()));
                            queue.push_back(target.clone());
                        }
                    }
                }
            }
            controller.layout.positions = layered_layout(
                nodes.iter().enumerate().map(|(i, node)| LayoutNode {
                    id: node.id.clone(),
                    size: sizes[i],
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
            .map(|m| (m.node_id, m.position))
            .collect();
        } else {
            for _ in 0..3 {
                controller.layout_running = controller.layout.step(1.0 / 60.0);
            }
        }
        controller.layout_signature = Some(signature);
        controller.fit_layout = true;
        controller.editor.canvas.fit_pending = true;
    }
    let snapshot_indexes = snapshot
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect::<HashMap<_, _>>();
    controller.scene = Arc::new(GraphScene {
        nodes: nodes
            .iter()
            .map(|n| snapshot_indexes[n.id.as_str()])
            .collect(),
        sizes,
        edges,
        connected,
        domains,
    });
    controller.scene_key = Some(key);
}

fn render_workspace(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let top = ui.cursor().top();
    let mut zoom_step = 0.0;
    ui.horizontal_wrapped(|ui| {
        let domain = controller
            .selected_domain
            .as_ref()
            .map_or(tr(language, "All domains"), |id| domain_label(snapshot, id))
            .to_owned();
        ui.menu_button(format!("{domain} ▾"), |ui| {
            ui.set_min_width(280.0);
            render_browser(controller, snapshot, ui, language, 560.0);
        });
        if ui
            .add(
                egui::TextEdit::singleline(&mut controller.node_search)
                    .desired_width(180.0)
                    .hint_text(tr(language, "Search terms")),
            )
            .changed()
        {
            controller.focus_connections = false;
            controller.row_selection.clear();
        }
        if ui
            .add_enabled(
                !snapshot.domains.is_empty() && !controller.draft_dirty,
                egui::Button::new(tr(language, "New term")),
            )
            .on_hover_text(tr(language, "New term"))
            .clicked()
        {
            controller.create_node();
        }
        ui.separator();
        if controller.view != CorpusView::Graph && ui.button(tr(language, "Graph")).clicked() {
            controller.view = CorpusView::Graph;
        }
        if controller.view != CorpusView::List {
            if ui
                .button("−")
                .on_hover_text(tr(language, "Zoom out"))
                .clicked()
            {
                zoom_step = -180.0;
            }
            if ui
                .button("+")
                .on_hover_text(tr(language, "Zoom in"))
                .clicked()
            {
                zoom_step = 180.0;
            }
            if ui.button(tr(language, "Fit graph")).clicked() {
                controller.editor.canvas.fit_pending = true;
                controller.fit_layout = true;
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
            }
        }
        ui.menu_button("⋯", |ui| {
            for (view, label) in [
                (CorpusView::Graph, "Graph"),
                (CorpusView::Layered, "Hierarchy"),
                (CorpusView::List, "List"),
            ] {
                if ui
                    .selectable_label(controller.view == view, tr(language, label))
                    .clicked()
                {
                    controller.view = view;
                    controller.editor.cancel_wire();
                    ui.close();
                }
            }
        });
    });
    ui.add_space(6.0);
    if controller.view == CorpusView::List {
        render_list(
            controller,
            snapshot,
            ui,
            language,
            height - (ui.cursor().top() - top),
        );
        return;
    }
    sync_layout(controller, snapshot, language);
    let scene = controller.scene.clone();
    let nodes = scene
        .nodes
        .iter()
        .map(|&i| &snapshot.nodes[i])
        .collect::<Vec<_>>();
    let canvas_height = (height - (ui.cursor().top() - top) - 24.0).max(220.0);
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
            controller.editor.canvas.min_zoom = 0.03;
            controller.editor.canvas.max_zoom = 3.0;
            let navigating = response.dragged()
                || zoom_step != 0.0
                || ui.input(|i| {
                    i.pointer.hover_pos().is_some_and(|p| canvas.contains(p))
                        && i.smooth_scroll_delta != Vec2::ZERO
                });
            if navigating {
                controller.fit_layout = false;
            }
            if controller.layout_running
                && !controller.editor.wire_active()
                && !ui.input(|i| i.pointer.any_down())
            {
                let start = Instant::now();
                for _ in 0..6 {
                    controller.layout_running = controller.layout.step(1.0 / 60.0);
                    if !controller.layout_running || start.elapsed() >= Duration::from_millis(3) {
                        break;
                    }
                }
                controller.editor.canvas.fit_pending |= controller.fit_layout;
                if controller.layout_running {
                    ui.ctx().request_repaint();
                }
            }
            let old_size = controller.editor.canvas.canvas_size;
            controller.editor.canvas.pan += (canvas.size() - old_size) * 0.5;
            controller.editor.canvas.canvas_size = canvas.size();
            if old_size != canvas.size()
                && let Some(id) = &controller.selected_node
                && let Some(p) = controller.layout.positions.get(id)
                && let Some(index) = nodes.iter().position(|node| &node.id == id)
            {
                let zoom = controller.editor.canvas.zoom;
                let size = scene.sizes[index];
                let center =
                    controller.editor.canvas.pan + (Vec2::new(p[0], p[1]) + size * 0.5) * zoom;
                let padding = (size * zoom * 0.5 + Vec2::splat(16.0)).min(canvas.size() * 0.5);
                controller.editor.canvas.pan +=
                    center.clamp(padding, canvas.size() - padding) - center;
            }
            if controller.editor.canvas.fit_pending {
                if let Some(bounds) = nodes
                    .iter()
                    .zip(&scene.sizes)
                    .filter_map(|(node, size)| {
                        controller
                            .layout
                            .positions
                            .get(&node.id)
                            .map(|p| Rect::from_min_size(Pos2::new(p[0], p[1]), *size))
                    })
                    .reduce(|a, b| a.union(b))
                {
                    controller
                        .editor
                        .canvas
                        .fit_to_bounds(bounds, canvas.size(), MAX_NODE_SIZE);
                }
                controller.editor.canvas.fit_pending = false;
            }
            if zoom_step != 0.0 {
                controller
                    .editor
                    .canvas
                    .zoom_at_pointer(canvas, canvas.center(), zoom_step);
            }
            let canvas_ui = graph_canvas::canvas_viewport(ui, canvas);
            controller
                .editor
                .handle_navigation(canvas, &response, &canvas_ui, true, true);
            if response.dragged_by(egui::PointerButton::Primary)
                && !controller.editor.wire_active()
                && !ui.input(|i| i.key_down(egui::Key::Space))
            {
                controller.editor.canvas.pan += ui.input(|i| i.pointer.delta());
            }
            let zoom = controller.editor.canvas.zoom;
            let t = ((zoom - 0.68) / 0.27).clamp(0.0, 1.0);
            let expansion = t * t * (3.0 - 2.0 * t);
            let dot_size = (24.0 * zoom).clamp(4.4, 16.0);
            let rects = nodes
                .iter()
                .zip(&scene.sizes)
                .map(|(node, node_size)| {
                    let p = controller.layout.positions[&node.id];
                    let center = controller
                        .editor
                        .canvas
                        .graph_rect(canvas, p, *node_size)
                        .center();
                    let size =
                        Vec2::splat(dot_size) * (1.0 - expansion) + *node_size * zoom * expansion;
                    Rect::from_center_size(center, size)
                })
                .collect::<Vec<_>>();
            let pointer = canvas_ui
                .ctx()
                .pointer_hover_pos()
                .filter(|p| canvas.contains(*p));
            let hovered = pointer.and_then(|p| {
                rects
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.expand(4.0).contains(p))
                    .min_by(|(_, a), (_, b)| {
                        a.center()
                            .distance_sq(p)
                            .total_cmp(&b.center().distance_sq(p))
                    })
                    .map(|(i, _)| i)
            });
            let selected = controller
                .selected_node
                .as_ref()
                .and_then(|id| nodes.iter().position(|n| &n.id == id));
            let focus = hovered.or(selected);
            let active = nodes
                .iter()
                .enumerate()
                .map(|(i, node)| {
                    controller.displayed_node_enabled(node)
                        && scene.domains[i].is_some_and(|d| {
                            controller.displayed_domain_enabled(&snapshot.domains[d])
                        })
                })
                .collect::<Vec<_>>();
            let colors = nodes
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    if active[i] && scene.connected[i] {
                        domain_color(&n.domain_id)
                    } else {
                        graph_style::NODE_MUTED
                    }
                })
                .collect::<Vec<_>>();
            let mut hit_edge = None;
            let mut closest = 5.0_f32.powi(2);
            if hovered.is_none()
                && zoom >= 0.55
                && let Some(p) = pointer
            {
                for &(a, b, e) in &scene.edges {
                    let from = rects[a].center();
                    let to = rects[b].center();
                    if !Rect::from_two_pos(from, to).expand(5.0).contains(p) {
                        continue;
                    }
                    let delta = to - from;
                    let t = ((p - from).dot(delta) / delta.length_sq().max(1.0)).clamp(0.0, 1.0);
                    let distance = p.distance_sq(from + delta * t);
                    if distance < closest {
                        closest = distance;
                        hit_edge = Some(e);
                    }
                }
            }
            let highlighted_edge = hit_edge
                .map(|i| edge_key(&snapshot.edges[i]))
                .or_else(|| controller.selected_edge.clone());
            let mut mesh = egui::Mesh::default();
            let mut detail_edges = Vec::new();
            for &(a, b, e) in &scene.edges {
                let edge = &snapshot.edges[e];
                if !Rect::from_two_pos(rects[a].center(), rects[b].center())
                    .expand(16.0)
                    .intersects(canvas)
                {
                    continue;
                }
                let highlighted = highlighted_edge.as_ref().is_some_and(|key| {
                    key.source_id == edge.source_id
                        && key.target_id == edge.target_id
                        && key.kind == edge.kind
                });
                let incident = focus.is_some_and(|i| i == a || i == b);
                let enabled = controller.displayed_edge_enabled(edge) && active[a] && active[b];
                let alpha = if highlighted {
                    0.9
                } else if incident {
                    0.7
                } else if focus.is_some() {
                    0.025
                } else if expansion > 0.7 {
                    0.045
                } else {
                    0.10
                };
                let color = [a, b].map(|i| {
                    (if highlighted {
                        graph_style::LINK_SELECTED
                    } else if enabled {
                        colors[i]
                    } else {
                        graph_style::LINK_INACTIVE
                    })
                    .gamma_multiply(alpha)
                });
                if incident || highlighted {
                    detail_edges.push((a, b, e, color, highlighted, enabled));
                } else {
                    graph_canvas::mesh_line(
                        &mut mesh,
                        rects[a].center(),
                        rects[b].center(),
                        0.7,
                        color,
                    );
                }
            }
            canvas_ui.painter().add(mesh);
            for (a, b, e, color, highlighted, enabled) in detail_edges {
                let edge = &snapshot.edges[e];
                let mut points = graph_canvas::capsule_connection(rects[a], rects[b]);
                let delta = points[3] - points[0];
                let bend = Vec2::new(-delta.y, delta.x).normalized()
                    * if edge.kind == GraphEdgeKind::Context {
                        12.0
                    } else {
                        -6.0
                    };
                points[1] += bend;
                points[2] += bend;
                graph_canvas::paint_directed_wire(
                    &canvas_ui,
                    points,
                    if highlighted { 2.0 } else { 1.2 },
                    color,
                    edge.kind == GraphEdgeKind::Context || !enabled,
                );
            }
            if response.clicked() && hovered.is_none() && !controller.draft_dirty {
                controller.selected_edge = hit_edge.map(|i| edge_key(&snapshot.edges[i]));
                if hit_edge.is_none() {
                    controller.selected_node = None;
                    controller.node_draft = None;
                    controller.focus_connections = false;
                    controller.editor.clear_selection();
                }
                controller.editor.cancel_wire();
            }
            let mut dots = egui::Mesh::default();
            for (i, node) in nodes.iter().enumerate() {
                let rect = rects[i];
                if !rect.expand(10.0).intersects(canvas) {
                    continue;
                }
                let highlighted = selected == Some(i)
                    || hovered == Some(i)
                    || highlighted_edge
                        .as_ref()
                        .is_some_and(|e| e.source_id == node.id || e.target_id == node.id);
                let accent = if highlighted {
                    graph_style::LINK_SELECTED
                } else {
                    colors[i]
                };
                if expansion <= 0.01 {
                    if highlighted {
                        graph_canvas::mesh_circle(
                            &mut dots,
                            rect.center(),
                            rect.width() * 0.5 + 2.0,
                            accent.gamma_multiply(0.25),
                        );
                    }
                    graph_canvas::mesh_circle(
                        &mut dots,
                        rect.center(),
                        rect.width() * 0.5,
                        accent.gamma_multiply(if active[i] { 1.0 } else { 0.45 }),
                    );
                    if !node.promptable || !scene.connected[i] {
                        graph_canvas::mesh_circle(
                            &mut dots,
                            rect.center(),
                            (rect.width() * 0.5 - 1.3).max(0.8),
                            graph_style::CANVAS_FILL,
                        );
                    }
                } else {
                    let radius = CornerRadius::same((rect.height() * 0.5).min(255.0) as u8);
                    canvas_ui.painter().rect_filled(
                        rect,
                        radius,
                        graph_style::CANVAS_FILL
                            .lerp_to_gamma(accent, if active[i] { 0.10 } else { 0.04 }),
                    );
                    canvas_ui.painter().rect_stroke(
                        rect,
                        radius,
                        Stroke::new(
                            if highlighted { 2.0 } else { 1.0 },
                            accent.gamma_multiply(if highlighted { 1.0 } else { 0.5 }),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    if !node.promptable && expansion > 0.7 {
                        canvas_ui.painter().circle_stroke(
                            rect.left_center() + Vec2::new((10.0 * zoom).max(8.0), 0.0),
                            (3.0 * zoom).clamp(2.5, 6.0),
                            Stroke::new(1.3, accent.gamma_multiply(0.75)),
                        );
                    }
                    if expansion > 0.7 {
                        let left_padding = if node.promptable { 10.0 } else { 22.0 } * zoom;
                        let text_rect = Rect::from_min_max(
                            rect.min + Vec2::new(left_padding, 2.0),
                            rect.max - Vec2::new(10.0 * zoom, 2.0),
                        );
                        let mut job = egui::text::LayoutJob::simple(
                            node_label(node, language).to_owned(),
                            egui::FontId::proportional((13.0 * zoom).clamp(11.0, 32.0)),
                            graph_style::NODE_TEXT
                                .gamma_multiply(((expansion - 0.7) / 0.3).min(1.0)),
                            text_rect.width(),
                        );
                        job.wrap.max_rows = 1;
                        let painter = canvas_ui.painter().with_clip_rect(text_rect);
                        let galley = painter.layout_job(job);
                        painter.galley(
                            text_rect.center() - galley.size() * 0.5,
                            galley,
                            graph_style::NODE_TEXT,
                        );
                    }
                }
                let body = canvas_ui.interact(
                    rect.expand(3.0),
                    canvas_ui.make_persistent_id(("corpus_node", &node.id)),
                    if controller.draft_dirty {
                        Sense::hover()
                    } else {
                        Sense::click()
                    },
                );
                if hovered == Some(i) {
                    body.clone().on_hover_ui(|ui| {
                        ui.label(node_label(node, language));
                        ui.small(format!(
                            "{} · {}",
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
                    });
                }
                if body.clicked() {
                    controller.select_node(node);
                }
                if controller.editor.wire_active()
                    || (expansion > 0.8 && (hovered == Some(i) || selected == Some(i)))
                {
                    let out = rect.right_center();
                    let input = rect.left_center();
                    canvas_ui.painter().circle_filled(out, 5.0, accent);
                    canvas_ui
                        .painter()
                        .circle_filled(input, 4.0, graph_style::CANVAS_FILL);
                    canvas_ui
                        .painter()
                        .circle_stroke(input, 4.0, Stroke::new(1.5, accent));
                    let output = canvas_ui
                        .interact(
                            Rect::from_center_size(out, Vec2::splat(16.0)),
                            canvas_ui.make_persistent_id(("corpus_out", &node.id)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_text(tr(language, "Add trigger connection"));
                    let input = canvas_ui.interact(
                        Rect::from_center_size(input, Vec2::splat(16.0)),
                        canvas_ui.make_persistent_id(("corpus_in", &node.id)),
                        Sense::click_and_drag(),
                    );
                    if !controller.pending && !controller.draft_dirty {
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
            canvas_ui.painter().add(dots);
            if expansion <= 0.01
                && controller.selected_domain.is_none()
                && !controller.focus_connections
            {
                let mut bounds = vec![Rect::NOTHING; snapshot.domains.len()];
                for (i, domain) in scene.domains.iter().enumerate() {
                    if let Some(domain) = domain {
                        bounds[*domain] = bounds[*domain].union(rects[i]);
                    }
                }
                for (i, bounds) in bounds
                    .iter()
                    .enumerate()
                    .filter(|(_, bounds)| bounds.is_positive())
                {
                    let label = Rect::from_center_size(
                        bounds.center_top() - Vec2::new(0.0, 13.0),
                        Vec2::new(bounds.width().clamp(80.0, 180.0), 20.0),
                    );
                    if !canvas.intersects(label) {
                        continue;
                    }
                    let domain = &snapshot.domains[i];
                    let mut job = egui::text::LayoutJob::simple(
                        domain.title.clone(),
                        egui::FontId::proportional(11.0),
                        domain_color(&domain.id),
                        label.width(),
                    );
                    job.wrap.max_rows = 1;
                    let galley = canvas_ui.painter().layout_job(job);
                    canvas_ui.painter().galley(
                        label.center() - galley.size() * 0.5,
                        galley,
                        domain_color(&domain.id),
                    );
                    if canvas_ui
                        .interact(
                            label,
                            canvas_ui.make_persistent_id(("corpus_domain", &domain.id)),
                            if controller.draft_dirty {
                                Sense::hover()
                            } else {
                                Sense::click()
                            },
                        )
                        .on_hover_text(&domain.title)
                        .clicked()
                    {
                        controller.select_domain(Some(domain));
                    }
                }
            }
            if !controller.pending
                && controller.editor.wire_active()
                && canvas_ui.input(|i| i.pointer.any_released())
                && let Some(pointer) = pointer
            {
                let target = nearest_port(
                    pointer,
                    nodes
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.id.clone(), rects[i].left_center())),
                    16.0,
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
                    nodes
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.id.clone(), rects[i].right_center())),
                    16.0,
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
                let endpoint = |id: &str| nodes.iter().position(|n| n.id == id).map(|i| rects[i]);
                let wire = controller
                    .editor
                    .wire_from
                    .as_ref()
                    .and_then(|id| endpoint(id))
                    .map(|r| (r.right_center(), pointer))
                    .or_else(|| {
                        controller
                            .editor
                            .wire_from_input
                            .as_ref()
                            .and_then(|id| endpoint(id))
                            .map(|r| (pointer, r.left_center()))
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
            if canvas_ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                controller.editor.cancel_wire();
            }
            if nodes.is_empty() {
                canvas_ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    tr(
                        language,
                        if controller.selected_domain.as_deref() == Some(VRCX_DOMAIN_ID) {
                            "VRCX adds room and player terms while connected"
                        } else {
                            "No matching terms"
                        },
                    ),
                    egui::FontId::proportional(14.0),
                    graph_style::MUTED,
                );
            }
        });
    ui.horizontal(|ui| {
        ui.small(format!("{} {}", nodes.len(), tr(language, "terms")));
        if controller.layout_running {
            ui.spinner();
        }
        ui.small(format!(
            "→ {} · ⇢ {}",
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
    let close = ui
        .horizontal(|ui| {
            components::section_heading(
                ui,
                tr(
                    language,
                    if controller.new_node {
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
