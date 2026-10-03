//! Another view of the same companion; behavior and dialogue stay with its owner.
use super::{Classic, Gaze, Presentation, Speech};
use crate::ui::animation::AnimationSystem;
use eframe::egui::{self, Id, Pos2, Rect};

#[derive(Default)]
pub(crate) struct Surface {
    presentation: Presentation,
}

impl Surface {
    pub fn update(&mut self, mut presentation: Presentation) {
        presentation.speech = if presentation.visible {
            presentation.speech.on_surface(&self.presentation.speech)
        } else {
            // The toolbar handle remains, but hidden dialogue must not retain
            // stale text, placement, opacity, or a native bubble hit region.
            Speech::default()
        };
        self.presentation = presentation;
    }

    pub fn paint(&mut self, ui: &egui::Ui, body: Rect, pointer: Option<Pos2>) -> Option<Rect> {
        let id = Id::new("desktop_companion");
        let dt = ui.ctx().input(|input| input.stable_dt.min(0.05));
        let center = body.center() + egui::vec2(0.0, 3.0);
        // Keep the existing toolbar hit target while reducing the visible model.
        let radius = body.height() * 0.24;
        let gaze = pointer.map_or(Gaze::default(), |point| Gaze::toward(point - center, 180.0));
        let mut pose = self.presentation.pose;
        pose.gaze = Gaze {
            yaw: AnimationSystem::animate_value(ui.ctx(), id.with("yaw"), gaze.yaw, 0.22),
            pitch: AnimationSystem::animate_value(ui.ctx(), id.with("pitch"), gaze.pitch, 0.22),
        };
        let painter = ui
            .ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, id));
        Classic::model().paint(&painter, id, center, radius, pose);
        self.presentation.speech.paint(
            &painter,
            ui.ctx().viewport_rect().shrink(4.0),
            center,
            radius,
            self.presentation.clock,
            dt,
        );
        self.presentation.speech.bounds()
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
