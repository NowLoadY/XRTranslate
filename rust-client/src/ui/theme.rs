use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke, Visuals};

pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = Visuals::light();

    // Popup windows (ComboBox, menus, tooltips) use `window_fill`. Keep them
    // readable over the acrylic surface while the root viewport remains
    // transparent through the app's WGPU clear color.
    visuals.window_fill = Color32::from_rgba_unmultiplied(242, 244, 244, 248);
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.faint_bg_color = surface_subtle();
    // Scroll areas use this color for their extreme background. Keeping it
    // transparent prevents a stale-looking white rectangle below live bubbles.
    visuals.extreme_bg_color = Color32::TRANSPARENT;

    let border_stroke = Stroke::new(1.0, border());

    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.bg_stroke = border_stroke;
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.expansion = 0.0;

    visuals.widgets.inactive.bg_fill = surface_control();
    visuals.widgets.inactive.weak_bg_fill = surface_control();
    visuals.widgets.inactive.bg_stroke = border_stroke;
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
    visuals.widgets.inactive.expansion = 0.0;

    visuals.widgets.hovered.bg_fill = surface_control_hover();
    visuals.widgets.hovered.weak_bg_fill = surface_control_hover();
    visuals.widgets.hovered.bg_stroke = border_stroke;
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = surface_control_active();
    visuals.widgets.active.weak_bg_fill = surface_control_active();
    visuals.widgets.active.bg_stroke = border_stroke;
    visuals.widgets.active.corner_radius = CornerRadius::same(8);
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.bg_stroke = border_stroke;
    visuals.widgets.open.corner_radius = CornerRadius::same(8);
    visuals.widgets.open.expansion = 0.0;

    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_normal());
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_strong());
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, primary());
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, primary_dark());
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, primary_dark());

    visuals.selection.bg_fill = Color32::TRANSPARENT;
    visuals.selection.stroke = Stroke::new(1.0, border_strong());
    visuals.hyperlink_color = primary_dark();

    visuals.slider_trailing_fill = true;
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.window_shadow = egui::Shadow::NONE;

    ctx.set_visuals(visuals.clone());
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Light);

    ctx.all_styles_mut(move |style| {
        style.visuals = visuals.clone();
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.window_margin = Margin::same(12);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

pub fn text_strong() -> Color32 {
    Color32::from_rgb(30, 36, 40)
}

pub fn text_normal() -> Color32 {
    Color32::from_rgb(62, 70, 74)
}

pub fn text_weak() -> Color32 {
    Color32::from_rgb(105, 114, 117)
}

pub fn surface_subtle() -> Color32 {
    Color32::from_rgba_unmultiplied(220, 225, 225, 68)
}

/// Neutral history layers keep the message bubbles from picking up a blue cast
/// when composited over the Windows acrylic backdrop.
pub fn history_surface() -> Color32 {
    Color32::from_rgba_unmultiplied(214, 216, 216, 72)
}

pub fn history_viewport() -> Color32 {
    Color32::from_rgba_unmultiplied(224, 224, 224, 62)
}

pub fn surface_control() -> Color32 {
    Color32::from_rgba_unmultiplied(205, 212, 214, 72)
}

pub fn surface_control_hover() -> Color32 {
    Color32::from_rgba_unmultiplied(196, 205, 208, 92)
}

pub fn surface_control_active() -> Color32 {
    Color32::from_rgba_unmultiplied(188, 198, 201, 116)
}

pub fn sidebar(focused: bool) -> Color32 {
    let alpha = if focused { 174 } else { 132 };
    Color32::from_rgba_unmultiplied(255, 255, 255, alpha)
}

pub fn content_backdrop(focused: bool) -> Color32 {
    let alpha = if focused { 172 } else { 116 };
    Color32::from_rgba_unmultiplied(255, 255, 255, alpha)
}

pub fn modal_backdrop() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 238)
}

pub fn border() -> Color32 {
    Color32::from_rgba_unmultiplied(28, 33, 36, 210)
}

pub fn primary() -> Color32 {
    Color32::from_rgb(29, 78, 216)
}

pub fn primary_dark() -> Color32 {
    // Selected/pressed feedback is intentionally a little lighter than the
    // hover blue, so selected items do not look heavier than hover states.
    Color32::from_rgb(37, 99, 235)
}

pub fn primary_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(37, 99, 235, 150)
}

pub fn success() -> Color32 {
    Color32::from_rgb(48, 91, 78)
}

pub fn danger() -> Color32 {
    Color32::from_rgb(132, 62, 62)
}

pub fn border_strong() -> Color32 {
    Color32::from_rgb(70, 78, 81)
}
