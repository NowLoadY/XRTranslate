use eframe::egui::{self, Color32, CornerRadius, Id, Margin, Stroke, Visuals};
use serde::{Deserialize, Serialize};

fn ui_theme_id() -> Id {
    Id::new("xrtranslate_ui_theme")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeVariant {
    #[default]
    Default,
    HandDrawn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTheme {
    #[serde(default)]
    pub variant: ThemeVariant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationTimings {
    pub hover: f32,
    pub active: f32,
    pub selection: f32,
    pub toggle: f32,
    pub button_click: f32,
    pub primary_click: f32,
    pub sidebar: f32,
    pub page: f32,
    pub page_flip: f32,
    pub data_text: f32,
    pub window_resize: f32,
}

impl Default for AnimationTimings {
    fn default() -> Self {
        Self {
            hover: 0.15,
            active: 0.08,
            selection: 0.20,
            toggle: 0.18,
            button_click: 0.28,
            primary_click: 0.25,
            sidebar: 0.20,
            page: 0.32,
            page_flip: 0.32,
            data_text: 0.32,
            window_resize: 0.26,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataTextMotion {
    pub min_opacity: f32,
    pub max_offset: f32,
}

impl Default for DataTextMotion {
    fn default() -> Self {
        Self {
            min_opacity: 0.64,
            max_offset: 3.0,
        }
    }
}

pub fn install_context(ctx: &egui::Context, theme: UiTheme) {
    ctx.data_mut(|data| data.insert_temp(ui_theme_id(), theme));
}

pub fn context_theme(ctx: &egui::Context) -> UiTheme {
    ctx.data(|data| data.get_temp(ui_theme_id()).unwrap_or_default())
}

pub fn variant(ctx: &egui::Context) -> ThemeVariant {
    context_theme(ctx).variant
}

pub fn is_hand_drawn(ctx: &egui::Context) -> bool {
    variant(ctx) == ThemeVariant::HandDrawn
}

pub fn animation_timings(_ctx: &egui::Context) -> AnimationTimings {
    AnimationTimings::default()
}

pub fn data_text_motion(_ctx: &egui::Context) -> DataTextMotion {
    DataTextMotion::default()
}

pub fn data_text_target_opacity(ctx: &egui::Context, activity: f32) -> f32 {
    let activity = activity.clamp(0.0, 1.0);
    let motion = data_text_motion(ctx);
    motion.min_opacity + (1.0 - motion.min_opacity) * activity
}

pub fn border_stroke(color: Color32) -> Stroke {
    Stroke::new(1.0, color)
}

pub const WIDGET_RADIUS: u8 = 8;

/// Calculates the mathematically correct outer radius for a container to achieve
/// perfect concentric harmony with standard child widgets inside it.
/// Outer Radius = Inner Radius + Padding.
pub fn container_radius(padding: u8) -> CornerRadius {
    CornerRadius::same(WIDGET_RADIUS.saturating_add(padding))
}

pub fn container_radius_f32(padding: f32) -> f32 {
    (WIDGET_RADIUS as f32) + padding
}

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

    let border_stroke = border_stroke(border());

    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.bg_stroke = border_stroke;
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    visuals.widgets.noninteractive.expansion = 0.0;

    visuals.widgets.inactive.bg_fill = surface_control();
    visuals.widgets.inactive.weak_bg_fill = surface_control();
    visuals.widgets.inactive.bg_stroke = border_stroke;
    visuals.widgets.inactive.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    visuals.widgets.inactive.expansion = 0.0;

    visuals.widgets.hovered.bg_fill = surface_control_hover();
    visuals.widgets.hovered.weak_bg_fill = surface_control_hover();
    visuals.widgets.hovered.bg_stroke = border_stroke;
    visuals.widgets.hovered.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = surface_control_active();
    visuals.widgets.active.weak_bg_fill = surface_control_active();
    visuals.widgets.active.bg_stroke = border_stroke;
    visuals.widgets.active.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.open.bg_stroke = border_stroke;
    visuals.widgets.open.corner_radius = CornerRadius::same(WIDGET_RADIUS);
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
    visuals.menu_corner_radius = CornerRadius::same(WIDGET_RADIUS);
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
    Color32::from_rgba_unmultiplied(246, 246, 246, 64)
}

/// A readable white backing for bounded content over custom background images.
pub fn surface_panel() -> Color32 {
    Color32::from_white_alpha(184)
}

pub fn set_custom_background(ctx: &egui::Context, visible: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::new("custom_background_visible"), visible));
}

pub fn panel_fill(ctx: &egui::Context, default: Color32) -> Color32 {
    if ctx.data(|data| {
        data.get_temp::<bool>(Id::new("custom_background_visible"))
            .unwrap_or(false)
    }) {
        surface_panel()
    } else {
        default
    }
}

/// Neutral history layers keep the message bubbles from picking up a blue cast
/// when composited over the Windows acrylic backdrop.
pub fn history_surface() -> Color32 {
    Color32::from_rgba_unmultiplied(248, 248, 248, 100)
}

pub fn history_viewport() -> Color32 {
    Color32::from_rgba_unmultiplied(224, 224, 224, 62)
}

pub fn surface_control() -> Color32 {
    Color32::from_rgba_unmultiplied(226, 228, 228, 100)
}

pub fn surface_control_hover() -> Color32 {
    Color32::from_rgba_unmultiplied(212, 216, 216, 120)
}

pub fn surface_control_active() -> Color32 {
    Color32::from_rgba_unmultiplied(199, 205, 205, 140)
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
    Color32::from_rgb(255, 255, 255)
}

pub fn border() -> Color32 {
    Color32::from_rgba_unmultiplied(100, 110, 114, 110)
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

pub fn danger() -> Color32 {
    Color32::from_rgb(132, 62, 62)
}

pub fn border_strong() -> Color32 {
    Color32::from_rgb(70, 78, 81)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_the_default() {
        assert_eq!(UiTheme::default().variant, ThemeVariant::Default);
        assert_eq!(
            serde_json::from_str::<UiTheme>("{}").unwrap(),
            UiTheme::default()
        );
    }

    #[test]
    fn hand_drawn_theme_round_trips_as_snake_case() {
        let theme = UiTheme {
            variant: ThemeVariant::HandDrawn,
        };
        assert_eq!(
            serde_json::to_string(&theme).unwrap(),
            r#"{"variant":"hand_drawn"}"#
        );
        assert_eq!(
            serde_json::from_str::<UiTheme>(r#"{"variant":"hand_drawn"}"#).unwrap(),
            theme
        );
    }

    #[test]
    fn legacy_border_theme_names_are_not_accepted() {
        assert!(serde_json::from_str::<UiTheme>(r#"{"variant":"organic"}"#).is_err());
    }

    #[test]
    fn data_text_opacity_combines_activity_with_a_theme_floor() {
        let ctx = egui::Context::default();
        assert_eq!(data_text_target_opacity(&ctx, 0.0), 0.64);
        assert_eq!(data_text_target_opacity(&ctx, 1.0), 1.0);
        assert_eq!(data_text_target_opacity(&ctx, 0.5), 0.82);
    }
}
