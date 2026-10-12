use crate::ui::{animation::AnimationSystem, theme};
use eframe::egui::{self, Ui, Vec2};

pub fn copy_button(ui: &mut Ui, label: &str) -> egui::Response {
    action_button(
        ui,
        label,
        egui::include_image!("../../../resources/icons/copy.svg"),
    )
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

fn action_button(ui: &mut Ui, label: &str, icon: egui::ImageSource<'_>) -> egui::Response {
    let response = ui.add_sized(Vec2::splat(32.0), egui::Button::new("").frame(false));
    let response =
        crate::ui::automation::button_response(ui, response.id, label, ui.is_enabled(), response);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
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
        theme::surface_subtle(),
        theme::primary().gamma_multiply(0.12 + pressed * 0.08),
        hover.max(pressed),
    );
    ui.painter().circle(
        response.rect.center(),
        16.0,
        fill,
        egui::Stroke::new(1.0, theme::border().gamma_multiply(0.6)),
    );
    egui::Image::new(icon).tint(theme::text_normal()).paint_at(
        ui,
        egui::Rect::from_center_size(response.rect.center(), Vec2::splat(17.0)),
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}
