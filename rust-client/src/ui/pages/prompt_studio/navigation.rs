use super::*;
use eframe::egui::{Pos2, Rect, Vec2};

pub(super) fn navigation_help(language: crate::i18n::UiLanguage) -> String {
    [
        "Space + Left Drag / Middle Drag to pan · Mouse Wheel to zoom",
        "Left Drag on canvas to box select · Shift + Click to multi-select",
        "Drag socket to connect / unplug · Click empty space to cancel wire",
        "Del to delete · Double-Click header to rename · Ctrl+Z: Undo · Ctrl+Y: Redo",
    ]
    .map(|text| crate::i18n::tr(language, text))
    .join("\n")
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
    let mut bounds = Rect::from_min_size(Pos2::from(controller.node_position(first)), first_size);
    for node in visible {
        let size = node_size(graph, node);
        bounds = bounds.union(Rect::from_min_size(
            Pos2::from(controller.node_position(node)),
            size,
        ));
    }
    controller
        .canvas
        .fit_to_bounds(bounds, available, Vec2::new(NODE_WIDTH, 84.0));
}
