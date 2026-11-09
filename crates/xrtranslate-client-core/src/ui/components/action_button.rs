use crate::ui::{animation::AnimationSystem, theme};
use eframe::egui::{self, Ui, Vec2};

pub fn copy_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/copy.svg"),
    )
}

pub(super) fn recognition_input_button(ui: &mut Ui, label: &str, active: bool) -> egui::Response {
    let accent = if active {
        theme::primary()
    } else {
        theme::text_normal()
    };
    icon_button_with_colors(
        ui,
        label,
        egui::include_image!("../../../resources/icons/microphone.svg"),
        32.0,
        [
            if active {
                theme::primary().gamma_multiply(0.12)
            } else {
                egui::Color32::TRANSPARENT
            },
            theme::primary(),
            if active {
                accent
            } else {
                theme::border().gamma_multiply(0.6)
            },
            accent,
        ],
    )
}

pub(super) fn floating_copy_button(ui: &mut Ui, label: &str) -> egui::Response {
    let diameter = 32.0_f32.max(ui.spacing().interact_size.y);
    theme::floating_frame(egui::CornerRadius::same((diameter * 0.5) as u8))
        // The shared icon button paints the outline and focus feedback.
        .stroke(egui::Stroke::NONE)
        .show(ui, |ui| {
            icon_button_with_colors(
                ui,
                label,
                egui::include_image!("../../../resources/icons/copy.svg"),
                diameter,
                [
                    egui::Color32::TRANSPARENT,
                    theme::primary(),
                    theme::border(),
                    theme::text_normal(),
                ],
            )
        })
        .inner
}

pub fn edit_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/edit.svg"),
    )
}

pub fn delete_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/trash.svg"),
    )
}

pub fn back_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/chevron-left.svg"),
    )
}

pub fn next_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/chevron-right.svg"),
    )
}

pub fn search_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/search.svg"),
    )
}

pub fn add_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/plus.svg"),
    )
}

pub fn refresh_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/reset.svg"),
    )
}

pub fn open_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/folder-open.svg"),
    )
}

pub fn icon_menu(
    ui: &mut Ui,
    label: &str,
    icon: egui::ImageSource<'_>,
    contents: impl FnOnce(&mut Ui),
) {
    let response = action_button(ui, label, icon);
    egui::Popup::menu(&response).show(contents);
}

/// A circular action in the same monochrome style as the smaller action buttons.
pub fn round_icon_button(
    ui: &mut Ui,
    label: &str,
    icon: egui::ImageSource<'_>,
    diameter: f32,
) -> egui::Response {
    icon_button_with_colors(
        ui,
        label,
        icon,
        diameter,
        [
            theme::surface_subtle(),
            theme::primary(),
            theme::border().gamma_multiply(0.6),
            theme::text_normal(),
        ],
    )
}

fn action_button(ui: &mut Ui, label: &str, icon: egui::ImageSource<'_>) -> egui::Response {
    icon_button_with_colors(
        ui,
        label,
        icon,
        32.0,
        [
            theme::surface_subtle(),
            theme::primary(),
            theme::border().gamma_multiply(0.6),
            theme::text_normal(),
        ],
    )
}

/// SVG action using the containing UI's palette, including isolated monochrome surfaces.
pub fn icon_button(ui: &mut Ui, label: &str, icon: egui::ImageSource<'_>) -> egui::Response {
    icon_button_with_colors(
        ui,
        label,
        icon,
        32.0,
        [
            ui.visuals().panel_fill,
            ui.visuals().text_color(),
            ui.visuals().widgets.inactive.bg_stroke.color,
            ui.visuals().text_color(),
        ],
    )
}

fn icon_button_with_colors(
    ui: &mut Ui,
    label: &str,
    icon: egui::ImageSource<'_>,
    diameter: f32,
    [background, accent, border, foreground]: [egui::Color32; 4],
) -> egui::Response {
    let diameter = diameter.max(ui.spacing().interact_size.y);
    let response = ui.add_sized(Vec2::splat(diameter), egui::Button::new("").frame(false));
    let response = ui_director::egui::Control::button(label).response(ui, response);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let hover = AnimationSystem::hover(
        ui.ctx(),
        response.id.with("hover"),
        response.hovered() || response.has_focus(),
    );
    let pressed = AnimationSystem::active(
        ui.ctx(),
        response.id.with("pressed"),
        response.is_pointer_button_down_on(),
    );
    let fill = AnimationSystem::lerp_color(
        background,
        accent.gamma_multiply(0.12 + pressed * 0.08),
        hover.max(pressed),
    );
    ui.painter().circle(
        response.rect.center(),
        response.rect.width().min(response.rect.height()) * 0.5,
        fill,
        egui::Stroke::new(if response.has_focus() { 1.5 } else { 0.85 }, border),
    );
    egui::Image::new(icon).tint(foreground).paint_at(
        ui,
        egui::Rect::from_center_size(response.rect.center(), Vec2::splat(diameter * 0.54)),
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}
