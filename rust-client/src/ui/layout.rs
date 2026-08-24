use eframe::egui::{self, Id, Vec2};

pub const BASE_MIN_INNER_SIZE: Vec2 = egui::vec2(880.0, 600.0);
const MONITOR_MARGIN: Vec2 = egui::vec2(48.0, 80.0);
const SIZE_EPSILON: f32 = 0.75;

fn requirements_id() -> Id {
    Id::new("xrtranslate_layout_requirements")
}

fn resize_state_id() -> Id {
    Id::new("xrtranslate_window_resize_state")
}

#[derive(Clone, Copy, Debug)]
struct LayoutRequirements {
    min_inner_size: Vec2,
}

#[derive(Clone, Copy, Debug)]
struct WindowResizeState {
    start_size: Vec2,
    target_size: Vec2,
    applied_min_size: Vec2,
    start_time: f64,
    active: bool,
}

impl Default for WindowResizeState {
    fn default() -> Self {
        Self {
            start_size: BASE_MIN_INNER_SIZE,
            target_size: BASE_MIN_INNER_SIZE,
            applied_min_size: BASE_MIN_INNER_SIZE,
            start_time: 0.0,
            active: false,
        }
    }
}

pub fn begin_frame(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.insert_temp(
            requirements_id(),
            LayoutRequirements {
                min_inner_size: BASE_MIN_INNER_SIZE,
            },
        );
    });
}

/// Reports a genuinely unbreakable content size to the root window.
///
/// Responsive containers should wrap or stack first. Call this only for the
/// minimum size below which an individual control can no longer remain usable.
pub fn require_content_size(ui: &egui::Ui, minimum: Vec2) {
    let Some(current) = current_inner_size(ui.ctx()) else {
        return;
    };
    let available = ui.available_size();
    let missing = (minimum - available).max(Vec2::ZERO);
    require_inner_size(ui.ctx(), current + missing);
}

pub fn require_content_width(ui: &egui::Ui, minimum_width: f32) {
    let Some(current) = current_inner_size(ui.ctx()) else {
        return;
    };
    let missing_width = (minimum_width - ui.available_width()).max(0.0);
    require_inner_size(ui.ctx(), current + egui::vec2(missing_width, 0.0));
}

fn require_inner_size(ctx: &egui::Context, minimum: Vec2) {
    ctx.data_mut(|data| {
        let requirements =
            data.get_temp_mut_or_insert_with(requirements_id(), || LayoutRequirements {
                min_inner_size: BASE_MIN_INNER_SIZE,
            });
        requirements.min_inner_size = requirements.min_inner_size.max(minimum);
    });
}

/// A common flow container for controls and short data items.
pub fn flow_row<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal_wrapped(add_contents).inner
}

/// Keeps a related control group together when the current flow line is too
/// short, while still allowing the group to wrap internally on narrow screens.
pub fn flow_group<R>(
    ui: &mut egui::Ui,
    preferred_min_width: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    if should_start_new_flow_line(
        ui.available_width(),
        ui.max_rect().width(),
        preferred_min_width,
    ) {
        ui.end_row();
    }
    flow_row(ui, add_contents)
}

fn should_start_new_flow_line(remaining: f32, line_width: f32, group_min_width: f32) -> bool {
    remaining + SIZE_EPSILON < group_min_width && line_width + SIZE_EPSILON >= group_min_width
}

/// Constrains a page to the current content width and reports any remaining
/// horizontal overflow to the root coordinator.
pub fn contain_width<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let available = ui.available_rect_before_wrap();
    let response = ui.scope(|ui| {
        ui.set_max_width(available.width());
        add_contents(ui)
    });
    let overflow = horizontal_overflow(response.response.rect, available);
    if overflow > SIZE_EPSILON {
        require_content_width(ui, available.width() + overflow);
    }
    response.inner
}

pub fn should_stack(available_width: f32, column_count: usize, min_column_width: f32) -> bool {
    if column_count <= 1 {
        return false;
    }
    let gaps = (column_count - 1) as f32 * 8.0;
    available_width + SIZE_EPSILON < column_count as f32 * min_column_width + gaps
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

/// Virtualizes a dynamic text list whose wrapped rows do not share one height.
///
/// Callers measure rows against the current content width before invoking this
/// function. Keeping the geometry here ensures scrolling, clipping, and
/// scroll-to-end all use the same measured layout.
pub fn show_variable_virtual_rows(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    row_heights: &[f32],
    row_gap: f32,
    scroll_to_end: bool,
    mut render_row: impl FnMut(&mut egui::Ui, usize, f32),
) {
    let offsets = variable_row_offsets(row_heights, row_gap);
    let content_height = offsets.last().copied().unwrap_or_default();
    let viewport_height_id = ui.make_persistent_id((id_salt, "viewport_height"));
    let viewport_height = ui
        .memory(|memory| memory.data.get_temp::<f32>(viewport_height_id))
        .unwrap_or_else(|| ui.available_height())
        .max(0.0);
    let mut scroll_area = egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .animated(false)
        .auto_shrink([false, false]);
    if scroll_to_end {
        scroll_area =
            scroll_area.vertical_scroll_offset((content_height - viewport_height).max(0.0));
    }

    let output = scroll_area.show_viewport(ui, |ui, viewport| {
        let content_top = ui.max_rect().top();
        let content_left = ui.max_rect().left();
        let content_width = ui.available_width();
        ui.set_height(content_height);

        for index in visible_variable_row_range(viewport, row_heights, &offsets) {
            let row_height = row_heights[index].max(0.0);
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(content_left, content_top + offsets[index]),
                egui::vec2(content_width, row_height),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt((id_salt, index))
                    .max_rect(row_rect),
                |ui| render_row(ui, index, row_height),
            );
        }
    });
    ui.memory_mut(|memory| {
        memory
            .data
            .insert_temp(viewport_height_id, output.inner_rect.height());
    });
}

/// Sizes a single-line control from its rendered label while respecting the
/// current container. Long labels are clipped by the widget instead of pushing
/// neighboring controls outside the window.
pub fn control_width(
    ui: &egui::Ui,
    text: &str,
    requested_width: Option<f32>,
    min_width: f32,
    max_width: f32,
) -> f32 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let measured = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id, egui::Color32::WHITE)
        .size()
        .x
        + 38.0;
    let preferred = requested_width
        .unwrap_or(measured)
        .clamp(min_width, max_width);
    let line_width = ui.max_rect().width().max(0.0);
    if line_width + SIZE_EPSILON < min_width {
        require_content_width(ui, min_width);
    }
    preferred.min(line_width.max(min_width))
}

pub fn finish_frame(ctx: &egui::Context) {
    let Some(current_size) = current_inner_size(ctx) else {
        return;
    };
    let viewport = ctx.input(|input| input.viewport().clone());
    let required = ctx
        .data(|data| data.get_temp::<LayoutRequirements>(requirements_id()))
        .map(|requirements| requirements.min_inner_size)
        .unwrap_or(BASE_MIN_INNER_SIZE);
    let required = constrain_to_monitor(required, viewport.monitor_size);

    if viewport.maximized == Some(true) || viewport.fullscreen == Some(true) {
        return;
    }

    let now = ctx.input(|input| input.time);
    let mut state = ctx
        .data(|data| data.get_temp::<WindowResizeState>(resize_state_id()))
        .unwrap_or_default();
    let needs_growth =
        current_size.x + SIZE_EPSILON < required.x || current_size.y + SIZE_EPSILON < required.y;

    if needs_growth
        && (!state.active
            || (state.target_size - required).length_sq() > SIZE_EPSILON * SIZE_EPSILON)
    {
        state.start_size = current_size;
        state.target_size = current_size.max(required);
        state.start_time = now;
        state.active = true;
    }

    if state.active {
        let duration = crate::ui::theme::animation_timings(ctx).window_resize;
        let progress = ((now - state.start_time) as f32 / duration).clamp(0.0, 1.0);
        let eased = crate::ui::animation::AnimationSystem::ease_out_cubic(progress);
        let next_size = state.start_size + (state.target_size - state.start_size) * eased;
        let animated_min_size = next_size.min(required);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(animated_min_size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(next_size));
        state.applied_min_size = animated_min_size;
        ctx.request_repaint();
        state.active = progress < 1.0;
    } else if (state.applied_min_size - required).length_sq() > SIZE_EPSILON * SIZE_EPSILON {
        // Lowering this constraint never shrinks the user's window. It only
        // restores the range in which manual resizing is allowed.
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(required));
        state.applied_min_size = required;
    }

    ctx.data_mut(|data| data.insert_temp(resize_state_id(), state));
}

fn current_inner_size(ctx: &egui::Context) -> Option<Vec2> {
    ctx.input(|input| input.viewport().inner_rect.map(|rect| rect.size()))
}

fn constrain_to_monitor(required: Vec2, monitor_size: Option<Vec2>) -> Vec2 {
    monitor_size
        .map(|monitor| required.min((monitor - MONITOR_MARGIN).max(egui::vec2(320.0, 240.0))))
        .unwrap_or(required)
}

fn horizontal_overflow(content: egui::Rect, available: egui::Rect) -> f32 {
    (content.max.x - available.max.x).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacking_uses_available_width_and_minimum_column_width() {
        assert!(!should_stack(628.0, 2, 300.0));
        assert!(should_stack(607.0, 2, 300.0));
        assert!(!should_stack(200.0, 1, 300.0));
    }

    #[test]
    fn grouped_controls_move_only_when_a_fresh_line_can_fit_them() {
        assert!(should_start_new_flow_line(80.0, 600.0, 240.0));
        assert!(!should_start_new_flow_line(300.0, 600.0, 240.0));
        assert!(!should_start_new_flow_line(80.0, 180.0, 240.0));
    }

    #[test]
    fn required_window_size_stays_inside_the_monitor() {
        assert_eq!(
            constrain_to_monitor(egui::vec2(2100.0, 1300.0), Some(egui::vec2(1920.0, 1080.0))),
            egui::vec2(1872.0, 1000.0)
        );
        assert_eq!(
            constrain_to_monitor(BASE_MIN_INNER_SIZE, Some(egui::vec2(800.0, 600.0))),
            egui::vec2(752.0, 520.0)
        );
    }

    #[test]
    fn overflow_is_reported_only_past_the_available_right_edge() {
        let available = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        assert_eq!(horizontal_overflow(available, available), 0.0);
        assert_eq!(
            horizontal_overflow(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(427.0, 100.0)),
                available,
            ),
            27.0
        );
    }

    #[test]
    fn variable_rows_have_no_trailing_gap() {
        assert_eq!(variable_row_offsets(&[], 8.0), vec![0.0]);
        assert_eq!(variable_row_offsets(&[88.0], 8.0), vec![0.0, 88.0]);
        assert_eq!(
            variable_row_offsets(&[88.0, 120.0, 96.0], 8.0),
            vec![0.0, 96.0, 224.0, 320.0]
        );
    }

    #[test]
    fn variable_row_visibility_is_bounded_near_the_end() {
        let heights = [88.0, 120.0, 96.0, 160.0, 88.0];
        let offsets = variable_row_offsets(&heights, 8.0);
        let content_height = *offsets.last().unwrap();
        let viewport = egui::Rect::from_min_max(
            egui::pos2(0.0, content_height - 180.0),
            egui::pos2(500.0, content_height),
        );
        let range = visible_variable_row_range(viewport, &heights, &offsets);

        assert!(range.start < range.end);
        assert_eq!(range.end, heights.len());
    }
}
