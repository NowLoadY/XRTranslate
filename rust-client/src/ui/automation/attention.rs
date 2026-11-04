//! Transient visual feedback, shared by controls and plugin document rows.
use eframe::egui::{self, Id, Pos2, Rect};

#[derive(Clone)]
struct Focus {
    target: Id,
    until: f64,
    reveal: bool,
    rect: Option<Rect>,
}

fn key() -> Id {
    Id::unique("director_attention")
}

pub fn focus(target: Id) {
    if let Some(ctx) = super::driver().context() {
        request(ctx, target);
    }
}

pub(super) fn request(ctx: &egui::Context, target: Id) {
    let until = ctx.input(|input| input.time) + 4.0;
    ctx.data_mut(|data| {
        data.insert_temp(
            key(),
            Focus {
                target,
                until,
                reveal: true,
                rect: None,
            },
        );
    });
    ctx.request_repaint();
}

pub(super) fn begin_frame(ctx: &egui::Context) {
    let now = ctx.input(|input| input.time);
    ctx.data_mut(|data| {
        if let Some(mut focus) = data.get_temp::<Focus>(key()) {
            if now >= focus.until {
                data.remove::<Focus>(key());
            } else {
                // Never point at a stale rectangle after navigation or scrolling.
                focus.rect = None;
                data.insert_temp(key(), focus);
            }
        }
    });
}

pub fn reveal_target(ctx: &egui::Context) -> Option<Id> {
    ctx.data(|data| data.get_temp::<Focus>(key()))
        .filter(|focus| focus.reveal)
        .map(|focus| focus.target)
}

/// Called with current geometry; works inside nested scroll areas as well.
pub fn record(ui: &egui::Ui, target: Id, rect: Rect) {
    let scroll = ui.ctx().data_mut(|data| {
        let Some(mut focus) = data
            .get_temp::<Focus>(key())
            .filter(|focus| focus.target == target)
        else {
            return false;
        };
        let visible = rect.intersect(ui.clip_rect());
        focus.rect = visible.is_positive().then_some(visible);
        let scroll = focus.reveal && !ui.clip_rect().contains(rect.center());
        if !scroll {
            focus.reveal = false;
        }
        data.insert_temp(key(), focus);
        scroll
    });
    if scroll {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
    }
}

pub fn paint(painter: &egui::Painter, head: Pos2, radius: f32) {
    let ctx = painter.ctx();
    let Some(focus) = ctx.data(|data| data.get_temp::<Focus>(key())) else {
        return;
    };
    let remaining = focus.until - ctx.input(|input| input.time);
    if remaining <= 0.0 {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
    let Some(rect) = focus.rect else { return };
    let color =
        super::super::theme::primary().gamma_multiply((remaining as f32 / 0.5).min(1.0) * 0.7);
    let end = rect.clamp(head);
    let direction = end - head;
    if direction.length() > radius {
        painter.extend(egui::Shape::dashed_line(
            &[head + direction.normalized() * radius, end],
            egui::Stroke::new(1.5, color),
            5.0,
            4.0,
        ));
    }
    painter.rect_stroke(
        rect.expand(2.0),
        5.0,
        egui::Stroke::new(1.5, color),
        egui::StrokeKind::Outside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
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
        for frame in 0..120 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 450.0))),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                },
                |ui| {
                    begin_frame(&ctx);
                    if frame == 0 {
                        request(&ctx, target);
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
                    visible = ctx
                        .data(|data| data.get_temp::<Focus>(key()))
                        .is_some_and(|focus| {
                            !focus.reveal
                                && focus
                                    .rect
                                    .is_some_and(|rect| ctx.content_rect().contains_rect(rect))
                        });
                    paint(
                        &ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, key())),
                        egui::pos2(30.0, 30.0),
                        20.0,
                    );
                },
            );
            connected |= output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::LineSegment { stroke, .. } if stroke.width == 1.5));
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
            ctx.data(|data| data.get_temp::<Focus>(key()))
                .unwrap()
                .rect
                .is_none(),
            "a hidden or replaced component must not retain an old connection"
        );
    }
}
