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
        let id = Id::new("page_transition").with(std::any::type_name::<P>());
        let duration = crate::ui::theme::animation_timings(ui.ctx()).page;
        Self::render_transition(
            ui,
            id,
            Id::new(page_id).value(),
            duration,
            false,
            add_contents,
        )
    }

    /// Wizard navigation shares the same motion, with direction following its step order.
    pub fn render_page_flip_transition<R>(
        ui: &mut egui::Ui,
        page_index: usize,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let duration = crate::ui::theme::animation_timings(ui.ctx()).page_flip;
        Self::render_transition(
            ui,
            Id::new("onboarding_transition"),
            page_index as u64,
            duration,
            true,
            add_contents,
        )
    }

    fn render_transition<R>(
        ui: &mut egui::Ui,
        id: Id,
        page: u64,
        duration: f32,
        directional: bool,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let now = ui.input(|input| input.time);
        let (started, direction) = ui.ctx().data_mut(|data| {
            let state = data.get_temp_mut_or_insert_with(id, || (page, now, 1.0f32));
            if state.0 != page {
                state.2 = if directional && page < state.0 {
                    -1.0
                } else {
                    1.0
                };
                state.0 = page;
                state.1 = now;
            }
            (state.1, state.2)
        });
        let t = if duration > 0.0 {
            ((now - started) as f32 / duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if t < 1.0 {
            ui.ctx().request_repaint();
        }
        let eased = Self::ease_out_cubic(t);
        let parent = ui.layer_id();
        let layer = if t < 1.0 {
            let layer = egui::LayerId::new(parent.order, id.with("content"));
            let offset = egui::vec2(direction * (1.0 - eased) * 18.0, 0.0);
            let transform = ui
                .ctx()
                .layer_transform_to_global(parent)
                .unwrap_or_default()
                * egui::emath::TSTransform::from_translation(offset);
            ui.ctx().set_sublayer(parent, layer);
            ui.ctx().set_transform_layer(layer, transform);
            layer
        } else {
            // ScrollArea hover detection uses registered area layers. Release the
            // temporary paint layer once settled so native wheel input works.
            parent
        };
        // Transform painting and hit testing together; never animate layout padding or width.
        ui.scope_builder(egui::UiBuilder::new().layer_id(layer), |ui| {
            ui.multiply_opacity(0.25 + 0.75 * eased);
            crate::ui::layout::contain_width(ui, add_contents)
        })
        .inner
    }
}
