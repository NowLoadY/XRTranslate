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

pub fn responsive_columns(
    ui: &mut egui::Ui,
    count: usize,
    min_width: f32,
    mut render: impl FnMut(&mut egui::Ui, usize),
) {
    if should_stack(ui.available_width(), count, min_width) {
        for index in 0..count {
            ui.push_id(index, |ui| render(ui, index));
            if index + 1 < count {
                ui.add_space(12.0);
            }
        }
    } else {
        ui.columns(count, |columns| {
            for (index, column) in columns.iter_mut().enumerate() {
                render(column, index);
            }
        });
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

/// Virtualizes a dynamic text list whose wrapped rows do not share one height.
///
/// Prepare text after the scrollbar has reserved its width, then reuse that
/// layout for measurement and rendering.
pub fn show_variable_virtual_rows<T, R: AsRef<[(f32, T)]>>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    row_gap: f32,
    scroll_target: impl Into<ScrollTarget>,
    prepare_rows: impl FnOnce(&egui::Ui) -> R,
    mut render_row: impl FnMut(&mut egui::Ui, usize, f32, &T),
) {
    let scroll_target = scroll_target.into();
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .animated(false)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let prepared = prepare_rows(ui);
            let rows = prepared.as_ref();
            let row_heights = rows.iter().map(|(height, _)| *height).collect::<Vec<_>>();
            let offsets = variable_row_offsets(&row_heights, row_gap);
            let content_height = offsets.last().copied().unwrap_or_default();
            let content_top = ui.max_rect().top();
            let content_left = ui.max_rect().left();
            let content_width = ui.available_width();
            ui.set_height(content_height);

            for index in visible_variable_row_range(viewport, &row_heights, &offsets) {
                let row_height = row_heights[index].max(0.0);
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_left, content_top + offsets[index]),
                    egui::vec2(content_width, row_height),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt((id_salt, index))
                        .max_rect(row_rect),
                    |ui| render_row(ui, index, row_height, &rows[index].1),
                );
            }
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
    if cfg!(target_os = "android") {
        return;
    }
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
