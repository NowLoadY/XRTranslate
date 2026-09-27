//! Pointer capture and placement, shared by every application page.
use eframe::egui::{self, Id, PointerButton, Pos2, Rect, Response, Vec2};

/// The body and clothing bounds, shared by hit testing, travel and window clamping.
pub(super) fn footprint(center: Pos2, radius: f32) -> Rect {
    Rect::from_min_max(
        center - egui::vec2(radius * 1.3, radius * 1.7),
        center + egui::vec2(radius * 1.3, radius * 1.1),
    )
}

pub(super) fn constrain(center: Pos2, radius: f32, screen: Rect) -> Pos2 {
    let size = footprint(Pos2::ZERO, radius);
    let screen = screen.shrink(6.0);
    Rect::from_min_max(
        screen.min - size.min.to_vec2(),
        screen.max - size.max.to_vec2(),
    )
    .clamp(center)
}

#[derive(Clone, Default)]
pub(super) struct Placement {
    radius: Option<f32>,
    grab_offset: Option<Vec2>,
}

impl Placement {
    pub fn was_dragged(&self) -> bool {
        self.radius.is_some()
    }

    pub fn radius(&mut self, automatic: f32, resting: f32, dt: f32) -> f32 {
        if let Some(radius) = &mut self.radius {
            // Keep the grabbed silhouette stable; settle gently after release.
            if self.grab_offset.is_none() {
                *radius += (resting - *radius) * (1.0 - (-dt / 0.3).exp());
            }
            *radius
        } else {
            automatic
        }
    }

    pub fn interact(
        &mut self,
        ctx: &egui::Context,
        id: Id,
        center: &mut Pos2,
        radius: f32,
        enabled: bool,
    ) -> Response {
        // Cover the body, cap and scarf, not the transparent render texture.
        let hit = footprint(*center, radius);
        let response = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(hit.min)
            .default_size(hit.size())
            .constrain(false)
            .fade_in(false)
            .enabled(enabled)
            .sense(egui::Sense::CLICK | egui::Sense::DRAG)
            .show(ctx, |ui| {
                ui.allocate_space(hit.size());
            })
            .response;
        if response.drag_started_by(PointerButton::Primary) {
            self.grab_offset =
                ctx.input(|input| input.pointer.press_origin().map(|point| *center - point));
            self.radius = Some(radius);
        }
        if enabled
            && (response.dragged_by(PointerButton::Primary)
                || response.drag_stopped_by(PointerButton::Primary))
            && let Some(offset) = self.grab_offset
            && let Some(point) = response.interact_pointer_pos()
        {
            *center = point + offset;
        }
        if !enabled || !ctx.input(|input| input.pointer.primary_down()) {
            self.grab_offset = None;
        }
        if self.was_dragged() {
            *center = constrain(*center, radius, ctx.viewport_rect());
        }
        response.on_hover_and_drag_cursor(if self.grab_offset.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        })
    }
}
