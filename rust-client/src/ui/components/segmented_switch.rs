//! Compact two-way selection with a sliding indicator.
use crate::ui::{animation::AnimationSystem, theme};
use eframe::egui::{self, Align2, Color32, FontId, Response, Sense, Stroke, Ui, Vec2};

pub fn segmented_switch(ui: &mut Ui, second: &mut bool, labels: [&str; 2]) -> Response {
    let font = FontId::proportional(11.5);
    let label_width = labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).into(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    let width = ((label_width + 20.0) * 2.0).min(ui.available_width());
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click_and_drag());
    let previous = *second;
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        *second = pointer.x >= rect.center().x;
    }
    if response.has_focus() {
        ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowLeft) {
                *second = false;
            }
            if input.key_pressed(egui::Key::ArrowRight) {
                *second = true;
            }
            if input.key_pressed(egui::Key::Space) || input.key_pressed(egui::Key::Enter) {
                *second = !*second;
            }
        });
    }
    if let Some(value) = crate::ui::automation::record_toggle(
        ui,
        response.id,
        &labels.join(" / "),
        *second,
        true,
        rect,
    ) {
        *second = value;
    }
    if previous != *second {
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            *second,
            labels[usize::from(*second)],
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(
            rect,
            8.0,
            theme::history_viewport(),
            Stroke::new(1.0, theme::border().gamma_multiply(0.4)),
            egui::StrokeKind::Inside,
        );
        let t = AnimationSystem::toggle(ui.ctx(), response.id.with("selection"), *second);
        let thumb = egui::Rect::from_min_size(
            rect.min + Vec2::new(2.0 + t * (width * 0.5), 2.0),
            Vec2::new(width * 0.5 - 4.0, 22.0),
        );
        painter.rect(
            thumb,
            6.0,
            theme::panel_fill(ui.ctx(), theme::surface_subtle()),
            Stroke::new(1.0, theme::border().gamma_multiply(0.45)),
            egui::StrokeKind::Inside,
        );
        for (index, label) in labels.into_iter().enumerate() {
            painter.text(
                egui::pos2(
                    rect.left() + width * (0.25 + index as f32 * 0.5),
                    rect.center().y,
                ),
                Align2::CENTER_CENTER,
                label,
                font.clone(),
                if index == usize::from(*second) {
                    theme::primary_dark()
                } else {
                    theme::text_weak()
                },
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
