//! Another view of the same companion; behavior and dialogue stay with its owner.
use super::{Gaze, Presentation, PresentationSampler, Speech};
use crate::ui::animation::AnimationSystem;
use eframe::egui::{self, Id, Pos2, Rect};

#[derive(Default)]
pub(crate) struct Surface {
    presentation: Presentation,
    sampler: PresentationSampler,
    speech: Speech,
}

impl Surface {
    pub fn visible(&self) -> bool {
        self.presentation.visible
    }

    pub fn update(&mut self, mut presentation: Presentation) {
        if !presentation.visible {
            // The toolbar handle remains, but hidden dialogue must not retain
            // stale text, placement, opacity, or a native bubble hit region.
            presentation.speech = Speech::default();
            self.speech = Speech::default();
        }
        // Record receipt even if the surface is currently hidden and not painted.
        self.sampler.sample(&presentation);
        self.presentation = presentation;
    }

    pub fn paint(&mut self, ui: &egui::Ui, body: Rect, pointer: Option<Pos2>) -> Option<Rect> {
        if !self.visible() {
            return None;
        }
        let frame = self.sampler.sample(&self.presentation);
        self.speech = frame.speech.on_surface(&self.speech);
        let id = Id::new("desktop_companion");
        let dt = ui.ctx().input(|input| input.stable_dt.min(0.05));
        let center = body.center() + egui::vec2(0.0, 3.0);
        // Keep the existing toolbar hit target while reducing the visible model.
        let radius = body.height() * 0.24;
        let gaze = pointer.map_or(Gaze::default(), |point| Gaze::toward(point - center, 180.0));
        let mut pose = frame.pose;
        // Local attention adds to the owner's nods and expressive head motion.
        pose.gaze.yaw += AnimationSystem::animate_value(ui.ctx(), id.with("yaw"), gaze.yaw, 0.22);
        pose.gaze.pitch +=
            AnimationSystem::animate_value(ui.ctx(), id.with("pitch"), gaze.pitch, 0.22);
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, id));
        frame
            .appearance
            .model()
            .paint(&painter, id, center, radius, pose);
        self.speech.paint(
            &painter,
            ui.ctx().viewport_rect().shrink(4.0),
            center,
            radius,
            frame.clock,
            dt,
        );
        self.speech.bounds()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_presentation_clears_stale_speech() {
        let mut speech = Speech::default();
        speech.say("A previous message", 0.0);
        let mut surface = Surface::default();
        surface.update(Presentation {
            speech: speech.clone(),
            visible: true,
            ..Presentation::default()
        });
        assert!(!surface.presentation.speech.finished(0.0));
        surface.update(Presentation {
            speech,
            visible: false,
            ..Presentation::default()
        });
        assert_eq!(surface.presentation.speech, Speech::default());
        assert!(surface.presentation.speech.bounds().is_none());
    }
}
