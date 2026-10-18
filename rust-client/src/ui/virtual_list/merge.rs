//! Temporary drag feedback and confirmation for the shared subtitle list.

use super::SubtitleMerge;
use eframe::egui;

#[derive(Clone)]
pub(super) struct MergeGesture {
    pub(super) source: egui::Id,
    pub(super) target: Option<egui::Id>,
    pub(super) grab: egui::Vec2,
    pub(super) scroll_offset: f32,
    pub(super) pending: bool,
}

impl MergeGesture {
    /// Keep only gestures whose source still exists. Pointer release either
    /// opens confirmation for an adjacent target or cancels the gesture.
    pub(super) fn update<T>(
        &mut self,
        ui: &egui::Ui,
        merge: &SubtitleMerge<'_, T>,
        rows: &[(f32, T)],
        key: fn(&T) -> egui::Id,
        visible: std::ops::Range<usize>,
        row_rect: impl Fn(usize) -> egui::Rect,
    ) -> bool {
        let source_index = rows
            .iter()
            .find(|(_, row)| key(row) == self.source)
            .and_then(|(_, row)| (merge.index)(row));
        let Some(source_index) = source_index else {
            return false;
        };
        if self.pending {
            return true;
        }

        self.target = ui
            .input(|input| input.pointer.interact_pos())
            .filter(|pointer| ui.clip_rect().contains(*pointer))
            .and_then(|pointer| {
                visible
                    .into_iter()
                    .find(|&index| {
                        (merge.index)(&rows[index].1)
                            .is_some_and(|target| source_index.abs_diff(target) == 1)
                            && row_rect(index)
                                .shrink(6.0_f32.min(rows[index].0 * 0.2))
                                .contains(pointer)
                    })
                    .map(|index| key(&rows[index].1))
            });
        if ui.input(|input| input.pointer.primary_released()) {
            self.pending = self.target.is_some();
            self.pending
        } else {
            ui.input(|input| input.pointer.primary_down())
        }
    }

    /// Transform only the floating visuals; the list keeps its measured space.
    pub(super) fn paint(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        render: impl FnOnce(&mut egui::Ui),
    ) {
        ui.set_clip_rect(ui.ctx().content_rect());
        let ready = self.target.is_some();
        let scale = if ready { 0.9 } else { 1.0 };
        ui.multiply_opacity(if ready { 0.55 } else { 0.9 });
        let pointer = ui
            .input(|input| input.pointer.interact_pos())
            .unwrap_or(rect.min + self.grab);
        let delta = pointer - self.grab - rect.min;
        let transform = egui::emath::TSTransform {
            translation: delta + rect.center().to_vec2() * (1.0 - scale),
            scaling: scale,
        };
        ui.with_visual_transform(transform, render);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
}

pub(super) fn confirm<T>(
    ctx: &egui::Context,
    id: egui::Id,
    gesture: &mut Option<MergeGesture>,
    merge: &SubtitleMerge<'_, T>,
    rows: &[(f32, T)],
    key: fn(&T) -> egui::Id,
) -> Option<(usize, usize)> {
    let active = gesture.as_ref().filter(|active| active.pending)?;
    let source = rows
        .iter()
        .find(|(_, row)| key(row) == active.source)
        .and_then(|(_, row)| (merge.index)(row));
    let target = rows
        .iter()
        .find(|(_, row)| Some(key(row)) == active.target)
        .and_then(|(_, row)| (merge.index)(row));
    let Some((source, target)) = source.zip(target).filter(|(a, b)| a.abs_diff(*b) == 1) else {
        *gesture = None;
        return None;
    };

    let tr = |text| crate::i18n::tr(merge.language, text);
    let message = format!(
        "{}\n\n{}\n\n{}",
        tr(
            "Join the original texts and translations in time order, keeping the destination subtitle's details?"
        ),
        (merge.text)(source),
        (merge.text)(target),
    );
    let accepted = crate::ui::modal::confirm(
        ctx,
        id,
        merge.language,
        tr("Merge subtitles"),
        &message,
        tr("Merge"),
        true,
    )?;
    *gesture = None;
    accepted.then_some((source, target))
}
