//! Interactive view of the persistent XR Corpus vocabulary graph.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{self, Color32, CornerRadius, Frame, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

use crate::{
    backend::{BackendManager, BackendStart},
    i18n::{UiLanguage, tr},
    ui::{
        components, graph_canvas,
        graph_editor::{GraphEditorState, closest_link, nearest_port},
        graph_style,
    },
};

const LANGUAGE_CODES: [&str; 16] = [
    "zh", "en", "fr", "pt", "es", "ja", "ru", "ko", "th", "it", "de", "vi", "id", "pl", "cs", "nl",
];
const NODE_SIZE: Vec2 = Vec2::new(154.0, 64.0);
const MAX_VISIBLE_NODES: usize = 80;
const MAX_OVERVIEW_EDGES: usize = 90;
const CORPUS_URL: &str = "http://127.0.0.1:7766/v1/graph";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GraphSnapshot {
    domains: Vec<GraphDomain>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GraphDomain {
    id: String,
    title: String,
    enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GraphNode {
    id: String,
    domain_id: String,
    subdomain: String,
    title: String,
    enabled: bool,
    promptable: bool,
    activation: String,
    priority: i32,
    values: Vec<String>,
    x: f32,
    y: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct EdgeKey {
    source_id: String,
    target_id: String,
    kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GraphEdge {
    source_id: String,
    target_id: String,
    kind: String,
    enabled: bool,
}

impl GraphEdge {
    fn key(&self) -> EdgeKey {
        EdgeKey {
            source_id: self.source_id.clone(),
            target_id: self.target_id.clone(),
            kind: self.kind.clone(),
        }
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
}

enum PendingEnabled {
    Domain(String, bool),
    Node(String, bool),
    Edge(EdgeKey, bool),
}

impl Command {
    fn saves_node(&self) -> bool {
        matches!(self, Self::SaveNode(_))
    }
}

#[derive(Deserialize)]
struct ApiError {
    code: String,
    error: String,
}

fn graph_url(parts: &[&str]) -> reqwest::Url {
    let mut url = reqwest::Url::parse(CORPUS_URL).expect("constant corpus URL");
    url.path_segments_mut()
        .expect("path URL")
        .extend(parts.iter().copied());
    url
}

async fn execute(client: &reqwest::Client, command: &Command) -> Result<GraphSnapshot, String> {
    let request = match command {
        Command::Refresh => client.get(graph_url(&[])),
        Command::PutDomain(domain) => client.put(graph_url(&["domains", &domain.id])).json(domain),
        Command::DeleteDomain(id) => client.delete(graph_url(&["domains", id])),
        Command::PutNode(node) | Command::SaveNode(node) => {
            client.put(graph_url(&["nodes", &node.id])).json(node)
        }
        Command::DeleteNode(id) => client.delete(graph_url(&["nodes", id])),
        Command::PutEdge(edge) => client.put(graph_url(&["edges"])).json(edge),
        Command::DeleteEdge(edge) => client.delete(graph_url(&["edges"])).json(edge),
    };
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.status().is_success() {
        response
            .json()
            .await
            .map_err(|error| format!("Invalid corpus response: {error}"))
    } else {
        let status = response.status();
        match response.json::<ApiError>().await {
            Ok(error) => Err(format!("{}: {}", error.code, error.error)),
            Err(_) => Err(format!("XR Corpus returned HTTP {status}")),
        }
    }
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
            let client = match reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(8))
                .build()
            {
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
    edge_kind: String,
    confirm_delete: bool,
    pending: bool,
    pending_node_save: bool,
    pending_enabled: Option<PendingEnabled>,
    pending_position: Option<(String, [f32; 2])>,
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
            edge_kind: "trigger".into(),
            confirm_delete: false,
            pending: false,
            pending_node_save: false,
            pending_enabled: None,
            pending_position: None,
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
            Command::PutEdge(edge) => Some(PendingEnabled::Edge(edge.key(), edge.enabled)),
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
            self.pending_position = None;
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
        self.pending_position = None;
        match result {
            Ok(snapshot) => {
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
                    .is_some_and(|key| !snapshot.edges.iter().any(|edge| edge.key() == *key))
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
            draft.x = saved.x;
            draft.y = saved.y;
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
            _ => node.enabled,
        }
    }

    fn displayed_edge_enabled(&self, edge: &GraphEdge) -> bool {
        match &self.pending_enabled {
            Some(PendingEnabled::Edge(key, enabled)) if *key == edge.key() => *enabled,
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
        self.editor.canvas.fit_pending = true;
    }

    fn select_domain(&mut self, domain: Option<&GraphDomain>) {
        if self.draft_dirty {
            return;
        }
        self.selected_domain = domain.map(|domain| domain.id.clone());
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
        let position = self.editor.new_node_position(
            self.snapshot
                .iter()
                .flat_map(|snapshot| snapshot.nodes.iter())
                .map(|node| Rect::from_min_size(Pos2::new(node.x, node.y), NODE_SIZE)),
            NODE_SIZE,
            None,
            16.0,
        );
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
            activation: "always".into(),
            priority: 20,
            values: vec![String::new(); LANGUAGE_CODES.len()],
            x: position[0],
            y: position[1],
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
                    render_canvas(controller, &snapshot, ui, language, height.min(460.0));
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
                let canvas_width = (ui.available_width() - 294.0).max(280.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(canvas_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| render_canvas(controller, &snapshot, ui, language, height),
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(284.0, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| render_inspector(controller, &snapshot, ui, language, height),
                );
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
    ui.add(
        egui::TextEdit::singleline(&mut controller.node_search)
            .hint_text(tr(language, "Search terms")),
    );
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
                            if controller.selected_node.as_deref() == Some(&node.id)
                                && let Some(draft) = &mut controller.node_draft
                            {
                                draft.enabled = enabled;
                            }
                            controller.send(Command::PutNode(GraphNode {
                                enabled,
                                ..node.clone()
                            }));
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

fn visible_nodes<'a>(
    controller: &CorpusStudioController,
    snapshot: &'a GraphSnapshot,
) -> Vec<&'a GraphNode> {
    let mut visible = snapshot
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
    visible.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    if let Some(selected) = &controller.selected_node {
        let neighbors = snapshot
            .edges
            .iter()
            .filter_map(|edge| {
                if &edge.source_id == selected {
                    Some(edge.target_id.as_str())
                } else if &edge.target_id == selected {
                    Some(edge.source_id.as_str())
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        visible.retain(|node| &node.id == selected || neighbors.contains(node.id.as_str()));
        for neighbor in snapshot
            .nodes
            .iter()
            .filter(|node| neighbors.contains(node.id.as_str()))
        {
            if !visible.iter().any(|existing| existing.id == neighbor.id) {
                visible.push(neighbor);
            }
        }
        if let Some(node) = snapshot.nodes.iter().find(|node| &node.id == selected)
            && !visible.iter().any(|existing| existing.id == node.id)
        {
            visible.insert(0, node);
        }
        visible.sort_by_key(|node| &node.id != selected);
    }
    if controller.selected_node.is_none() {
        visible.truncate(MAX_VISIBLE_NODES);
    }
    visible
}

fn node_positions(nodes: &[&GraphNode]) -> HashMap<String, [f32; 2]> {
    nodes
        .iter()
        .map(|node| (node.id.clone(), [node.x, node.y]))
        .collect()
}

fn domain_color(id: &str) -> Color32 {
    let hash = id.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    const COLORS: [Color32; 6] = [
        Color32::from_rgb(215, 231, 242),
        Color32::from_rgb(230, 222, 243),
        Color32::from_rgb(218, 238, 225),
        Color32::from_rgb(244, 230, 209),
        Color32::from_rgb(238, 218, 223),
        Color32::from_rgb(221, 235, 235),
    ];
    COLORS[(hash as usize) % COLORS.len()]
}

fn render_canvas(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let nodes = visible_nodes(controller, snapshot);
    ui.horizontal(|ui| {
        ui.label(format!("{} {}", nodes.len(), tr(language, "terms")));
        if components::secondary_button(ui, tr(language, "Fit graph")).clicked() {
            controller.editor.canvas.fit_pending = true;
        }
        if controller.editor.wire_active() {
            ui.label(tr(language, "Select a target node"));
            if ui.button(tr(language, "Cancel")).clicked() {
                controller.editor.cancel_wire();
            }
        }
    });
    let mut positions = node_positions(&nodes);
    if let Some((id, position)) = &controller.pending_position {
        if let Some(visible_position) = positions.get_mut(id) {
            *visible_position = *position;
        }
    }
    Frame::new()
        .fill(graph_style::CANVAS_FILL)
        .stroke(Stroke::new(1.0, graph_style::CANVAS_BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            let (canvas, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), (height - 34.0).max(260.0)),
                Sense::click_and_drag(),
            );
            controller.editor.canvas.canvas_size = canvas.size();
            if controller.editor.canvas.fit_pending {
                if !positions.is_empty() {
                    let left = positions
                        .values()
                        .map(|p| p[0])
                        .fold(f32::INFINITY, f32::min);
                    let top = positions
                        .values()
                        .map(|p| p[1])
                        .fold(f32::INFINITY, f32::min);
                    let right = positions
                        .values()
                        .map(|p| p[0] + NODE_SIZE.x)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let bottom = positions
                        .values()
                        .map(|p| p[1] + NODE_SIZE.y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    controller.editor.canvas.fit_to_bounds(
                        Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom)),
                        canvas.size(),
                        NODE_SIZE,
                    );
                }
                controller.editor.canvas.fit_pending = false;
            }
            let canvas_ui = graph_canvas::canvas_viewport(ui, canvas);
            controller
                .editor
                .handle_navigation(canvas, &response, &canvas_ui, true, true);
            graph_canvas::paint_grid(
                &canvas_ui,
                canvas,
                &controller.editor.canvas,
                graph_style::GRID,
            );
            let rects = nodes
                .iter()
                .filter_map(|node| {
                    let p = positions.get(&node.id)?;
                    Some((
                        node.id.clone(),
                        controller.editor.canvas.graph_rect(
                            canvas,
                            controller.editor.display_position(&node.id, *p),
                            NODE_SIZE,
                        ),
                    ))
                })
                .collect::<HashMap<_, _>>();
            let domain_enabled = snapshot
                .domains
                .iter()
                .map(|domain| {
                    (
                        domain.id.as_str(),
                        controller.displayed_domain_enabled(domain),
                    )
                })
                .collect::<HashMap<_, _>>();
            let node_lookup = snapshot
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node))
                .collect::<HashMap<_, _>>();
            let focused_node = controller.selected_node.as_deref();
            let focused_edge = controller.selected_edge.as_ref();
            let candidates = snapshot
                .edges
                .iter()
                .filter(|edge| {
                    let source_visible = rects.contains_key(&edge.source_id);
                    let target_visible = rects.contains_key(&edge.target_id);
                    (source_visible && target_visible)
                        || (source_visible && !node_lookup.contains_key(edge.target_id.as_str()))
                        || (target_visible && !node_lookup.contains_key(edge.source_id.as_str()))
                })
                .filter(|edge| {
                    if let Some(id) = focused_node {
                        edge.source_id == id || edge.target_id == id
                    } else if let Some(key) = focused_edge {
                        edge.source_id == key.source_id
                            && edge.target_id == key.target_id
                            && edge.kind == key.kind
                    } else {
                        true
                    }
                })
                .collect::<Vec<_>>();
            let overview = focused_node.is_none() && focused_edge.is_none();
            let graph_edges = if overview && candidates.len() > MAX_OVERVIEW_EDGES {
                let trigger_count = candidates
                    .iter()
                    .filter(|edge| edge.kind == "trigger")
                    .count();
                let context_count = candidates.len() - trigger_count;
                let context_limit = context_count
                    .min(MAX_OVERVIEW_EDGES - trigger_count.min(MAX_OVERVIEW_EDGES / 2));
                let trigger_limit = trigger_count.min(MAX_OVERVIEW_EDGES - context_limit);
                [("trigger", trigger_limit), ("context", context_limit)]
                    .into_iter()
                    .flat_map(|(kind, limit)| {
                        let pool = candidates
                            .iter()
                            .copied()
                            .filter(|edge| edge.kind == kind)
                            .collect::<Vec<_>>();
                        let stride = pool.len().div_ceil(limit.max(1)).max(1);
                        pool.into_iter().step_by(stride)
                    })
                    .collect::<Vec<_>>()
            } else {
                candidates
            };
            let pair_kinds = graph_edges
                .iter()
                .filter(|edge| {
                    rects.contains_key(&edge.source_id) && rects.contains_key(&edge.target_id)
                })
                .fold(HashMap::<(&str, &str), u8>::new(), |mut pairs, edge| {
                    let flag = if edge.kind == "context" { 2 } else { 1 };
                    *pairs
                        .entry((edge.source_id.as_str(), edge.target_id.as_str()))
                        .or_default() |= flag;
                    pairs
                });
            let edge_lines = graph_edges
                .iter()
                .filter_map(|edge| {
                    let (from, to) = (rects.get(&edge.source_id)?, rects.get(&edge.target_id)?);
                    let offset = if pair_kinds
                        .get(&(edge.source_id.as_str(), edge.target_id.as_str()))
                        == Some(&3)
                    {
                        if edge.kind == "context" { 6.0 } else { -6.0 }
                    } else {
                        0.0
                    };
                    Some((
                        edge,
                        graph_canvas::bezier_points(
                            Pos2::new(from.right(), from.center().y + offset),
                            Pos2::new(to.left(), to.center().y + offset),
                        ),
                    ))
                })
                .collect::<Vec<_>>();
            for (edge, points) in &edge_lines {
                let active = controller.displayed_edge_enabled(edge)
                    && node_lookup
                        .get(edge.source_id.as_str())
                        .is_some_and(|node| {
                            controller.displayed_node_enabled(node)
                                && domain_enabled.get(node.domain_id.as_str()) == Some(&true)
                        })
                    && node_lookup
                        .get(edge.target_id.as_str())
                        .is_some_and(|node| {
                            controller.displayed_node_enabled(node)
                                && domain_enabled.get(node.domain_id.as_str()) == Some(&true)
                        });
                let selected = controller.selected_edge.as_ref() == Some(&edge.key());
                let color = if selected {
                    graph_style::LINK_SELECTED
                } else if edge.kind == "context" {
                    if active {
                        Color32::from_rgb(112, 81, 148)
                    } else {
                        Color32::from_rgb(180, 163, 193)
                    }
                } else if active {
                    graph_style::LINK
                } else {
                    graph_style::LINK_INACTIVE
                };
                let stroke = Stroke::new(if selected { 2.5 } else { 1.6 }, color);
                if active && edge.kind == "trigger" {
                    graph_canvas::paint_wire(&canvas_ui, *points, stroke);
                } else {
                    graph_canvas::paint_dashed_wire(&canvas_ui, *points, stroke);
                }
                let tip = points[3] - Vec2::new(7.0, 0.0);
                canvas_ui.painter().add(egui::Shape::convex_polygon(
                    vec![tip, tip - Vec2::new(8.0, 4.0), tip - Vec2::new(8.0, -4.0)],
                    color,
                    Stroke::NONE,
                ));
            }
            for edge in &graph_edges {
                let dangling = match (rects.get(&edge.source_id), rects.get(&edge.target_id)) {
                    (Some(source), None) if !node_lookup.contains_key(edge.target_id.as_str()) => {
                        let from = Pos2::new(source.right(), source.center().y);
                        Some((from, from + Vec2::new(70.0, -24.0)))
                    }
                    (None, Some(target)) if !node_lookup.contains_key(edge.source_id.as_str()) => {
                        let to = Pos2::new(target.left(), target.center().y);
                        Some((to - Vec2::new(70.0, 24.0), to))
                    }
                    _ => None,
                };
                if let Some((from, to)) = dangling {
                    graph_canvas::paint_dashed_wire(
                        &canvas_ui,
                        graph_canvas::bezier_points(from, to),
                        Stroke::new(1.4, graph_style::LINK_INACTIVE),
                    );
                }
            }
            let pointer = response.interact_pointer_pos();
            let pointer_over_node =
                pointer.is_some_and(|p| rects.values().any(|rect| rect.expand(10.0).contains(p)));
            let hit_edge = pointer.and_then(|p| {
                closest_link(
                    p,
                    edge_lines
                        .iter()
                        .map(|(edge, points)| (edge.key(), *points)),
                    9.0,
                )
            });
            if response.clicked() && !pointer_over_node && !controller.draft_dirty {
                if let Some(key) = hit_edge.clone() {
                    controller.selected_edge = Some(key.clone());
                    controller.selected_node = None;
                    controller.node_draft = None;
                    controller.editor.select_link(key, false);
                } else if controller.selected_node.is_some() || controller.selected_edge.is_some() {
                    controller.selected_node = None;
                    controller.selected_edge = None;
                    controller.node_draft = None;
                    controller.editor.clear_selection();
                    controller.editor.canvas.fit_pending = true;
                }
            }
            for node in &nodes {
                let Some(rect) = rects.get(&node.id).copied() else {
                    continue;
                };
                if !canvas.intersects(rect) {
                    continue;
                }
                let selected = controller.selected_node.as_deref() == Some(&node.id);
                let active = controller.displayed_node_enabled(node)
                    && domain_enabled.get(node.domain_id.as_str()) == Some(&true);
                let fill = if active {
                    domain_color(&node.domain_id)
                } else {
                    Color32::from_gray(226)
                };
                canvas_ui
                    .painter()
                    .rect_filled(rect, CornerRadius::same(5), fill);
                canvas_ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(5),
                    Stroke::new(
                        if selected { 2.2 } else { 1.0 },
                        if selected {
                            graph_style::LINK_SELECTED
                        } else {
                            graph_style::NODE_BORDER
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                let text_painter = canvas_ui.painter().with_clip_rect(rect.shrink(8.0));
                text_painter.text(
                    rect.min + Vec2::new(9.0, 11.0),
                    egui::Align2::LEFT_TOP,
                    node_label(node, language),
                    egui::FontId::proportional(13.0),
                    graph_style::NODE_TEXT,
                );
                text_painter.text(
                    rect.min + Vec2::new(9.0, 40.0),
                    egui::Align2::LEFT_TOP,
                    domain_label(snapshot, &node.domain_id),
                    egui::FontId::proportional(11.0),
                    graph_style::NODE_MUTED,
                );
                let body = canvas_ui.interact(
                    rect.shrink2(Vec2::new(10.0, 0.0)),
                    canvas_ui.make_persistent_id(("corpus_node", &node.id)),
                    if controller.draft_dirty {
                        Sense::hover()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                if body.clicked() {
                    controller.select_node(node);
                }
                if !controller.pending && !controller.draft_dirty && body.drag_started() {
                    controller.editor.begin_node_drag(
                        node.id.clone(),
                        positions
                            .iter()
                            .map(|(id, position)| (id.clone(), *position)),
                    );
                }
                if controller.editor.drag_node.as_deref() == Some(&node.id) {
                    if body.dragged() {
                        controller.editor.update_node_drag(body.drag_delta());
                    }
                    if body.drag_stopped() {
                        if let Some(movement) = controller
                            .editor
                            .finish_node_drag(Some(8.0))
                            .into_iter()
                            .find(|movement| movement.node_id == node.id)
                        {
                            let mut changed = (**node).clone();
                            changed.x = movement.position[0];
                            changed.y = movement.position[1];
                            if controller.selected_node.as_deref() == Some(&node.id) {
                                if let Some(draft) = &mut controller.node_draft {
                                    draft.x = changed.x;
                                    draft.y = changed.y;
                                }
                            }
                            controller.pending_position =
                                Some((node.id.clone(), movement.position));
                            controller.send(Command::PutNode(changed));
                        }
                    }
                }
                let out = Pos2::new(rect.right(), rect.center().y);
                let input = Pos2::new(rect.left(), rect.center().y);
                canvas_ui.painter().add(egui::Shape::convex_polygon(
                    vec![
                        out + Vec2::new(6.0, 0.0),
                        out + Vec2::new(-4.0, -6.0),
                        out + Vec2::new(-4.0, 6.0),
                    ],
                    Color32::from_rgb(62, 108, 145),
                    Stroke::NONE,
                ));
                canvas_ui.painter().circle_filled(input, 5.0, fill);
                canvas_ui.painter().circle_stroke(
                    input,
                    5.0,
                    Stroke::new(2.0, Color32::from_rgb(106, 131, 113)),
                );
                let out_response = canvas_ui.interact(
                    Rect::from_center_size(out, Vec2::splat(16.0)),
                    canvas_ui.make_persistent_id(("corpus_out", &node.id)),
                    Sense::click_and_drag(),
                );
                let in_response = canvas_ui.interact(
                    Rect::from_center_size(input, Vec2::splat(16.0)),
                    canvas_ui.make_persistent_id(("corpus_in", &node.id)),
                    Sense::click_and_drag(),
                );
                if !controller.pending {
                    let commit = controller
                        .editor
                        .interact_output_port(&out_response, node.id.clone())
                        .or_else(|| {
                            controller.editor.interact_input_port(
                                &in_response,
                                node.id.clone(),
                                None,
                            )
                        });
                    if let Some(commit) = commit
                        && let Some(target_id) = commit.to
                        && commit.from != target_id
                    {
                        controller.send(Command::PutEdge(GraphEdge {
                            source_id: commit.from,
                            target_id,
                            kind: "trigger".into(),
                            enabled: true,
                        }));
                    }
                }
            }
            if !controller.pending
                && controller.editor.wire_from.is_some()
                && canvas_ui.input(|input| input.pointer.any_released())
                && let Some(pointer) = canvas_ui.ctx().pointer_latest_pos()
                && let Some(target) = nearest_port(
                    pointer,
                    rects
                        .iter()
                        .map(|(id, rect)| (id.clone(), Pos2::new(rect.left(), rect.center().y))),
                    12.0,
                )
                && controller.editor.wire_from.as_deref() != Some(target.as_str())
                && let Some(commit) = controller.editor.finish_wire(Some(target.clone()))
            {
                controller.send(Command::PutEdge(GraphEdge {
                    source_id: commit.from,
                    target_id: target,
                    kind: "trigger".into(),
                    enabled: true,
                }));
            }
            if !controller.pending
                && controller.editor.wire_from_input.is_some()
                && canvas_ui.input(|input| input.pointer.any_released())
                && let Some(pointer) = canvas_ui.ctx().pointer_latest_pos()
                && let Some(source) = nearest_port(
                    pointer,
                    rects
                        .iter()
                        .map(|(id, rect)| (id.clone(), Pos2::new(rect.right(), rect.center().y))),
                    12.0,
                )
                && controller.editor.wire_from_input.as_deref() != Some(source.as_str())
                && let Some(commit) = controller.editor.finish_reverse_wire(source)
                && let Some(target_id) = commit.to
            {
                controller.send(Command::PutEdge(GraphEdge {
                    source_id: commit.from,
                    target_id,
                    kind: "trigger".into(),
                    enabled: true,
                }));
            }
            if let Some(from) = &controller.editor.wire_from
                && let Some(rect) = rects.get(from)
                && let Some(pointer) = canvas_ui.ctx().pointer_hover_pos()
            {
                graph_canvas::paint_dashed_wire(
                    &canvas_ui,
                    graph_canvas::bezier_points(Pos2::new(rect.right(), rect.center().y), pointer),
                    Stroke::new(1.5, graph_style::LINK_SELECTED),
                );
            }
            if let Some(to) = &controller.editor.wire_from_input
                && let Some(rect) = rects.get(to)
                && let Some(pointer) = canvas_ui.ctx().pointer_hover_pos()
            {
                graph_canvas::paint_dashed_wire(
                    &canvas_ui,
                    graph_canvas::bezier_points(pointer, Pos2::new(rect.left(), rect.center().y)),
                    Stroke::new(1.5, graph_style::LINK_SELECTED),
                );
            }
            controller.editor.handle_canvas_selection(
                &response,
                &canvas_ui,
                true,
                pointer_over_node,
                hit_edge.is_some(),
                rects.into_iter(),
            );
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
        .and_then(|key| snapshot.edges.iter().find(|edge| edge.key() == *key));
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
    egui::ScrollArea::vertical()
        .id_salt("corpus_inspector")
        .max_height(height - 28.0)
        .show(ui, |ui| {
            if controller.pending {
                ui.disable();
            }
            if let Some(edge) = focused_edge {
                ui.label(format!("{} → {}", endpoint_label(snapshot, &edge.source_id, language), endpoint_label(snapshot, &edge.target_id, language)));
                ui.label(if edge.kind == "context" {
                    tr(language, "Context")
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
                .selected_text(if draft.activation == "always" {
                    tr(language, "Always active")
                } else {
                    tr(language, "Activated by trigger")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut draft.activation,
                        "always".into(),
                        tr(language, "Always active"),
                    );
                    ui.selectable_value(
                        &mut draft.activation,
                        "on-evidence".into(),
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
                edge.target_id == draft.id && edge.kind == "trigger"
                    && controller.displayed_edge_enabled(edge)
                    && snapshot.nodes.iter().find(|node| node.id == edge.source_id)
                        .is_some_and(node_enabled)
            });
            if draft.activation == "always" && has_incoming_trigger {
                ui.small(tr(language, "Incoming triggers do not gate this term"));
            } else if draft.activation != "always" && target_enabled && !has_incoming_trigger {
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
                    .selected_text(if controller.edge_kind == "context" {
                        tr(language, "Context")
                    } else {
                        tr(language, "Trigger")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut controller.edge_kind,
                            "trigger".into(),
                            tr(language, "Trigger"),
                        );
                        ui.selectable_value(
                            &mut controller.edge_kind,
                            "context".into(),
                            tr(language, "Context"),
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
                        kind: controller.edge_kind.clone(),
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
                                    if edge.kind == "context" { tr(language, "Context") }
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
