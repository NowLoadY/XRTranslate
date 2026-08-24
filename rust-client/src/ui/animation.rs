use eframe::egui::{self, Color32, Id};

pub struct AnimationSystem;

impl AnimationSystem {
    pub fn ease_out_cubic(t: f32) -> f32 {
        let p = (1.0 - t.clamp(0.0, 1.0)).max(0.0);
        1.0 - p * p * p
    }

    pub fn animate_value(ctx: &egui::Context, id: Id, target_val: f32, duration: f32) -> f32 {
        let current = ctx.animate_value_with_time(id, target_val, duration);
        if (current - target_val).abs() > 0.001 {
            ctx.request_repaint();
        }
        current
    }

    pub fn animate_bool(ctx: &egui::Context, id: Id, active: bool, duration: f32) -> f32 {
        let target = if active { 1.0 } else { 0.0 };
        Self::animate_value(ctx, id, target, duration)
    }

    pub fn hover(ctx: &egui::Context, id: Id, active: bool) -> f32 {
        Self::animate_bool(
            ctx,
            id,
            active,
            crate::ui::theme::animation_timings(ctx).hover,
        )
    }

    pub fn active(ctx: &egui::Context, id: Id, active: bool) -> f32 {
        Self::animate_bool(
            ctx,
            id,
            active,
            crate::ui::theme::animation_timings(ctx).active,
        )
    }

    pub fn selection(ctx: &egui::Context, id: Id, active: bool) -> f32 {
        Self::animate_bool(
            ctx,
            id,
            active,
            crate::ui::theme::animation_timings(ctx).selection,
        )
    }

    pub fn toggle(ctx: &egui::Context, id: Id, active: bool) -> f32 {
        Self::animate_bool(
            ctx,
            id,
            active,
            crate::ui::theme::animation_timings(ctx).toggle,
        )
    }

    pub fn button_click_duration(ctx: &egui::Context) -> f32 {
        crate::ui::theme::animation_timings(ctx).button_click
    }

    pub fn primary_click_duration(ctx: &egui::Context) -> f32 {
        crate::ui::theme::animation_timings(ctx).primary_click
    }

    /// Renders changing data with a shared freshness-to-opacity mapping.
    ///
    /// The activity value is semantic rather than visual: callers provide how
    /// current or active the data is, while the theme owns the presentation.
    pub fn render_data_text<R>(
        ui: &mut egui::Ui,
        id: Id,
        activity: f32,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let motion = crate::ui::theme::data_text_motion(ui.ctx());
        let timings = crate::ui::theme::animation_timings(ui.ctx());
        let target_opacity = crate::ui::theme::data_text_target_opacity(ui.ctx(), activity);
        let opacity = Self::animate_value(
            ui.ctx(),
            id.with("opacity"),
            target_opacity,
            timings.data_text,
        );
        let target_offset = (1.0 - activity.clamp(0.0, 1.0)) * motion.max_offset;
        let offset = Self::animate_value(
            ui.ctx(),
            id.with("offset"),
            target_offset,
            timings.data_text,
        );

        ui.scope(|ui| {
            ui.set_opacity(opacity);
            if offset > 0.1 {
                ui.add_space(offset);
            }
            add_contents(ui)
        })
        .inner
    }

    #[allow(dead_code)]
    pub fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
        let factor = t.clamp(0.0, 1.0);
        from + (to - from) * factor
    }

    pub fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
        let factor = t.clamp(0.0, 1.0);
        Color32::from_rgba_premultiplied(
            (from.r() as f32 + (to.r() as f32 - from.r() as f32) * factor) as u8,
            (from.g() as f32 + (to.g() as f32 - from.g() as f32) * factor) as u8,
            (from.b() as f32 + (to.b() as f32 - from.b() as f32) * factor) as u8,
            (from.a() as f32 + (to.a() as f32 - from.a() as f32) * factor) as u8,
        )
    }

    pub fn render_animated_page<P, R>(
        ui: &mut egui::Ui,
        page_id: P,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R
    where
        P: std::hash::Hash + std::fmt::Debug,
    {
        let current_time = ui.ctx().input(|i| i.time);
        let global_id = Id::new("page_transition_state").with(std::any::type_name::<P>());

        let target_hash = Id::new(&page_id).value();

        let start_time = ui.ctx().memory_mut(|m| {
            let state = m
                .data
                .get_temp_mut_or_insert_with(global_id, || (target_hash, current_time));
            if state.0 != target_hash {
                state.0 = target_hash;
                state.1 = current_time;
            }
            state.1
        });

        let elapsed = (current_time - start_time) as f32;
        let duration = crate::ui::theme::animation_timings(ui.ctx()).page;
        let raw_t = (elapsed / duration).clamp(0.0, 1.0);

        if raw_t < 1.0 {
            ui.ctx().request_repaint();
        }

        let eased = Self::ease_out_cubic(raw_t);

        let y_offset = (1.0 - eased) * 12.0;

        ui.scope(|ui| {
            if y_offset > 0.1 {
                ui.add_space(y_offset);
            }
            if eased < 0.999 {
                ui.set_opacity(eased);
            }
            crate::ui::layout::contain_width(ui, add_contents)
        })
        .inner
    }

    /// Renders a directional page-flip / slide transition for multi-step wizards.
    /// Pages moving forward (next) glide smoothly in from the right; pages moving backward (back) glide in from the left.
    pub fn render_page_flip_transition<R>(
        ui: &mut egui::Ui,
        page_index: usize,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let current_time = ui.ctx().input(|i| i.time);
        let global_id = Id::new("onboarding_page_flip_transition_state");

        let (direction, start_time) = ui.ctx().memory_mut(|m| {
            let state = m
                .data
                .get_temp_mut_or_insert_with(global_id, || (page_index, current_time, 0.0f32));
            if state.0 != page_index {
                let dir = if page_index > state.0 {
                    1.0f32
                } else {
                    -1.0f32
                };
                state.0 = page_index;
                state.1 = current_time;
                state.2 = dir;
            }
            (state.2, state.1)
        });

        let elapsed = (current_time - start_time) as f32;
        let duration = crate::ui::theme::animation_timings(ui.ctx()).page_flip;
        let raw_t = (elapsed / duration).clamp(0.0, 1.0);

        if raw_t < 1.0 {
            ui.ctx().request_repaint();
        }

        let eased = Self::ease_out_cubic(raw_t);
        let x_offset = (1.0 - eased) * (direction * 42.0);
        let opacity = (eased * 1.15).clamp(0.0, 1.0);
        let left_padding = if x_offset > 0.0 { x_offset } else { 0.0 };
        let right_padding = if x_offset < 0.0 { -x_offset } else { 0.0 };

        ui.horizontal(|ui| {
            if left_padding > 0.1 {
                ui.add_space(left_padding);
            }
            ui.vertical(|ui| {
                if right_padding > 0.1 {
                    ui.set_width((ui.available_width() - right_padding).max(100.0));
                } else {
                    ui.set_width(ui.available_width());
                }
                if opacity < 0.999 {
                    ui.set_opacity(opacity);
                }
                add_contents(ui)
            })
            .inner
        })
        .inner
    }
}
