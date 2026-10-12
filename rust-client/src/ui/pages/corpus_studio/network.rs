//! Circular domain hubs and a bounded vocabulary preview. Membership supplies the wires.
use super::*;

#[derive(Clone, Copy)]
enum Node {
    Overview,
    Domain(usize),
    Term(usize),
}

impl Node {
    fn size(self) -> Vec2 {
        match self {
            Self::Term(_) => Vec2::new(112.0, 68.0),
            _ => Vec2::new(152.0, 116.0),
        }
    }
}

#[derive(Default)]
pub(super) struct Network {
    editor: GraphEditorState<usize, ()>,
    layout: ForceLayout<usize>,
    nodes: Vec<Node>,
    edges: Vec<(usize, usize)>,
    query: Option<(Option<String>, String, usize)>,
    total: usize,
    running: bool,
    follow_layout: bool,
}

impl Network {
    pub fn invalidate(&mut self) {
        self.query = None;
    }

    fn sync(
        &mut self,
        controller: &mut CorpusStudioController,
        snapshot: &GraphSnapshot,
        limit: usize,
    ) {
        let scope = controller.selected_domain.as_deref();
        let query = (
            scope.map(str::to_owned),
            controller.node_search.trim().to_lowercase(),
            limit,
        );
        if self.query.as_ref() == Some(&query) {
            return;
        }
        let rows = (scope.is_some() || !query.1.is_empty())
            .then(|| controller.term_index.rows(snapshot, scope, &query.1));
        let matching_groups = (!query.1.is_empty()).then(|| {
            rows.as_ref()
                .into_iter()
                .flat_map(|rows| rows.iter())
                .map(|&i| snapshot.nodes[i].domain_id.as_str())
                .collect::<HashSet<_>>()
                .into_iter()
                .filter_map(|id| layout_group(snapshot, scope, id))
                .collect::<HashSet<_>>()
        });
        let current = snapshot
            .domains
            .iter()
            .position(|domain| Some(domain.id.as_str()) == scope);
        self.nodes = vec![current.map_or(Node::Overview, Node::Domain)];
        self.nodes.extend(
            snapshot
                .domains
                .iter()
                .enumerate()
                .filter(|(i, domain)| {
                    domain.parent_id.as_deref() == scope
                        && matching_groups
                            .as_ref()
                            .is_none_or(|groups| groups.contains(i))
                })
                .map(|(i, _)| Node::Domain(i)),
        );
        let hubs: HashMap<_, _> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, node)| match node {
                Node::Domain(d) => Some((snapshot.domains[*d].id.as_str(), i)),
                _ => None,
            })
            .collect();
        self.edges = (1..self.nodes.len()).map(|i| (0, i)).collect();
        let mut groups: Vec<_> = (0..self.nodes.len()).collect();
        self.total = 0;
        if let Some(rows) = rows {
            self.total = rows.len();
            // Resolve each domain once, then distribute the preview across its groups.
            let membership: HashMap<_, _> = snapshot
                .domains
                .iter()
                .map(|domain| {
                    let hub = layout_group(snapshot, scope, &domain.id)
                        .and_then(|i| hubs.get(snapshot.domains[i].id.as_str()).copied())
                        .unwrap_or(0);
                    (domain.id.as_str(), hub)
                })
                .collect();
            let mut buckets = vec![Vec::new(); self.nodes.len()];
            for &i in rows.iter() {
                let hub = membership
                    .get(snapshot.nodes[i].domain_id.as_str())
                    .copied()
                    .unwrap_or(0);
                buckets[hub].push(i);
            }
            let mut shown = 0;
            for row in 0..limit {
                for (hub, bucket) in buckets.iter().enumerate() {
                    if let Some(&term) = bucket.get(row) {
                        self.edges.push((hub, self.nodes.len()));
                        self.nodes.push(Node::Term(term));
                        groups.push(hub);
                        shown += 1;
                    }
                    if shown == limit {
                        break;
                    }
                }
                if shown == limit || shown == self.total {
                    break;
                }
            }
        }
        self.layout.reset(
            self.nodes.iter().enumerate().map(|(id, &node)| ForceNode {
                id,
                size: node.size(),
                group: groups[id],
            }),
            self.edges.iter().copied(),
        );
        self.running = true;
        self.follow_layout = true;
        self.editor.canvas.fit_pending = true;
        self.query = Some(query);
    }
}

pub(super) fn render(
    controller: &mut CorpusStudioController,
    snapshot: &GraphSnapshot,
    ui: &mut egui::Ui,
    language: UiLanguage,
    height: f32,
) {
    let top = ui.cursor().top();
    ui.weak(tr(language, "Click a domain to explore its terms. Terms join the network automatically through their domain."));
    let mut network = std::mem::take(&mut controller.domain_network);
    network.sync(
        controller,
        snapshot,
        if ui.ctx().content_rect().width() < 600.0 {
            16
        } else {
            64
        },
    );
    let shown = network
        .nodes
        .iter()
        .filter(|node| matches!(node, Node::Term(_)))
        .count();
    ui.horizontal_wrapped(|ui| {
        if ui
            .small_button("−")
            .on_hover_text(tr(language, "Zoom out"))
            .clicked()
        {
            network.editor.canvas.zoom_from_center(-180.0);
            network.follow_layout = false;
        }
        if ui
            .small_button("+")
            .on_hover_text(tr(language, "Zoom in"))
            .clicked()
        {
            network.editor.canvas.zoom_from_center(180.0);
            network.follow_layout = false;
        }
        if ui.small_button(tr(language, "Fit graph")).clicked() {
            network.editor.canvas.fit_pending = true;
            network.follow_layout = true;
        }
        if controller.selected_domain.is_some() || !controller.node_search.trim().is_empty() {
            ui.small(format!(
                "{shown} / {} {}",
                network.total,
                tr(language, "terms")
            ));
            if ui.small_button(tr(language, "Browse terms")).clicked() {
                controller.view = CorpusView::List;
            }
        } else {
            ui.small(format!(
                "{} {}",
                network.nodes.len().saturating_sub(1),
                tr(language, "Domains")
            ));
        }
    });
    if shown < network.total {
        ui.weak(tr(
            language,
            "Showing a preview. Search or open a domain to find any term.",
        ));
    } else if network.total == 0 && !controller.node_search.trim().is_empty() {
        ui.weak(tr(language, "No matching terms"));
    }
    let (canvas, response) = ui.allocate_exact_size(
        Vec2::new(
            ui.available_width(),
            (height - (ui.cursor().top() - top)).max(120.0),
        ),
        Sense::click_and_drag(),
    );
    let mut viewport = graph_canvas::canvas_viewport(ui, canvas);
    viewport
        .painter()
        .rect_filled(canvas, 8, graph_style::CANVAS_FILL);
    network.editor.canvas.min_zoom = 0.08;
    network.editor.canvas.max_zoom = 3.0;
    network.editor.canvas.resize_viewport(canvas.size());
    if network.running && !ui.input(|i| i.pointer.any_down()) {
        let started = Instant::now();
        for _ in 0..6 {
            network.running = network.layout.step(1.0 / 60.0);
            if !network.running || started.elapsed() >= Duration::from_millis(2) {
                break;
            }
        }
        network.editor.canvas.fit_pending |= network.follow_layout;
        if network.running {
            ui.ctx().request_repaint();
        }
    }
    // Keep the same cluster layout readable in portrait without rotating labels.
    let graph_rects: Vec<_> = network
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let p = network.layout.positions[&i];
            let mut center = Pos2::new(p[0], p[1]) + node.size() * 0.5;
            if canvas.width() < canvas.height() {
                center = Pos2::new(center.y, center.x);
            }
            Rect::from_center_size(center, node.size())
        })
        .collect();
    if network.editor.canvas.fit_pending {
        if let Some(bounds) = graph_rects.iter().copied().reduce(|a, b| a.union(b)) {
            network
                .editor
                .canvas
                .fit_to_bounds(bounds, canvas.size(), Node::Overview.size());
        }
        network.editor.canvas.fit_pending = false;
    }
    let touch = network
        .editor
        .handle_navigation(canvas, &response, &viewport, true, true);
    if touch {
        viewport.disable();
    }
    if response.dragged()
        || touch
        || ui.input(|i| {
            i.pointer.hover_pos().is_some_and(|p| canvas.contains(p))
                && i.smooth_scroll_delta != Vec2::ZERO
        })
    {
        network.follow_layout = false;
    }
    if response.dragged_by(egui::PointerButton::Primary)
        && !touch
        && !ui.input(|i| i.key_down(egui::Key::Space) || i.any_touches())
    {
        network.editor.canvas.pan += ui.input(|i| i.pointer.delta());
    }
    let zoom = network.editor.canvas.zoom;
    let rects: Vec<_> = graph_rects
        .iter()
        .map(|rect| {
            network
                .editor
                .canvas
                .graph_rect(canvas, rect.min.into(), rect.size())
        })
        .collect();
    let centers: Vec<_> = rects
        .iter()
        .map(|rect| rect.center() - Vec2::new(0.0, rect.height() * 0.15))
        .collect();
    let radii: Vec<_> = network
        .nodes
        .iter()
        .map(|node| match node {
            Node::Term(_) => (12.0 * zoom).clamp(4.0, 24.0),
            _ => (32.0 * zoom).clamp(10.0, 64.0),
        })
        .collect();
    let hovered = viewport
        .ctx()
        .pointer_hover_pos()
        .filter(|p| canvas.contains(*p))
        .and_then(|p| {
            rects
                .iter()
                .enumerate()
                .filter(|(_, rect)| rect.expand(4.0).contains(p))
                .min_by(|(a, _), (b, _)| {
                    centers[*a]
                        .distance_sq(p)
                        .total_cmp(&centers[*b].distance_sq(p))
                })
                .map(|(i, _)| i)
        });
    let selected = network.nodes.iter().position(|node| {
        matches!(node, Node::Term(i)
        if controller.selected_node.as_deref() == Some(snapshot.nodes[*i].id.as_str()))
    });
    let colors: Vec<_> = network
        .nodes
        .iter()
        .map(|node| match node {
            Node::Overview => graph_style::LINK_SELECTED,
            Node::Domain(i) => domain_color(&snapshot.domains[*i].id),
            Node::Term(i) => domain_color(&snapshot.nodes[*i].domain_id),
        })
        .collect();
    let mut wires = egui::Mesh::default();
    for &(a, b) in &network.edges {
        let direction = (centers[b] - centers[a]).normalized();
        let focused = hovered.or(selected).is_some_and(|i| i == a || i == b);
        graph_canvas::mesh_line(
            &mut wires,
            centers[a] + direction * radii[a],
            centers[b] - direction * radii[b],
            if focused { 1.8 } else { 1.0 },
            [colors[a], colors[b]]
                .map(|color| color.gamma_multiply(if focused { 0.75 } else { 0.25 })),
        );
    }
    viewport.painter().add(wires);
    for (i, &node) in network.nodes.iter().enumerate() {
        if !viewport.is_rect_visible(rects[i].expand(20.0)) {
            continue;
        }
        let (title, count, enabled) = match node {
            Node::Overview => (
                tr(language, "All domains").to_owned(),
                Some(snapshot.nodes.iter().filter(|node| node.promptable).count()),
                true,
            ),
            Node::Domain(d) => {
                let domain = &snapshot.domains[d];
                (
                    tr_dynamic(language, &domain.title).into_owned(),
                    Some(
                        controller
                            .term_index
                            .counts
                            .get(&domain.id)
                            .map_or(0, |count| count.0),
                    ),
                    domain_effective_enabled(controller, snapshot, &domain.id),
                )
            }
            Node::Term(t) => {
                let term = &snapshot.nodes[t];
                (
                    node_label(term, language).to_owned(),
                    None,
                    controller.displayed_node_enabled(term)
                        && domain_effective_enabled(controller, snapshot, &term.domain_id),
                )
            }
        };
        let highlighted = hovered == Some(i) || selected == Some(i);
        let accent = if enabled {
            colors[i]
        } else {
            graph_style::NODE_MUTED.gamma_multiply(0.45)
        };
        let painter = viewport.painter();
        if highlighted {
            painter.circle_filled(centers[i], radii[i] + 5.0, accent.gamma_multiply(0.14));
        }
        painter.circle_filled(
            centers[i],
            radii[i],
            if count.is_some() {
                graph_style::CANVAS_FILL.lerp_to_gamma(accent, 0.18)
            } else {
                accent
            },
        );
        painter.circle_stroke(
            centers[i],
            radii[i],
            Stroke::new(if highlighted { 2.0 } else { 1.2 }, accent),
        );
        if let Some(count) = count {
            painter.text(
                centers[i],
                egui::Align2::CENTER_CENTER,
                count,
                egui::FontId::proportional((radii[i] * 0.35).clamp(8.0, 22.0)),
                graph_style::NODE_TEXT,
            );
        }
        let mut job = egui::text::LayoutJob::simple(
            title.clone(),
            egui::FontId::proportional((13.0 * zoom).clamp(9.0, 20.0)),
            graph_style::NODE_TEXT,
            rects[i].width().max(64.0),
        );
        job.wrap.max_rows = 2;
        job.halign = egui::Align::Center;
        let galley = painter.layout_job(job);
        painter.galley(
            Pos2::new(centers[i].x, centers[i].y + radii[i] + 5.0),
            galley,
            graph_style::NODE_TEXT,
        );
        let hit = viewport.interact(
            rects[i].expand(4.0),
            viewport.make_persistent_id(("corpus_ball", i)),
            if controller.draft_dirty {
                Sense::hover()
            } else {
                Sense::click()
            },
        );
        if hovered == Some(i) {
            hit.clone().on_hover_ui(|ui| {
                ui.strong(&title);
                match node {
                    Node::Domain(d) => {
                        ui.label(domain_rules::summary(
                            snapshot,
                            &snapshot.domains[d].id,
                            language,
                        ));
                    }
                    Node::Term(t) => {
                        ui.label(domain_path_label(
                            snapshot,
                            &snapshot.nodes[t].domain_id,
                            language,
                        ));
                        ui.weak(tr(
                            language,
                            if snapshot.nodes[t].inherit_domain {
                                "Domain rules"
                            } else {
                                "Custom rules"
                            },
                        ));
                    }
                    Node::Overview => {
                        ui.label(format!(
                            "{} {}",
                            snapshot.nodes.iter().filter(|node| node.promptable).count(),
                            tr(language, "terms")
                        ));
                    }
                }
            });
        }
        if hit.clicked() && !touch && !controller.draft_dirty && !controller.pending {
            match node {
                Node::Domain(d) => controller.select_domain(Some(&snapshot.domains[d])),
                Node::Term(t) => controller.select_node(&snapshot.nodes[t]),
                Node::Overview => controller.select_domain(None),
            }
        }
    }
    controller.domain_network = network;
}
