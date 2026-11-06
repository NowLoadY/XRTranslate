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
    let size = Vec2::new(width, 26.0);
    let (id, space) = ui.allocate_space(size);
    let rect = ui.layout().align_size_within_rect(size, space);
    let mut response = ui.interact(rect, id, Sense::click_and_drag());
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
    if let Some(value) =
        ui_director::egui::record_toggle(ui, response.id, &labels.join(" / "), *second, true, rect)
    {
        *second = value;
    }
    if previous != *second {
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn justified_layout_keeps_blank_space_outside_the_switch() {
        let ctx = egui::Context::default();
        let mut second = false;
        let mut frame = |events| {
            let mut rect = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 200.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        rect = segmented_switch(ui, &mut second, ["Audio", "Subtitles"]).rect;
                    });
                },
            );
            output.textures_delta.clear();
            (second, rect)
        };
        let (_, rect) = frame(vec![]);
        let blank = egui::pos2(500.0, rect.center().y);
        let pointer = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        frame(pointer(blank, true));
        let (second, rect) = frame(pointer(blank, false));
        assert!(!second, "clicking blank space must not select subtitles");
        let subtitle = egui::pos2(rect.right() - 10.0, rect.center().y);
        frame(pointer(subtitle, true));
        assert!(frame(pointer(subtitle, false)).0);
    }
}
