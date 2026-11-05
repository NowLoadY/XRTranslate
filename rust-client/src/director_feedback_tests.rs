use eframe::egui::{self, Id, Pos2, Rect};
use ui_director::{
    egui::feedback::*,
    feedback::{Effect, Feedback},
};

use crate::ui::virtual_list::{self, ScrollTarget};

#[test]
fn reveals_a_virtual_row_inside_a_scrolled_page_and_clears_stale_geometry() {
    let ctx = egui::Context::default();
    let rows: Vec<_> = (0..100)
        .map(|index| (40.0, Id::unique("row").with(index)))
        .collect();
    let target = rows[75].1;
    let mut visible = false;
    let mut connected = false;
    for frame in 0..60 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 450.0))),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ui| {
                begin_frame(&ctx);
                if frame == 0 {
                    show_in(&ctx, [Feedback::new(target.value(), "Row", Effect::Read)]);
                }
                egui::ScrollArea::vertical().id_salt("page").show(ui, |ui| {
                    ui.add_space(800.0);
                    ui.allocate_ui(egui::vec2(500.0, 200.0), |ui| {
                        virtual_list::show_variable_virtual_rows(
                            ui,
                            "list",
                            4.0,
                            ScrollTarget::None,
                            Some(|id: &Id| *id),
                            |_| &rows,
                            |ui, index, _, _| {
                                ui.label(format!("Row {index}"));
                            },
                        );
                    });
                });
                visible = reveal_target(&ctx).is_none()
                    && current(&ctx)
                        .first()
                        .and_then(|item| item.rect)
                        .is_some_and(|rect| ctx.content_rect().contains_rect(rect));
                paint(
                    &ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        Id::unique("feedback_test"),
                    )),
                    Some((egui::pos2(30.0, 30.0), 20.0)),
                    egui::Color32::BLUE,
                );
            },
        );
        connected |= output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::LineSegment { stroke, .. } if stroke.width > 0.0 && stroke.color == egui::Color32::BLUE));
        output.textures_delta.clear();
        if visible && connected {
            break;
        }
    }
    assert!(
        visible,
        "both page and virtual list must reveal the selected row"
    );
    assert!(
        connected,
        "draw a dashed connection from the avatar to current geometry"
    );
    begin_frame(&ctx);
    assert!(
        current(&ctx).iter().all(|item| item.rect.is_none()),
        "a hidden or replaced component must not retain an old connection"
    );
}
