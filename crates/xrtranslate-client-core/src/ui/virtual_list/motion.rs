//! Interpolate measured geometry only for agent edits. No document copies or extra rows.
use eframe::egui;
use std::collections::HashMap;

const DURATION: f64 = 0.3;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Row {
    pub id: egui::Id,
    pub top: f32,
    pub height: f32,
}

#[derive(Clone, Default)]
struct Layout {
    width: f32,
    target: Vec<Row>,
    from: Vec<Row>,
    started: f64,
}

impl Layout {
    fn sample(&self, now: f64) -> Vec<Row> {
        let t = super::super::animation::AnimationSystem::ease_out_cubic(
            ((now - self.started) / DURATION) as f32,
        );
        self.target
            .iter()
            .zip(&self.from)
            .map(|(to, from)| Row {
                id: to.id,
                top: egui::lerp(from.top..=to.top, t),
                height: egui::lerp(from.height..=to.height, t),
            })
            .collect()
    }

    fn update(&mut self, width: f32, target: Vec<Row>, now: f64, animate: bool) -> Vec<Row> {
        if self.target != target || self.width != width {
            let previous = self
                .sample(now)
                .into_iter()
                .map(|row| (row.id, row))
                .collect::<HashMap<_, _>>();
            let animate = animate && self.width == width && !previous.is_empty();
            self.from = target.clone();
            if animate {
                // New rows unfold between the previous positions of their
                // surviving neighbors. Deleted space closes as those neighbors move.
                let mut next_top = previous
                    .values()
                    .map(|row| row.top + row.height)
                    .fold(0.0, f32::max);
                for row in self.from.iter_mut().rev() {
                    if let Some(old) = previous.get(&row.id) {
                        *row = *old;
                        next_top = old.top;
                    } else {
                        row.top = row.top.min(next_top);
                        row.height = 0.0;
                    }
                }
                let mut previous_end: f32 = 0.0;
                for row in &mut self.from {
                    row.top = row.top.max(previous_end);
                    previous_end = row.top + row.height;
                }
            }
            self.width = width;
            self.target = target;
            self.started = if animate { now } else { now - DURATION };
        }
        self.sample(now)
    }
}

pub(super) fn layout(
    ui: &egui::Ui,
    id: egui::Id,
    width: f32,
    rows: Vec<Row>,
    allow: bool,
) -> Vec<Row> {
    let now = ui.input(|input| input.time);
    let animate = allow
        && ui_director::egui::feedback::current(ui.ctx())
            .iter()
            .any(|item| {
                matches!(
                    item.feedback.effect,
                    ui_director::feedback::Effect::Change { .. }
                )
            });
    let mut state = ui
        .data_mut(|data| data.remove_temp::<Layout>(id))
        .unwrap_or_default();
    let rows = state.update(width, rows, now, animate);
    if now < state.started + DURATION {
        ui.ctx().request_repaint();
    }
    ui.data_mut(|data| data.insert_temp(id, state));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, top: f32, height: f32) -> Row {
        Row {
            id: egui::Id::new(id),
            top,
            height,
        }
    }

    #[test]
    fn split_and_merge_move_existing_rows_and_unfold_new_ones_without_overlap() {
        let mut layout = Layout::default();
        let initial = vec![
            row(1, 0.0, 100.0),
            row(2, 108.0, 100.0),
            row(3, 216.0, 100.0),
        ];
        layout.update(400.0, initial, 0.0, false);
        let split = vec![
            row(4, 0.0, 80.0),
            row(5, 88.0, 80.0),
            row(2, 176.0, 100.0),
            row(3, 284.0, 100.0),
        ];
        let first = layout.update(400.0, split.clone(), 1.0, true);
        assert_eq!(first[0].height, 0.0);
        assert_eq!(first[2].top, 108.0);
        for step in 1..=4 {
            let rows = layout.sample(1.0 + step as f64 * 0.1);
            assert!(
                rows.windows(2)
                    .all(|pair| pair[0].top + pair[0].height <= pair[1].top)
            );
        }
        assert!(layout.sample(1.4) == split);
        let merged = vec![
            row(4, 0.0, 120.0),
            row(2, 128.0, 100.0),
            row(3, 236.0, 100.0),
        ];
        let first = layout.update(400.0, merged.clone(), 2.0, true);
        assert_eq!(first[1].top, 176.0);
        assert!(layout.sample(2.2)[1].top > 128.0);
        assert!(layout.sample(2.4) == merged);
        let resized = vec![row(4, 0.0, 150.0)];
        assert!(layout.update(300.0, resized.clone(), 3.0, true) == resized);
    }
}
