//! Discover attention targets from egui instead of wiring behavior into each page.
use super::{OnboardingLayout, placement};
use eframe::egui::{self, Id, Pos2, Rect, Response};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Attention {
    Feature(usize),
    Next,
    Control(Id),
    Avatar,
}

impl Attention {
    pub fn bit(self) -> u8 {
        match self {
            Self::Feature(index) => 1 << index,
            Self::Next => 1 << 3,
            Self::Control(_) | Self::Avatar => 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Target {
    pub attention: Attention,
    rect: Rect,
}

impl Target {
    pub fn read(
        ctx: &egui::Context,
        layout: Option<&OnboardingLayout>,
        avatar: &Response,
    ) -> Option<Self> {
        if avatar.hovered() {
            return Some(Self {
                attention: Attention::Avatar,
                rect: avatar.rect,
            });
        }
        // Only the first welcome page follows controls; other pages park.
        let layout = layout?;
        layout.features?;
        if let Some(pointer) = ctx.pointer_hover_pos() {
            if let Some(cards) = layout.features
                && let Some(index) = cards.iter().position(|rect| rect.contains(pointer))
            {
                return Some(Self {
                    attention: Attention::Feature(index),
                    rect: cards[index],
                });
            }
            if layout.next.contains(pointer) {
                return Some(Self {
                    attention: Attention::Next,
                    rect: layout.next,
                });
            }
        }
        let hovered = ctx
            .interaction_snapshot(|snapshot| snapshot.hovered.iter().copied().collect::<Vec<_>>());
        let read = |id| {
            let response = ctx.read_response(id)?;
            if !response.enabled()
                || !response.sense.interactive()
                || response.layer_id == avatar.layer_id
            {
                return None;
            }
            let mut rect = response.interact_rect;
            if let Some(transform) = ctx.layer_transform_to_global(response.layer_id) {
                rect = transform * rect;
            }
            rect = rect.intersect(ctx.viewport_rect());
            (rect.is_finite() && rect.is_positive()).then_some(Self {
                attention: Attention::Control(id),
                rect,
            })
        };
        hovered
            .into_iter()
            .filter_map(read)
            .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
            .or_else(|| ctx.memory(|memory| memory.focused()).and_then(read))
    }

    pub fn destination(self, current: Pos2, radius: f32, screen: Rect) -> Pos2 {
        if self.attention == Attention::Avatar {
            return current;
        }
        let body = placement::footprint(Pos2::ZERO, radius);
        let top = -body.min.to_vec2();
        let bottom = body.max.to_vec2();
        let center = self.rect.center();
        // Choose the nearest clear side, keeping both the avatar and focused control visible.
        [
            egui::pos2(self.rect.right() + top.x + 18.0, center.y),
            egui::pos2(self.rect.left() - bottom.x - 18.0, center.y),
            egui::pos2(center.x, self.rect.top() - bottom.y - 18.0),
            egui::pos2(center.x, self.rect.bottom() + top.y + 18.0),
        ]
        .into_iter()
        .map(|point| placement::constrain(point, radius, screen))
        .filter(|point| !placement::footprint(*point, radius).intersects(self.rect.expand(8.0)))
        .min_by(|a, b| a.distance_sq(current).total_cmp(&b.distance_sq(current)))
        .unwrap_or(current)
    }
}
