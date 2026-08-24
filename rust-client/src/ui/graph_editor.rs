//! Domain-neutral graph editor interaction state and reducer helpers.
//!
//! Prompt Studio and Audio Studio project their own node/link types onto this
//! state machine. Domain renderers still own node bodies and validation; this
//! module exclusively owns selection, drag, wire, box-select, navigation, and
//! keyboard-operation semantics.

use super::graph_canvas::{self, GraphCanvasState};
use eframe::egui::{self, Pos2, Rect, Response, Vec2};
use std::{collections::HashMap, collections::HashSet, hash::Hash};

/// A domain-neutral node description used by the shared layered layout.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutNode<N> {
    pub id: N,
    pub size: Vec2,
}

/// Spacing and origin for [`layered_layout`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayeredLayoutOptions {
    pub origin: [f32; 2],
    pub horizontal_gap: f32,
    pub vertical_gap: f32,
    pub snap: Option<f32>,
}

impl Default for LayeredLayoutOptions {
    fn default() -> Self {
        Self {
            origin: [48.0, 48.0],
            horizontal_gap: 144.0,
            vertical_gap: 56.0,
            snap: Some(16.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GraphEditHistory<T> {
    undo_stack: Vec<T>,
    redo_stack: Vec<T>,
    max_depth: usize,
}

impl<T> Default for GraphEditHistory<T> {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_depth: 60,
        }
    }
}

impl<T: Clone + PartialEq> GraphEditHistory<T> {
    pub fn new(max_depth: usize) -> Self {
        Self {
            max_depth: max_depth.max(1),
            ..Self::default()
        }
    }

    pub fn push(&mut self, before: T) {
        if self.undo_stack.last() == Some(&before) {
            return;
        }
        self.undo_stack.push(before);
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, current: T) -> Option<T> {
        while let Some(previous) = self.undo_stack.pop() {
            if previous != current {
                self.redo_stack.push(current);
                return Some(previous);
            }
        }
        None
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        while let Some(next) = self.redo_stack.pop() {
            if next != current {
                self.undo_stack.push(current);
                return Some(next);
            }
        }
        None
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    #[cfg(test)]
    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    #[cfg(test)]
    pub fn redo_count(&self) -> usize {
        self.redo_stack.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NodeMove<N> {
    pub node_id: N,
    pub position: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WireCommit<O, I, L> {
    pub from: O,
    pub to: Option<I>,
    pub replaced: Option<L>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GraphShortcut {
    Undo,
    Redo,
    Delete,
    Cancel,
}

#[derive(Clone, Debug)]
pub(crate) struct GraphEditorState<N, L, O = N, I = O>
where
    N: Clone + Eq + Hash,
    L: Clone + Eq + Hash,
    O: Clone,
    I: Clone,
{
    pub canvas: GraphCanvasState,
    graph_key: Option<String>,
    pub selected_nodes: HashSet<N>,
    pub selected_links: HashSet<L>,
    pub drag_node: Option<N>,
    pub drag_origins: HashMap<N, [f32; 2]>,
    drag_delta: Vec2,
    pub wire_from: Option<O>,
    pub wire_from_input: Option<I>,
    pub rewire_link: Option<L>,
    pub box_select_start: Option<Pos2>,
    pub box_select_current: Option<Pos2>,
    pub add_node_center: Option<[f32; 2]>,
    secondary_action_suppressed: bool,
}

impl<N, L, O, I> Default for GraphEditorState<N, L, O, I>
where
    N: Clone + Eq + Hash,
    L: Clone + Eq + Hash,
    O: Clone,
    I: Clone,
{
    fn default() -> Self {
        Self {
            canvas: GraphCanvasState::default(),
            graph_key: None,
            selected_nodes: HashSet::new(),
            selected_links: HashSet::new(),
            drag_node: None,
            drag_origins: HashMap::new(),
            drag_delta: Vec2::ZERO,
            wire_from: None,
            wire_from_input: None,
            rewire_link: None,
            box_select_start: None,
            box_select_current: None,
            add_node_center: None,
            secondary_action_suppressed: false,
        }
    }
}

impl<N, L, O, I> GraphEditorState<N, L, O, I>
where
    N: Clone + Eq + Hash,
    L: Clone + Eq + Hash,
    O: Clone,
    I: Clone,
{
    pub fn reset_for_graph(&mut self, graph_key: impl Into<String>) -> bool {
        let graph_key = graph_key.into();
        if self.graph_key.as_deref() == Some(graph_key.as_str()) {
            return false;
        }
        self.graph_key = Some(graph_key);
        self.clear_interaction();
        self.clear_selection();
        self.canvas.fit_pending = true;
        true
    }

    pub fn replace_graph(&mut self, graph_key: impl Into<String>) {
        self.graph_key = None;
        self.reset_for_graph(graph_key);
    }

    pub fn clear_selection(&mut self) {
        self.selected_nodes.clear();
        self.selected_links.clear();
    }

    pub fn select_node(&mut self, node_id: N, extend: bool) {
        if extend {
            if !self.selected_nodes.insert(node_id.clone()) {
                self.selected_nodes.remove(&node_id);
            }
        } else {
            self.clear_selection();
            self.selected_nodes.insert(node_id);
        }
    }

    pub fn select_link(&mut self, link_id: L, extend: bool) {
        if extend {
            if !self.selected_links.insert(link_id.clone()) {
                self.selected_links.remove(&link_id);
            }
        } else {
            self.clear_selection();
            self.selected_links.insert(link_id);
        }
    }

    pub fn begin_node_drag(
        &mut self,
        node_id: N,
        positions: impl IntoIterator<Item = (N, [f32; 2])>,
    ) {
        if !self.selected_nodes.contains(&node_id) {
            self.clear_selection();
            self.selected_nodes.insert(node_id.clone());
        }
        self.drag_node = Some(node_id);
        self.drag_origins = positions
            .into_iter()
            .filter(|(id, _)| self.selected_nodes.contains(id))
            .collect();
        self.drag_delta = Vec2::ZERO;
    }

    pub fn update_node_drag(&mut self, pixel_delta: Vec2) {
        self.drag_delta = pixel_delta / self.canvas.zoom;
    }

    pub fn display_position(&self, node_id: &N, stored: [f32; 2]) -> [f32; 2] {
        self.drag_origins.get(node_id).map_or(stored, |origin| {
            [origin[0] + self.drag_delta.x, origin[1] + self.drag_delta.y]
        })
    }

    pub fn finish_node_drag(&mut self, snap: Option<f32>) -> Vec<NodeMove<N>> {
        let delta = self.drag_delta;
        let moves = self
            .drag_origins
            .drain()
            .map(|(node_id, origin)| {
                let mut position = [origin[0] + delta.x, origin[1] + delta.y];
                if let Some(grid) = snap.filter(|grid| *grid > 0.0) {
                    position[0] = (position[0] / grid).round() * grid;
                    position[1] = (position[1] / grid).round() * grid;
                }
                NodeMove { node_id, position }
            })
            .collect();
        self.drag_node = None;
        self.drag_delta = Vec2::ZERO;
        moves
    }

    pub fn start_wire_from_output(&mut self, from: O) {
        self.wire_from = Some(from);
        self.wire_from_input = None;
        self.rewire_link = None;
        self.selected_links.clear();
    }

    pub fn start_wire_from_input(&mut self, input: I, replaced: Option<L>) {
        self.wire_from = None;
        self.wire_from_input = Some(input);
        self.rewire_link = replaced;
        self.selected_links.clear();
    }

    pub fn start_rewire(&mut self, from: O, replaced: L) {
        self.start_wire_from_output(from);
        self.rewire_link = Some(replaced);
    }

    pub fn interact_output_port(
        &mut self,
        response: &Response,
        output: O,
    ) -> Option<WireCommit<O, I, L>> {
        if self.wire_from_input.is_some() && (response.clicked() || response.drag_stopped()) {
            return self.finish_reverse_wire(output);
        }
        if response.clicked() || response.drag_started() {
            self.start_wire_from_output(output);
        }
        None
    }

    pub fn interact_input_port(
        &mut self,
        response: &Response,
        input: I,
        connected: Option<(O, L)>,
    ) -> Option<WireCommit<O, I, L>> {
        if self.wire_from.is_some() && (response.clicked() || response.drag_stopped()) {
            return self.finish_wire(Some(input));
        }
        if response.drag_started() {
            if let Some((from, link)) = connected {
                self.start_rewire(from, link);
            } else {
                self.start_wire_from_input(input, None);
            }
        }
        None
    }

    pub fn finish_wire(&mut self, to: Option<I>) -> Option<WireCommit<O, I, L>> {
        let from = self.wire_from.take()?;
        self.wire_from_input = None;
        Some(WireCommit {
            from,
            to,
            replaced: self.rewire_link.take(),
        })
    }

    pub fn finish_reverse_wire(&mut self, from: O) -> Option<WireCommit<O, I, L>> {
        let to = self.wire_from_input.take()?;
        self.wire_from = None;
        Some(WireCommit {
            from,
            to: Some(to),
            replaced: self.rewire_link.take(),
        })
    }

    pub fn wire_active(&self) -> bool {
        self.wire_from.is_some() || self.wire_from_input.is_some()
    }

    pub fn cancel_wire(&mut self) {
        self.wire_from = None;
        self.wire_from_input = None;
        self.rewire_link = None;
        self.canvas.cancel_wire_navigation();
    }

    /// Cancels an in-progress connection on secondary click anywhere inside the canvas.
    ///
    /// The suppression bit lets domain renderers avoid interpreting that same click as a
    /// disconnect/context-menu action later in the frame.
    pub fn handle_secondary_wire_cancel(&mut self, canvas: Rect, ui: &egui::Ui) -> bool {
        self.secondary_action_suppressed = false;
        let secondary_clicked = ui.input(|input| input.pointer.secondary_clicked());
        let pointer = ui
            .ctx()
            .pointer_latest_pos()
            .or_else(|| ui.ctx().pointer_hover_pos());
        if self.wire_active()
            && secondary_clicked
            && pointer.is_some_and(|pointer| canvas.contains(pointer))
        {
            self.cancel_wire();
            self.secondary_action_suppressed = true;
            return true;
        }
        false
    }

    pub fn secondary_action_suppressed(&self) -> bool {
        self.secondary_action_suppressed
    }

    pub fn begin_box_selection(&mut self, pointer: Pos2, extend: bool) {
        if !extend {
            self.clear_selection();
        }
        self.box_select_start = Some(pointer);
        self.box_select_current = Some(pointer);
    }

    pub fn update_box_selection(&mut self, pointer: Option<Pos2>) {
        if self.box_select_start.is_some() {
            self.box_select_current = pointer;
        }
    }

    pub fn selection_rect(&self) -> Option<Rect> {
        Some(graph_canvas::rect_between(
            self.box_select_start?,
            self.box_select_current?,
        ))
    }

    pub fn finish_box_selection(&mut self) -> Option<Rect> {
        let rect = self.selection_rect();
        self.box_select_start = None;
        self.box_select_current = None;
        rect
    }

    pub fn select_nodes(&mut self, nodes: impl IntoIterator<Item = N>) {
        self.selected_nodes.extend(nodes);
    }

    pub fn take_selection(&mut self) -> (Vec<N>, Vec<L>) {
        (
            self.selected_nodes.drain().collect(),
            self.selected_links.drain().collect(),
        )
    }

    pub fn new_node_position(
        &self,
        existing: impl IntoIterator<Item = Rect>,
        node_size: Vec2,
        preferred_center: Option<[f32; 2]>,
        grid: f32,
    ) -> [f32; 2] {
        let center = preferred_center.unwrap_or_else(|| {
            let center = (self.canvas.canvas_size * 0.5 - self.canvas.pan) / self.canvas.zoom;
            [center.x, center.y]
        });
        let snap = |value: f32| {
            if grid > 0.0 {
                (value / grid).round() * grid
            } else {
                value
            }
        };
        let mut candidate = [
            snap(center[0] - node_size.x * 0.5),
            snap(center[1] - node_size.y * 0.5),
        ];
        let existing = existing.into_iter().collect::<Vec<_>>();
        while existing.iter().any(|rect| {
            Rect::from_min_size(Pos2::new(candidate[0], candidate[1]), node_size)
                .expand(8.0)
                .intersects(rect.expand(8.0))
        }) {
            candidate[0] += grid.max(32.0);
            candidate[1] += grid.max(32.0);
        }
        candidate
    }

    pub fn clear_interaction(&mut self) {
        self.cancel_wire();
        self.drag_node = None;
        self.drag_origins.clear();
        self.drag_delta = Vec2::ZERO;
        self.box_select_start = None;
        self.box_select_current = None;
        self.add_node_center = None;
        self.secondary_action_suppressed = false;
    }

    pub fn cancel_current_operation(&mut self) {
        self.clear_interaction();
        self.selected_links.clear();
    }

    pub fn handle_navigation(
        &mut self,
        canvas: Rect,
        response: &Response,
        ui: &egui::Ui,
        allow_primary_pan: bool,
        allow_zoom: bool,
    ) {
        self.canvas
            .update_wire_dragging_navigation(canvas, ui, self.wire_active());
        let space_held = ui.input(|input| input.key_down(egui::Key::Space));
        if response.dragged_by(egui::PointerButton::Middle)
            || (allow_primary_pan
                && space_held
                && !self.wire_active()
                && response.dragged_by(egui::PointerButton::Primary))
        {
            self.canvas.pan += ui.input(|input| input.pointer.delta());
        }
        if allow_zoom && response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > f32::EPSILON {
                let pointer = ui
                    .input(|input| input.pointer.hover_pos())
                    .unwrap_or(canvas.center());
                self.canvas.zoom_at_pointer(canvas, pointer, scroll);
            }
        }
    }

    pub fn handle_canvas_selection(
        &mut self,
        response: &Response,
        ui: &egui::Ui,
        editable: bool,
        pointer_over_node: bool,
        pointer_over_link: bool,
        nodes: impl IntoIterator<Item = (N, Rect)>,
    ) {
        let space_held = ui.input(|input| input.key_down(egui::Key::Space));
        if editable
            && !space_held
            && !pointer_over_node
            && !pointer_over_link
            && !self.wire_active()
            && response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let extend = ui.input(|input| input.modifiers.shift || input.modifiers.ctrl);
            self.begin_box_selection(pointer, extend);
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            self.update_box_selection(response.interact_pointer_pos());
        }
        if editable
            && response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(selection) = self.finish_box_selection()
        {
            self.select_nodes(
                nodes
                    .into_iter()
                    .filter_map(|(node_id, rect)| selection.intersects(rect).then_some(node_id)),
            );
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && !pointer_over_node
            && !pointer_over_link
        {
            if self.wire_active() {
                self.cancel_wire();
            } else {
                self.clear_selection();
            }
        }
    }
}

pub(crate) fn paint_navigation_hint(
    ui: &egui::Ui,
    canvas: Rect,
    items: &[String],
    color: egui::Color32,
) {
    let font_id = egui::FontId::monospace(9.5);
    let line_height = 16.0;
    let base_y = canvas.bottom() - 12.0;
    let right_x = canvas.right() - 16.0;
    for (index, item) in items.iter().rev().enumerate() {
        ui.painter().text(
            Pos2::new(right_x, base_y - index as f32 * line_height),
            egui::Align2::RIGHT_BOTTOM,
            item,
            font_id.clone(),
            color,
        );
    }
}

/// Produces a left-to-right layered layout without depending on a graph domain.
///
/// Nodes are assigned to the layer after their deepest predecessor. Cyclic remnants stay in a
/// final layer instead of making layout fail; domain validation remains responsible for rejecting
/// cycles. Column widths and each node's real size determine spacing, so labels and ports have a
/// predictable corridor between adjacent layers.
pub(crate) fn layered_layout<N>(
    nodes: impl IntoIterator<Item = LayoutNode<N>>,
    edges: impl IntoIterator<Item = (N, N)>,
    options: LayeredLayoutOptions,
) -> Vec<NodeMove<N>>
where
    N: Clone + Eq + Hash,
{
    let nodes = nodes.into_iter().collect::<Vec<_>>();
    if nodes.is_empty() {
        return Vec::new();
    }
    let indexes = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut adjacency = vec![Vec::<usize>::new(); nodes.len()];
    let mut indegree = vec![0usize; nodes.len()];
    let mut seen_edges = HashSet::new();
    for (from, to) in edges {
        let (Some(&from), Some(&to)) = (indexes.get(&from), indexes.get(&to)) else {
            continue;
        };
        if seen_edges.insert((from, to)) {
            adjacency[from].push(to);
            indegree[to] += 1;
        }
    }

    let mut queue = std::collections::VecDeque::new();
    for (index, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            queue.push_back(index);
        }
    }
    let mut layer = vec![0usize; nodes.len()];
    let mut visited = vec![false; nodes.len()];
    while let Some(source) = queue.pop_front() {
        visited[source] = true;
        for &target in &adjacency[source] {
            layer[target] = layer[target].max(layer[source] + 1);
            indegree[target] -= 1;
            if indegree[target] == 0 {
                queue.push_back(target);
            }
        }
    }
    let fallback_layer = layer.iter().copied().max().unwrap_or(0) + 1;
    for (index, was_visited) in visited.into_iter().enumerate() {
        if !was_visited {
            layer[index] = fallback_layer;
        }
    }

    let layer_count = layer.iter().copied().max().unwrap_or(0) + 1;
    let mut columns = vec![Vec::<usize>::new(); layer_count];
    for (index, layer) in layer.into_iter().enumerate() {
        columns[layer].push(index);
    }
    let column_heights = columns
        .iter()
        .map(|column| {
            let content = column
                .iter()
                .map(|&index| nodes[index].size.y.max(1.0))
                .sum::<f32>();
            content + options.vertical_gap.max(0.0) * column.len().saturating_sub(1) as f32
        })
        .collect::<Vec<_>>();
    let maximum_height = column_heights.iter().copied().fold(0.0_f32, f32::max);
    let snap = |value: f32| {
        options
            .snap
            .filter(|grid| *grid > 0.0)
            .map_or(value, |grid| (value / grid).round() * grid)
    };

    let mut x = options.origin[0];
    let mut moves = Vec::with_capacity(nodes.len());
    for (column_index, column) in columns.iter().enumerate() {
        let column_width = column
            .iter()
            .map(|&index| nodes[index].size.x.max(1.0))
            .fold(0.0_f32, f32::max);
        let mut y = options.origin[1] + (maximum_height - column_heights[column_index]) * 0.5;
        for &index in column {
            moves.push(NodeMove {
                node_id: nodes[index].id.clone(),
                position: [snap(x), snap(y)],
            });
            y += nodes[index].size.y.max(1.0) + options.vertical_gap.max(0.0);
        }
        x += column_width + options.horizontal_gap.max(0.0);
    }
    moves
}

pub(crate) fn shortcut(ui: &egui::Ui) -> Option<GraphShortcut> {
    if ui.ctx().egui_wants_keyboard_input() {
        return None;
    }
    let ctrl = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
    let shift = ui.input(|input| input.modifiers.shift);
    if ctrl && !shift && ui.input(|input| input.key_pressed(egui::Key::Z)) {
        Some(GraphShortcut::Undo)
    } else if (ctrl && shift && ui.input(|input| input.key_pressed(egui::Key::Z)))
        || (ctrl && ui.input(|input| input.key_pressed(egui::Key::Y)))
    {
        Some(GraphShortcut::Redo)
    } else if ui.input(|input| {
        input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
    }) {
        Some(GraphShortcut::Delete)
    } else if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        Some(GraphShortcut::Cancel)
    } else {
        None
    }
}

pub(crate) fn closest_link<L: Clone>(
    pointer: Pos2,
    links: impl IntoIterator<Item = (L, [Pos2; 4])>,
    hit_radius: f32,
) -> Option<L> {
    links
        .into_iter()
        .filter_map(|(link, points)| {
            let distance = graph_canvas::distance_to_curve(pointer, points);
            (distance <= hit_radius).then_some((distance, link))
        })
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map(|(_, link)| link)
}

pub(crate) fn nearest_port<P: Clone>(
    pointer: Pos2,
    ports: impl IntoIterator<Item = (P, Pos2)>,
    hit_radius: f32,
) -> Option<P> {
    ports
        .into_iter()
        .filter_map(|(port, position)| {
            let distance = pointer.distance(position);
            (distance <= hit_radius).then_some((distance, port))
        })
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map(|(_, port)| port)
}

#[cfg(test)]
mod tests {
    use super::*;

    type State = GraphEditorState<&'static str, u8, &'static str, &'static str>;

    #[test]
    fn graph_switch_resets_all_transient_editor_state() {
        let mut state = State::default();
        state.select_node("node", false);
        state.select_link(1, false);
        state.start_wire_from_output("out");

        assert!(state.reset_for_graph("next"));
        assert!(state.selected_nodes.is_empty());
        assert!(state.selected_links.is_empty());
        assert!(!state.wire_active());
        assert!(state.canvas.fit_pending);
        assert!(!state.reset_for_graph("next"));
    }

    #[test]
    fn drag_uses_stable_origins_and_commits_all_selected_nodes() {
        let mut state = State::default();
        state.select_node("a", false);
        state.select_node("b", true);
        state.begin_node_drag("a", [("a", [0.0, 0.0]), ("b", [20.0, 10.0])]);
        state.update_node_drag(Vec2::new(17.0, 31.0));
        let mut moves = state.finish_node_drag(Some(16.0));
        moves.sort_by_key(|movement| movement.node_id);
        assert_eq!(moves[0].position, [16.0, 32.0]);
        assert_eq!(moves[1].position, [32.0, 48.0]);
    }

    #[test]
    fn rewire_commit_is_atomic_and_retains_the_replaced_link() {
        let mut state = State::default();
        state.start_rewire("source", 7);
        let commit = state.finish_wire(Some("target")).unwrap();
        assert_eq!(
            commit,
            WireCommit {
                from: "source",
                to: Some("target"),
                replaced: Some(7),
            }
        );
        assert!(!state.wire_active());
    }

    #[test]
    fn closest_link_returns_only_the_nearest_hit() {
        let first = graph_canvas::bezier_points(Pos2::ZERO, Pos2::new(100.0, 0.0));
        let second = graph_canvas::bezier_points(Pos2::new(0.0, 20.0), Pos2::new(100.0, 20.0));
        assert_eq!(
            closest_link(Pos2::new(50.0, 18.0), [(1, first), (2, second)], 12.0),
            Some(2)
        );
    }

    #[test]
    fn nearest_port_returns_the_closest_target_inside_the_hit_radius() {
        assert_eq!(
            nearest_port(
                Pos2::new(10.0, 0.0),
                [
                    ("far", Pos2::new(20.0, 0.0)),
                    ("near", Pos2::new(12.0, 0.0))
                ],
                12.0,
            ),
            Some("near")
        );
    }

    #[test]
    fn layered_layout_uses_real_node_sizes_and_keeps_columns_apart() {
        let moves = layered_layout(
            [
                LayoutNode {
                    id: "source-a",
                    size: Vec2::new(232.0, 148.0),
                },
                LayoutNode {
                    id: "source-b",
                    size: Vec2::new(232.0, 112.0),
                },
                LayoutNode {
                    id: "mixer",
                    size: Vec2::new(280.0, 112.0),
                },
                LayoutNode {
                    id: "sink",
                    size: Vec2::new(232.0, 148.0),
                },
            ],
            [
                ("source-a", "mixer"),
                ("source-b", "mixer"),
                ("mixer", "sink"),
            ],
            LayeredLayoutOptions {
                horizontal_gap: 160.0,
                vertical_gap: 64.0,
                snap: None,
                ..LayeredLayoutOptions::default()
            },
        );
        let positions = moves
            .into_iter()
            .map(|movement| (movement.node_id, movement.position))
            .collect::<HashMap<_, _>>();
        assert!(positions["source-b"][1] - positions["source-a"][1] >= 148.0 + 64.0);
        assert!(positions["mixer"][0] - positions["source-a"][0] >= 232.0 + 160.0);
        assert!(positions["sink"][0] - positions["mixer"][0] >= 280.0 + 160.0);
    }

    #[test]
    fn layered_layout_puts_cyclic_remainders_in_a_safe_final_column() {
        let moves = layered_layout(
            [
                LayoutNode {
                    id: 1,
                    size: Vec2::splat(100.0),
                },
                LayoutNode {
                    id: 2,
                    size: Vec2::splat(100.0),
                },
            ],
            [(1, 2), (2, 1)],
            LayeredLayoutOptions::default(),
        );
        assert_eq!(moves.len(), 2);
        assert_eq!(moves[0].position[0], moves[1].position[0]);
        assert!(moves[1].position[1] > moves[0].position[1]);
    }

    #[test]
    fn shared_history_undoes_redoes_and_discards_a_redo_branch() {
        let mut history = GraphEditHistory::new(3);
        history.push(1);
        history.push(2);
        assert_eq!(history.undo(3), Some(2));
        assert_eq!(history.undo(2), Some(1));
        assert_eq!(history.redo(1), Some(2));
        history.push(4);
        assert!(!history.can_redo());
        assert_eq!(history.undo_count(), 2);
    }

    #[test]
    fn history_skips_uncommitted_or_rejected_no_op_snapshots() {
        let mut history = GraphEditHistory::new(4);
        history.push(1);
        history.push(2);
        assert_eq!(history.undo(2), Some(1));
    }
}
