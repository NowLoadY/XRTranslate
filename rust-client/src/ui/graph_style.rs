//! Shared visual language for XRTranslate graph editors.

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};

pub const GRAPH_ACCENT: Color32 = Color32::from_gray(72);
pub const BAR_FILL: Color32 = Color32::from_gray(247);
pub const BAR_BORDER: Color32 = Color32::from_gray(211);
pub const INK: Color32 = Color32::from_gray(68);
pub const MUTED: Color32 = Color32::from_gray(112);
pub const CANVAS_FILL: Color32 = Color32::from_gray(250);
pub const CANVAS_BORDER: Color32 = Color32::from_gray(208);
pub const GRID: Color32 = Color32::from_gray(222);
pub const NODE_TEXT: Color32 = Color32::from_gray(55);
pub const NODE_MUTED: Color32 = Color32::from_gray(105);
pub const NODE_BORDER: Color32 = Color32::from_gray(195);
pub const LINK: Color32 = Color32::from_gray(112);
pub const LINK_INACTIVE: Color32 = Color32::from_gray(184);
pub const LINK_SELECTED: Color32 = Color32::from_rgb(64, 132, 228);
pub const ERROR_BORDER: Color32 = Color32::from_rgb(232, 110, 95);
pub const ERROR_FILL: Color32 = Color32::from_rgba_unmultiplied_const(253, 242, 240, 200);

pub fn command_button(ui: &mut egui::Ui, label: &str, filled: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(if filled {
            Color32::WHITE
        } else {
            INK
        }))
        .fill(if filled { LINK_SELECTED } else { BAR_FILL })
        .stroke(if filled {
            Stroke::NONE
        } else {
            Stroke::new(1.0, BAR_BORDER)
        })
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(0.0, 28.0)),
    )
}

pub fn toolbar_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| command_button(ui, label, false))
        .inner
}

pub fn tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(if selected {
            LINK_SELECTED
        } else {
            MUTED
        }))
        .fill(if selected {
            Color32::from_rgb(233, 240, 251)
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(0.0, 28.0)),
    )
}

pub fn canvas_frame() -> Frame {
    Frame::new()
        .fill(CANVAS_FILL)
        .stroke(Stroke::new(1.0, CANVAS_BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(4))
}

pub fn apply(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = Vec2::new(7.0, 6.0);
    style.spacing.button_padding = Vec2::new(9.0, 5.0);
    let visuals = &mut style.visuals;
    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.corner_radius = CornerRadius::same(6);
        widgets.expansion = 0.0;
    }
    visuals.selection.bg_fill = Color32::from_rgb(224, 235, 250);
    visuals.selection.stroke = Stroke::new(1.0, LINK_SELECTED);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BAR_BORDER);
    visuals.widgets.inactive.bg_fill = BAR_FILL;
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.popup_shadow = egui::Shadow::NONE;
}
