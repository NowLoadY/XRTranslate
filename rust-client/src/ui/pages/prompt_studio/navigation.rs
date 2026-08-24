use super::*;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

/// Renders the keybinding and navigation cheatsheet in the bottom right corner of the canvas.
pub(super) fn render_canvas_navigation_hint(
    ui: &egui::Ui,
    canvas: Rect,
    language: crate::i18n::UiLanguage,
) {
    let items = [
        (
            "NAVIGATE",
            "Space + Left Drag / Middle Drag to pan · Mouse Wheel to zoom",
        ),
        (
            "SELECT",
            "Left Drag on canvas to box select · Shift + Click to multi-select",
        ),
        (
            "CONNECT",
            "Drag socket to connect / unplug · Click empty space to cancel wire",
        ),
        (
            "ACTIONS",
            "Del to delete · Double-Click header to rename · Ctrl+Z: Undo · Ctrl+Y: Redo",
        ),
    ];

    let lines = items
        .into_iter()
        .map(|(tag, detail)| {
            format!(
                "{} · {}",
                crate::i18n::tr(language, tag),
                crate::i18n::tr(language, detail)
            )
        })
        .collect::<Vec<_>>();
    crate::ui::graph_editor::paint_navigation_hint(ui, canvas, &lines, Color32::BLACK);
}

/// Centers and scales the canvas viewport so all visible graph nodes fit comfortably.
pub(super) fn fit_graph_to_canvas(
    graph: &PromptNodeGraph,
    controller: &mut PromptStudioController,
    available: Vec2,
) {
    let mut visible = graph
        .nodes
        .iter()
        .filter(|node| controller.node_is_visible(node));
    let Some(first) = visible.next() else {
        return;
    };
    let first_size = node_size(graph, first);
    let mut bounds =
        Rect::from_min_size(Pos2::new(first.position[0], first.position[1]), first_size);
    for node in visible {
        let size = node_size(graph, node);
        bounds = bounds.union(Rect::from_min_size(
            Pos2::new(node.position[0], node.position[1]),
            size,
        ));
    }
    controller
        .canvas
        .fit_to_bounds(bounds, available, Vec2::new(NODE_WIDTH, 84.0));
}
