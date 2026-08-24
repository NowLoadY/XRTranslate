use eframe::egui::{self, Align, Layout, Pos2, Rect, RichText, Stroke, UiBuilder};
use xrtranslate_prompt::{PromptExecutionTrace, PromptNode, PromptNodeGraph, PromptNodeKind};

pub(super) const WIDTH: f32 = 220.0;
const INSET: f32 = 8.0;
const STATUS_HEIGHT: f32 = 18.0;

pub(super) fn pane_rect(node_rect: Rect, scale: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(
            node_rect.right() - (WIDTH - INSET) * scale,
            node_rect.top() + (NODE_HEADER_HEIGHT + INSET) * scale,
        ),
        Pos2::new(
            node_rect.right() - INSET * scale,
            node_rect.bottom() - INSET * scale,
        ),
    )
}

pub(super) fn configuration_right(node_rect: Rect, scale: f32) -> f32 {
    pane_rect(node_rect, scale).left() - INSET * scale
}

pub(super) fn render(
    ui: &mut egui::Ui,
    node_rect: Rect,
    graph: &PromptNodeGraph,
    node: &PromptNode,
    trace: Option<&PromptExecutionTrace>,
    scale: f32,
    language: crate::i18n::UiLanguage,
) {
    if scale < 0.58 {
        return;
    }
    let pane = pane_rect(node_rect, scale);
    ui.painter().line_segment(
        [
            Pos2::new(pane.left() - 4.0 * scale, pane.top()),
            Pos2::new(pane.left() - 4.0 * scale, pane.bottom()),
        ],
        Stroke::new(1.0, super::style::BAR_BORDER),
    );

    let node_trace = trace.and_then(|trace| trace.node(&node.id));
    let status = match (trace, node_trace) {
        (None, _) => crate::i18n::tr(language, "NO LIVE DATA").to_owned(),
        (Some(_), None) => crate::i18n::tr(language, "NOT USED").to_owned(),
        (Some(_), Some(node_trace)) => node_trace.selected_input.map_or_else(
            || crate::i18n::tr(language, "LIVE OUTPUT").to_owned(),
            |input| {
                format!(
                    "{} / {}",
                    crate::i18n::tr(language, "LIVE OUTPUT"),
                    crate::i18n::tr_dynamic(
                        language,
                        &super::input_socket_label(graph, node, input),
                    )
                )
            },
        ),
    };
    ui.painter().text(
        pane.min,
        egui::Align2::LEFT_TOP,
        status,
        egui::FontId::monospace((8.0 * scale).max(6.5)),
        super::style::NODE_MUTED,
    );

    let content = Rect::from_min_max(
        Pos2::new(pane.left(), pane.top() + STATUS_HEIGHT * scale),
        pane.max,
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(content)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content));
            egui::ScrollArea::vertical()
                .id_salt(("prompt_node_runtime", &node.id))
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .max_height(content.height())
                .show(ui, |ui| {
                    ui.set_min_width((content.width() - 12.0 * scale).max(1.0));
                    let output = node_trace.map_or("", |node| node.output.as_str());
                    let output = if output.is_empty() {
                        match (trace, node_trace) {
                            (Some(_), Some(_)) => crate::i18n::tr(language, "(empty)"),
                            _ => "",
                        }
                    } else {
                        output
                    };
                    ui.label(
                        RichText::new(output)
                            .font(egui::FontId::monospace((9.0 * scale).max(7.0)))
                            .color(super::style::NODE_TEXT),
                    );
                });
        },
    );
}

pub(super) fn base_width(kind: &PromptNodeKind) -> f32 {
    match kind {
        PromptNodeKind::Compose { .. } => 320.0,
        PromptNodeKind::Switch { .. } | PromptNodeKind::Request { .. } => 260.0,
        _ => super::NODE_WIDTH,
    }
}

const NODE_HEADER_HEIGHT: f32 = super::NODE_HEADER_HEIGHT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_pane_keeps_a_fixed_configuration_region() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::Vec2::new(540.0, 160.0));
        let pane = pane_rect(rect, 1.0);

        assert_eq!(pane.width(), WIDTH - INSET * 2.0);
        assert_eq!(configuration_right(rect, 1.0), 320.0);
    }
}
