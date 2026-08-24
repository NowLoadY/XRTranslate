//! Shared visual language for XRTranslate graph editors.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Vec2};

pub const GRAPH_ACCENT: Color32 = Color32::from_gray(72);
pub const BAR_FILL: Color32 = Color32::from_rgba_unmultiplied_const(250, 250, 249, 164);
pub const BAR_BORDER: Color32 = Color32::from_gray(194);
pub const INK: Color32 = Color32::from_gray(68);
pub const MUTED: Color32 = Color32::from_gray(112);
pub const CANVAS_FILL: Color32 = Color32::from_rgba_unmultiplied_const(242, 243, 242, 104);
pub const CANVAS_BORDER: Color32 = Color32::from_gray(188);
pub const GRID: Color32 = Color32::from_gray(218);
pub const NODE_TEXT: Color32 = Color32::from_gray(55);
pub const NODE_MUTED: Color32 = Color32::from_gray(105);
pub const NODE_BORDER: Color32 = Color32::from_gray(155);
pub const LINK: Color32 = Color32::from_gray(112);
pub const LINK_INACTIVE: Color32 = Color32::from_gray(184);
pub const LINK_SELECTED: Color32 = Color32::from_rgb(64, 132, 228);
pub const ERROR_BORDER: Color32 = Color32::from_rgb(232, 110, 95);
pub const ERROR_FILL: Color32 = Color32::from_rgba_unmultiplied_const(253, 242, 240, 200);

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
        widgets.corner_radius = CornerRadius::same(1);
        widgets.expansion = 0.0;
    }
    visuals.selection.bg_fill = Color32::from_gray(218);
    visuals.selection.stroke = Stroke::new(1.0, GRAPH_ACCENT);
    visuals.menu_corner_radius = CornerRadius::same(1);
    visuals.popup_shadow = egui::Shadow::NONE;
}
