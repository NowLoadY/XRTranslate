use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

pub(super) use crate::ui::graph_style::{
    BAR_BORDER, BAR_FILL, CANVAS_BORDER, CANVAS_FILL, ERROR_BORDER, ERROR_FILL, GRAPH_ACCENT, GRID,
    INK, LINK_SELECTED, MUTED, NODE_BORDER, NODE_MUTED, NODE_TEXT,
};

pub(super) fn apply(ui: &mut egui::Ui) {
    crate::ui::graph_style::apply(ui);
}

pub(super) fn command_button(ui: &mut egui::Ui, text: &str, filled: bool) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(egui::FontId::monospace(9.5))
                .color(if filled { Color32::WHITE } else { INK }),
        )
        .fill(if filled {
            Color32::from_gray(76)
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::new(1.0, BAR_BORDER))
        .corner_radius(CornerRadius::same(1))
        .min_size(Vec2::new(62.0, 25.0)),
    )
}

pub(super) fn provider_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .font(egui::FontId::monospace(9.5))
                .color(if selected { Color32::WHITE } else { MUTED }),
        )
        .fill(if selected {
            Color32::from_gray(82)
        } else {
            Color32::TRANSPARENT
        })
        .stroke(if selected {
            Stroke::NONE
        } else {
            Stroke::new(1.0, BAR_BORDER)
        })
        .corner_radius(CornerRadius::same(1))
        .min_size(Vec2::new(70.0, 24.0)),
    )
}
