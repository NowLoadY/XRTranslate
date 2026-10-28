use crate::ui::virtual_list::ScrollTarget;
use eframe::egui;
use std::sync::Arc;

pub(super) fn text(ui: &egui::Ui, text: egui::RichText, width: f32) -> Arc<egui::Galley> {
    egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        width.max(1.0),
        egui::TextStyle::Body,
    )
}

pub(super) fn scroll_target(
    ui: &egui::Ui,
    scroll_id: egui::Id,
    geometry_id: egui::Id,
    live: bool,
    evidence: Option<usize>,
) -> ScrollTarget {
    let reading = ui.rect_contains_pointer(ui.max_rect())
        && ui.input(|input| {
            input.smooth_scroll_delta.y.abs() > 0.1
                || input.pointer.any_down()
                || input.pointer.is_decidedly_dragging()
        });
    let geometry = ui.data(|data| data.get_temp::<(f32, f32)>(geometry_id));
    let offset = egui::scroll_area::State::load(ui.ctx(), scroll_id)
        .map_or(0.0, |state| state.clamped_offset().y);
    let at_bottom =
        geometry.is_none_or(|(content, viewport)| offset >= (content - viewport).max(0.0) - 1.0);
    let follow_id = scroll_id.with("follow_latest");
    let (was_following, was_at_bottom) = ui
        .data(|data| data.get_temp::<(bool, bool)>(follow_id))
        .unwrap_or((false, false));
    let following =
        !reading && evidence.is_none() && (was_following || (at_bottom && !was_at_bottom));
    // Keep following during the native scroll animation; new rows can arrive
    // before it reaches the previous bottom. Resume only on reaching the bottom,
    // so an evidence jump cannot be undone before its first animation frame.
    ui.data_mut(|data| {
        data.insert_temp(
            follow_id,
            (following, (at_bottom && !reading) || evidence.is_some()),
        )
    });
    if let Some(index) = evidence {
        ScrollTarget::Row(index)
    } else if live && following {
        ScrollTarget::End
    } else {
        ScrollTarget::None
    }
}

pub(super) fn remember_geometry<T>(
    ui: &egui::Ui,
    id: egui::Id,
    rows: &[(f32, T)],
    gap: f32,
    viewport_height: f32,
) {
    let height = rows.iter().map(|(height, _)| height).sum::<f32>()
        + gap * rows.len().saturating_sub(1) as f32;
    ui.data_mut(|data| data.insert_temp(id, (height, viewport_height)));
}

pub(super) fn record(ui: &egui::Ui, id: &str, rect: egui::Rect) {
    crate::ui::automation::record_output(
        ui,
        "Meeting segment",
        crate::ui::automation::ElementValue::Text(id.to_owned()),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        time: f64,
        count: usize,
        wheel: f32,
        evidence: Option<usize>,
    ) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            time: Some(time),
            events: if wheel != 0.0 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(300.0, 150.0)),
                    egui::Event::MouseWheel {
                        source: egui::MouseWheelSource::Wheel,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, wheel),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let mut id = egui::Id::NULL;
        let mut output = ctx.run_ui(input, |ui| {
            id = ui.make_persistent_id(egui::IdSalt::new("meeting_follow_test"));
            let geometry = id.with("geometry");
            let target = scroll_target(ui, id, geometry, true, evidence);
            let height = ui.available_height();
            crate::ui::virtual_list::show_variable_virtual_rows(
                ui,
                "meeting_follow_test",
                6.0,
                target,
                Some(|index: &usize| egui::Id::unique(index)),
                |ui| {
                    let rows = (0..count).map(|index| (80.0, index)).collect::<Vec<_>>();
                    remember_geometry(ui, geometry, &rows, 6.0, height);
                    rows
                },
                |ui, index, _, _| {
                    ui.label(format!("Meeting sentence {index}"));
                },
            );
        });
        output.textures_delta.clear();
        egui::scroll_area::State::load(ctx, id)
            .unwrap()
            .clamped_offset()
            .y
    }

    #[test]
    fn following_survives_continuous_appends_until_user_navigates() {
        let ctx = egui::Context::default();
        let mut bottom = 0.0;
        for index in 0..70 {
            bottom = frame(
                &ctx,
                index as f64 / 60.0,
                3 + (index / 3).min(10),
                0.0,
                None,
            );
        }
        assert!((bottom - 812.0).abs() < 2.0);

        let mut reading = bottom;
        for index in 0..30 {
            reading = frame(
                &ctx,
                2.0 + index as f64 / 60.0,
                13,
                if index == 0 { 140.0 } else { 0.0 },
                None,
            );
        }
        assert!(reading < bottom - 100.0);
        let mut after_append = reading;
        for index in 0..30 {
            after_append = frame(&ctx, 3.0 + index as f64 / 60.0, 15, 0.0, None);
        }
        assert!((after_append - reading).abs() < 0.5);

        let mut jumped = reading;
        for index in 0..30 {
            jumped = frame(
                &ctx,
                4.0 + index as f64 / 60.0,
                15,
                0.0,
                (index == 0).then_some(2),
            );
        }
        assert!(jumped < 150.0);
        assert!((frame(&ctx, 5.0, 18, 0.0, None) - jumped).abs() < 0.5);

        for index in 0..40 {
            frame(
                &ctx,
                6.0 + index as f64 / 60.0,
                18,
                if index == 0 { -10000.0 } else { 0.0 },
                None,
            );
        }
        for index in 0..40 {
            bottom = frame(&ctx, 7.0 + index as f64 / 60.0, 19, 0.0, None);
        }
        assert!((bottom - 1328.0).abs() < 2.0);
    }

    #[test]
    fn evidence_navigation_from_bottom_stops_following() {
        let ctx = egui::Context::default();
        for index in 0..40 {
            frame(&ctx, index as f64 / 60.0, 13, 0.0, None);
        }
        let mut jumped = 0.0;
        for index in 0..30 {
            jumped = frame(
                &ctx,
                1.0 + index as f64 / 60.0,
                13,
                0.0,
                (index == 0).then_some(2),
            );
        }
        assert!(
            jumped < 150.0,
            "Evidence target should remain visible: {jumped}"
        );
        assert!((frame(&ctx, 2.0, 16, 0.0, None) - jumped).abs() < 0.5);
    }
}
