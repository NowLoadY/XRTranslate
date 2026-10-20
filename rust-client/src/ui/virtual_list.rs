//! Exact row measurements and visible-row rendering for variable-height lists.

use eframe::egui::{self, Vec2};
use std::{collections::HashMap, sync::Arc};

mod merge;
use merge::MergeGesture;

#[derive(Clone)]
pub struct MeasuredRow<T> {
    /// Stable identity across insertion, filtering and reordering.
    pub id: egui::Id,
    /// Content and local view state that affect this row's height.
    pub signature: egui::Id,
    pub entry: T,
}

#[derive(Clone, PartialEq)]
struct Metrics {
    width: f32,
    pixels_per_point: f32,
    style: Arc<egui::Style>,
    card_margins: egui::Vec2,
    hand_drawn: bool,
    language: crate::i18n::UiLanguage,
}

/// One exact geometry for the current view. Keep heights, never font-atlas-bound
/// galleys; only visible rows prepare text for the current frame.
#[derive(Clone)]
pub struct Measurements<T> {
    metrics: Option<Metrics>,
    revision: Option<egui::Id>,
    rows: Arc<[(f32, MeasuredRow<T>)]>,
}

impl<T> Default for Measurements<T> {
    fn default() -> Self {
        Self {
            metrics: None,
            revision: None,
            rows: Arc::from([]),
        }
    }
}

impl<T: Clone> Measurements<T> {
    pub fn invalidate(&mut self) {
        self.revision = None;
    }

    /// Rebuild entries when the document or view revision changes. Unchanged
    /// rows retain their measured height; changed display metrics remeasure all rows.
    pub fn prepare(
        &mut self,
        ui: &egui::Ui,
        revision: egui::Id,
        language: crate::i18n::UiLanguage,
        entries: impl FnOnce() -> Vec<MeasuredRow<T>>,
        measure: impl Fn(&MeasuredRow<T>) -> f32,
    ) -> Arc<[(f32, MeasuredRow<T>)]> {
        let metrics = Metrics {
            width: ui.available_width(),
            pixels_per_point: ui.ctx().pixels_per_point(),
            style: ui.style().clone(),
            card_margins: crate::ui::components::card_margins(ui.ctx()),
            hand_drawn: crate::ui::theme::is_hand_drawn(ui.ctx()),
            language,
        };
        let same_metrics = self.metrics.as_ref() == Some(&metrics);
        if !same_metrics || self.revision != Some(revision) {
            let previous = if same_metrics {
                self.rows
                    .iter()
                    .map(|(height, row)| (row.id, (row.signature, *height)))
                    .collect::<HashMap<_, _>>()
            } else {
                HashMap::new()
            };
            self.rows = entries()
                .into_iter()
                .map(|row| {
                    let height = previous
                        .get(&row.id)
                        .filter(|(signature, _)| *signature == row.signature)
                        .map_or_else(|| measure(&row), |(_, height)| *height);
                    (height, row)
                })
                .collect();
            self.metrics = Some(metrics);
            self.revision = Some(revision);
        }
        self.rows.clone()
    }
}

fn variable_row_offsets(row_heights: &[f32], row_gap: f32) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(row_heights.len() + 1);
    let mut next = 0.0;
    offsets.push(next);
    for (index, height) in row_heights.iter().enumerate() {
        next += height.max(0.0);
        if index + 1 < row_heights.len() {
            next += row_gap.max(0.0);
        }
        offsets.push(next);
    }
    offsets
}

fn visible_variable_row_range(
    viewport: egui::Rect,
    row_heights: &[f32],
    offsets: &[f32],
) -> std::ops::Range<usize> {
    if row_heights.is_empty() {
        return 0..0;
    }

    let first_visible = (0..row_heights.len())
        .find(|&index| offsets[index] + row_heights[index] >= viewport.min.y)
        .unwrap_or(row_heights.len());
    let end_visible = (first_visible..row_heights.len())
        .find(|&index| offsets[index] > viewport.max.y)
        .unwrap_or(row_heights.len());

    first_visible.saturating_sub(1)..(end_visible + 1).min(row_heights.len())
}

#[derive(Clone, Copy)]
pub enum ScrollTarget {
    None,
    End,
    Row(usize),
}
impl From<bool> for ScrollTarget {
    fn from(end: bool) -> Self {
        if end { Self::End } else { Self::None }
    }
}

#[derive(Clone)]
struct VisibleRowAnchor {
    key: egui::Id,
    row_top: f32,
    scroll_offset: f32,
}

/// Optional subtitle interaction; ordinals refer to the complete document, not
/// the filtered visual rows. Existing row keys remain the only identities.
pub struct SubtitleMerge<'a, T> {
    pub enabled: bool,
    pub language: crate::i18n::UiLanguage,
    pub index: &'a dyn Fn(&T) -> Option<usize>,
    pub text: &'a dyn Fn(usize) -> &'a str,
}

pub fn show_subtitle_rows<T, R: AsRef<[(f32, T)]>>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    row_gap: f32,
    scroll_target: impl Into<ScrollTarget>,
    row_key: fn(&T) -> egui::Id,
    merge: SubtitleMerge<'_, T>,
    prepare_rows: impl FnOnce(&egui::Ui) -> R,
    render_row: impl FnMut(&mut egui::Ui, usize, f32, &T),
) -> Option<(usize, usize)> {
    show_rows(
        ui,
        id_salt,
        row_gap,
        scroll_target.into(),
        Some(row_key),
        Some(merge),
        prepare_rows,
        render_row,
    )
}

/// Virtualizes a dynamic text list whose wrapped rows do not share one height.
///
/// Prepare exact row heights after the scrollbar has reserved its width, then
/// render only the visible rows. An optional stable row key preserves
/// the visible position when earlier rows change height while the view is at rest.
pub fn show_variable_virtual_rows<T, R: AsRef<[(f32, T)]>>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    row_gap: f32,
    scroll_target: impl Into<ScrollTarget>,
    row_key: Option<fn(&T) -> egui::Id>,
    prepare_rows: impl FnOnce(&egui::Ui) -> R,
    render_row: impl FnMut(&mut egui::Ui, usize, f32, &T),
) {
    show_rows(
        ui,
        id_salt,
        row_gap,
        scroll_target.into(),
        row_key,
        None,
        prepare_rows,
        render_row,
    );
}

fn show_rows<T, R: AsRef<[(f32, T)]>>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    row_gap: f32,
    scroll_target: ScrollTarget,
    row_key: Option<fn(&T) -> egui::Id>,
    merge: Option<SubtitleMerge<'_, T>>,
    prepare_rows: impl FnOnce(&egui::Ui) -> R,
    mut render_row: impl FnMut(&mut egui::Ui, usize, f32, &T),
) -> Option<(usize, usize)> {
    let anchor_id = ui.make_persistent_id((id_salt, "visible_row_anchor"));
    let gesture_id = ui.make_persistent_id((id_salt, "merge"));
    let mut gesture = ui.data_mut(|data| {
        let gesture = data.get_temp::<MergeGesture>(gesture_id);
        data.remove::<MergeGesture>(gesture_id);
        gesture
    });
    if !merge.as_ref().is_some_and(|merge| merge.enabled)
        || ui.input(|input| input.key_pressed(egui::Key::Escape))
    {
        if gesture.is_some() {
            ui.ctx().stop_dragging();
        }
        gesture = None;
    }
    let mut confirmed = None;
    let touch = cfg!(target_os = "android") || ui.input(|input| input.has_touch_screen());
    let grip_width = if touch && merge.as_ref().is_some_and(|merge| merge.enabled) {
        40.0
    } else {
        0.0
    };
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .auto_shrink([false, false]);
    if let Some(gesture) = &gesture {
        scroll = scroll
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
            .vertical_scroll_offset(gesture.scroll_offset);
    }
    let mut output = scroll.show_viewport(ui, |ui, viewport| {
        let content_width = ui.available_width();
        ui.set_max_width((content_width - grip_width).max(1.0));
        let prepared = prepare_rows(ui);
        ui.set_max_width(content_width);
        let rows = prepared.as_ref();
        let row_heights = rows.iter().map(|(height, _)| *height).collect::<Vec<_>>();
        let offsets = variable_row_offsets(&row_heights, row_gap);
        let content_height = offsets.last().copied().unwrap_or_default();
        let content_top = ui.max_rect().top();
        let content_left = ui.max_rect().left();
        ui.set_height(content_height);
        let row_rect = |index: usize| {
            egui::Rect::from_min_size(
                egui::pos2(content_left, content_top + offsets[index]),
                egui::vec2(content_width, row_heights[index].max(0.0)),
            )
        };
        if let (Some(merge), Some(key), Some(active)) = (&merge, row_key, &mut gesture)
            && !active.update(
                ui,
                merge,
                rows,
                key,
                visible_variable_row_range(viewport, &row_heights, &offsets),
                row_rect,
            )
        {
            gesture = None;
        }
        let scroll_target = if gesture.is_some() {
            ScrollTarget::None
        } else {
            scroll_target
        };

        if let Some(key) = row_key.filter(|_| gesture.is_none()) {
            let previous = ui.data(|data| data.get_temp::<VisibleRowAnchor>(anchor_id));
            // Let wheel input, dragging and the native scroll animation move freely.
            if matches!(scroll_target, ScrollTarget::None)
                && let Some(mut anchor) = previous
                && (anchor.scroll_offset - viewport.min.y).abs() < 0.5
                && let Some(index) = rows.iter().position(|(_, row)| key(row) == anchor.key)
            {
                let offset = (offsets[index] - anchor.row_top)
                    .clamp(0.0, (content_height - viewport.height()).max(0.0));
                let correction = offset - viewport.min.y;
                if correction.abs() > 0.5 {
                    anchor.scroll_offset = offset;
                    ui.data_mut(|data| data.insert_temp(anchor_id, anchor));
                    return Some(correction);
                }
            }
            let index = offsets
                .partition_point(|offset| *offset <= viewport.min.y)
                .saturating_sub(1)
                .min(rows.len().saturating_sub(1));
            if let Some((_, row)) = rows.get(index) {
                ui.data_mut(|data| {
                    data.insert_temp(
                        anchor_id,
                        VisibleRowAnchor {
                            key: key(row),
                            row_top: offsets[index] - viewport.min.y,
                            scroll_offset: viewport.min.y,
                        },
                    )
                });
            } else {
                ui.data_mut(|data| data.remove::<VisibleRowAnchor>(anchor_id));
            }
        }

        for index in visible_variable_row_range(viewport, &row_heights, &offsets) {
            let row_height = row_heights[index].max(0.0);
            let rect = row_rect(index);
            let row = &rows[index].1;
            let key = row_key.map(|key| key(row));
            let merge_index = merge
                .as_ref()
                .filter(|merge| merge.enabled)
                .and_then(|merge| (merge.index)(row));
            // Register below child buttons, so play/copy/edit keep their clicks.
            let drag_rect = if touch {
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - grip_width, rect.top()),
                    egui::pos2(rect.right(), (rect.top() + 44.0).min(rect.bottom())),
                )
            } else {
                rect
            };
            if merge_index.is_some() && !gesture.as_ref().is_some_and(|state| state.pending) {
                let response = ui
                    .interact(
                        drag_rect,
                        if touch {
                            key.unwrap().with("merge_grip")
                        } else {
                            key.unwrap()
                        },
                        egui::Sense::click_and_drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::Grab);
                if touch {
                    let center = egui::pos2(drag_rect.center().x, rect.top() + 22.0);
                    for x in [-3.0, 3.0] {
                        for y in [-6.0, 0.0, 6.0] {
                            ui.painter().circle_filled(
                                center + egui::vec2(x, y),
                                1.5,
                                ui.visuals().weak_text_color(),
                            );
                        }
                    }
                }
                if response.drag_started() {
                    let pointer = ui
                        .input(|input| input.pointer.press_origin())
                        .unwrap_or(rect.center());
                    gesture = Some(MergeGesture {
                        source: key.unwrap(),
                        target: None,
                        grab: pointer - rect.min,
                        scroll_offset: viewport.min.y,
                        pending: false,
                    });
                }
            }
            let dragging = gesture
                .as_ref()
                .filter(|state| !state.pending && Some(state.source) == key);
            let mut builder = egui::UiBuilder::new().id_salt((id_salt, index)).max_rect(
                egui::Rect::from_min_max(rect.min, rect.max - egui::vec2(grip_width, 0.0)),
            );
            if let Some(drag) = dragging {
                builder = builder.layer_id(egui::LayerId::new(egui::Order::Tooltip, drag.source));
            }
            ui.scope_builder(builder, |ui| {
                if let Some(drag) = dragging {
                    drag.paint(ui, rect, |ui| render_row(ui, index, row_height, row));
                } else {
                    render_row(ui, index, row_height, row);
                }
                if let Some(ordinal) = merge_index {
                    crate::ui::automation::record_output(
                        ui,
                        "Mergeable subtitle",
                        crate::ui::automation::ElementValue::Number(ordinal as f64),
                        drag_rect,
                    );
                }
            });
            if gesture
                .as_ref()
                .is_some_and(|drag| !drag.pending && drag.target == key)
            {
                ui.painter().rect_stroke(
                    rect.shrink(2.0),
                    12.0,
                    egui::Stroke::new(2.0, crate::ui::theme::primary()),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if let (Some(merge), Some(key)) = (&merge, row_key) {
            confirmed = merge::confirm(ui.ctx(), gesture_id, &mut gesture, merge, rows, key);
        }
        let scroll_target = if gesture.is_some() {
            ScrollTarget::None
        } else {
            scroll_target
        };
        let target = match scroll_target {
            ScrollTarget::None => None,
            ScrollTarget::End => Some((content_height, 0.0, egui::Align::Max)),
            ScrollTarget::Row(index) => row_heights
                .get(index)
                .map(|height| (offsets[index], *height, egui::Align::Center)),
        };
        if let Some((offset, height, align)) = target {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(
                    egui::pos2(content_left, content_top + offset),
                    Vec2::new(content_width, height),
                ),
                Some(align),
            );
        }
        None
    });
    if let Some(gesture) = gesture {
        // Existing native scroll animations also need their offset held in place.
        output.state.offset.y = gesture.scroll_offset;
        output.state.store(ui.ctx(), output.id);
        ui.data_mut(|data| data.insert_temp(gesture_id, gesture));
    }
    if let Some(correction) = output.inner {
        output.state.offset.y += correction;
        output.state.store(ui.ctx(), output.id);
        // Remeasure with the corrected native offset before presenting this frame.
        ui.ctx()
            .request_discard("Preserve the visible row after its preceding rows changed height");
    }
    confirmed
}

#[cfg(test)]
mod merge_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::i18n::UiLanguage;
    use std::cell::Cell;

    fn measured_frame(
        ctx: &egui::Context,
        measurements: &mut Measurements<usize>,
        document: &[(usize, String)],
        revision: usize,
        width: f32,
        language: UiLanguage,
    ) -> (Arc<[(f32, MeasuredRow<usize>)]>, usize, usize) {
        let built = Cell::new(0);
        let measured = Cell::new(0);
        let mut result = Arc::from([]);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                result = measurements.prepare(
                    ui,
                    egui::Id::new(revision),
                    language,
                    || {
                        built.set(built.get() + 1);
                        document
                            .iter()
                            .enumerate()
                            .map(|(index, (id, content))| MeasuredRow {
                                id: egui::Id::new(id),
                                signature: egui::Id::new(content),
                                entry: index,
                            })
                            .collect()
                    },
                    |row| {
                        measured.set(measured.get() + 1);
                        let mut content = document[row.entry].1.clone();
                        if language == UiLanguage::Russian {
                            content.push_str("\nA localized label takes another line.");
                        }
                        ui.painter()
                            .layout(
                                content,
                                egui::TextStyle::Body.resolve(ui.style()),
                                egui::Color32::WHITE,
                                ui.available_width(),
                            )
                            .size()
                            .y
                    },
                );
            },
        );
        output.textures_delta.clear();
        (result, built.get(), measured.get())
    }

    #[test]
    fn measurements_reuse_hot_geometry_and_only_measure_changed_rows() {
        let ctx = egui::Context::default();
        let mut measurements = Measurements::default();
        let mut document = (0..5000)
            .map(|id| (id, "A stable list item.".to_owned()))
            .collect::<Vec<_>>();
        let (initial, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            600.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 5000));
        for _ in 0..3 {
            let (hot, built, measured) = measured_frame(
                &ctx,
                &mut measurements,
                &document,
                0,
                600.0,
                UiLanguage::English,
            );
            assert_eq!((built, measured), (0, 0));
            assert!(Arc::ptr_eq(&initial, &hot));
        }

        document.push((5000, "A newly committed sentence.".to_owned()));
        let (appended, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            1,
            600.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 1));
        assert_eq!(appended.len(), 5001);
        assert_eq!(appended[2500].0, initial[2500].0);

        document[2500]
            .1
            .push_str("\nA translation arrived.\nA second translated line.");
        let (updated, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            2,
            600.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 1));
        assert!(updated[2500].0 > initial[2500].0);
        assert_eq!(updated[2499].0, initial[2499].0);
    }

    #[test]
    fn measurements_refresh_entries_after_reordering_and_filtering() {
        let ctx = egui::Context::default();
        let mut measurements = Measurements::default();
        let mut document = vec![
            (10, "First sentence.".to_owned()),
            (20, "Second sentence.\nIts translation.".to_owned()),
            (
                30,
                "Third sentence.\nIts translation.\nIts note.".to_owned(),
            ),
        ];
        let (initial, _, _) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            600.0,
            UiLanguage::English,
        );
        document.reverse();
        document.retain(|(id, _)| *id != 20);
        let (filtered, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            1,
            600.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 0));
        assert_eq!(filtered.len(), 2);
        for (index, old_index) in [(0, 2), (1, 0)] {
            assert_eq!(filtered[index].1.entry, index);
            assert_eq!(filtered[index].1.id, initial[old_index].1.id);
            assert_eq!(filtered[index].0, initial[old_index].0);
        }
    }

    #[test]
    fn measurements_reflow_when_width_font_or_language_changes() {
        let ctx = egui::Context::default();
        let mut measurements = Measurements::default();
        let document = vec![(
            0,
            "A longer label wraps when the available space becomes narrow.".to_owned(),
        )];
        let (wide, _, _) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            600.0,
            UiLanguage::English,
        );
        let (narrow, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            180.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 1));
        assert!(narrow[0].0 > wide[0].0);

        ctx.all_styles_mut(|style| {
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(30.0));
        });
        let (large, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            180.0,
            UiLanguage::English,
        );
        assert_eq!((built, measured), (1, 1));
        assert!(large[0].0 > narrow[0].0);

        let (localized, built, measured) = measured_frame(
            &ctx,
            &mut measurements,
            &document,
            0,
            180.0,
            UiLanguage::Russian,
        );
        assert_eq!((built, measured), (1, 1));
        assert_ne!(localized[0].0, large[0].0);
        let (fresh, _, _) = measured_frame(
            &ctx,
            &mut Measurements::default(),
            &document,
            0,
            180.0,
            UiLanguage::Russian,
        );
        assert_eq!(localized[0].0, fresh[0].0);
    }

    fn subtitle_frame(
        ctx: &egui::Context,
        time: f64,
        translated: bool,
        target: ScrollTarget,
        wheel: bool,
    ) -> (f32, usize, f32, usize) {
        let mut first = None;
        let mut id = egui::Id::NULL;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500.0, 400.0),
            )),
            time: Some(time),
            events: if wheel {
                vec![
                    egui::Event::PointerMoved(egui::pos2(250.0, 200.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -120.0),
                        modifiers: Default::default(),
                        phase: egui::TouchPhase::Move,
                    },
                ]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            first = None;
            id = ui.make_persistent_id(egui::IdSalt::new("subtitles"));
            show_variable_virtual_rows(
                ui,
                "subtitles",
                8.0,
                target,
                Some(|index: &usize| egui::Id::new(index)),
                |_| {
                    (0..50)
                        .map(|index| (if translated && index < 20 { 80.0 } else { 40.0 }, index))
                        .collect::<Vec<_>>()
                },
                |ui, index, _, _| {
                    if first.is_none() && ui.max_rect().bottom() >= ui.clip_rect().top() {
                        first = Some((index, ui.max_rect().top()));
                    }
                    ui.label(format!("Subtitle {index}"));
                },
            );
        });
        output.textures_delta.clear();
        let (index, top) = first.expect("The final pass renders visible subtitles");
        (
            egui::scroll_area::State::load(ctx, id).unwrap().offset.y,
            index,
            top,
            output.platform_output.num_completed_passes,
        )
    }

    #[test]
    fn subtitle_height_changes_preserve_position_without_blocking_scrolls() {
        let ctx = egui::Context::default();
        let mut before = (0.0, 0, 0.0, 0);
        for frame in 0..26 {
            before = subtitle_frame(
                &ctx,
                frame as f64 / 60.0,
                false,
                if frame == 1 {
                    ScrollTarget::Row(30)
                } else {
                    ScrollTarget::None
                },
                false,
            );
        }
        let after = subtitle_frame(&ctx, 1.0, true, ScrollTarget::None, false);
        assert_eq!(before.1, after.1);
        assert!((before.2 - after.2).abs() < 0.01);
        assert_eq!(after.0 - before.0, 800.0);
        assert_eq!(after.3, 2);
        let stable = subtitle_frame(&ctx, 1.01, true, ScrollTarget::None, false);
        assert_eq!(stable.3, 1);

        let mut manual = stable;
        for frame in 0..25 {
            manual = subtitle_frame(
                &ctx,
                1.1 + frame as f64 / 60.0,
                true,
                ScrollTarget::None,
                frame == 0,
            );
        }
        assert!(manual.0 > stable.0 + 100.0);

        let mut jump = Vec::new();
        for frame in 0..30 {
            jump.push(
                subtitle_frame(
                    &ctx,
                    2.0 + frame as f64 / 60.0,
                    true,
                    if frame == 0 {
                        ScrollTarget::Row(5)
                    } else {
                        ScrollTarget::None
                    },
                    false,
                )
                .0,
            );
        }
        assert!(jump[2] < jump[0] && jump[2] > *jump.last().unwrap());
        assert!((jump.last().unwrap() - 280.0).abs() < 2.0);
    }
}
