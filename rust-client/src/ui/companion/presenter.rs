//! A temporary, externally authored performance using the companion's existing rig.
//! No appearance, speech, or placement settings are persisted by a recording.
use super::{Expression, Gaze, Motion};
use crate::ui::components::avatar::Appearance;
use eframe::egui::{self, Id, LayerId, Order};
use serde::Deserialize;
use std::time::{Duration, Instant};
use ui_director::{Tool, ToolCall, ToolProvider, ToolResult};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cue {
    at: f32,
    /// Normalized viewport point; omitted means looking at the viewer.
    look: Option<[f32; 2]>,
    #[serde(default)]
    expression: Expression,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    duration: f32,
    center: [f32; 2],
    radius: f32,
    #[serde(default)]
    cues: Vec<Cue>,
    /// Normalized audio amplitude sampled at 30 Hz, including silent intervals.
    #[serde(default)]
    speech: Vec<f32>,
}

impl Scene {
    fn validate(&self) -> Result<(), String> {
        let point = |p: [f32; 2]| p.into_iter().all(|v| (0.0..=1.0).contains(&v));
        if !(0.1..=120.0).contains(&self.duration)
            || !(16.0..=160.0).contains(&self.radius)
            || !point(self.center)
            || self.cues.len() > 120
            || self.cues.iter().any(|cue| {
                !(0.0..self.duration).contains(&cue.at) || cue.look.is_some_and(|look| !point(look))
            })
            || self.cues.windows(2).any(|pair| pair[0].at >= pair[1].at)
            || self.speech.len() > (self.duration * 30.0).ceil() as usize
            || self.speech.iter().any(|v| !(0.0..=1.0).contains(v))
        {
            return Err("Invalid presenter timing, position, size, or audio envelope".into());
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Presenter {
    scene: Option<(Instant, Scene)>,
    motion: Motion,
}

impl Presenter {
    pub(crate) fn paint(&mut self, ctx: &egui::Context, appearance: &Appearance) -> bool {
        let Some((started, scene)) = &self.scene else {
            return false;
        };
        let elapsed = started.elapsed().as_secs_f32();
        if elapsed >= scene.duration {
            self.scene = None;
            return false;
        }
        let screen = ctx.viewport_rect();
        let point = |p: [f32; 2]| screen.min + screen.size() * egui::vec2(p[0], p[1]);
        let center = point(scene.center);
        let cue = scene.cues.iter().rev().find(|cue| cue.at <= elapsed);
        let gaze = cue
            .and_then(|cue| cue.look)
            .map_or(Gaze::default(), |look| {
                Gaze::toward(point(look) - center, screen.width() * 0.55)
            });
        let sample = elapsed * 30.0;
        let index = sample as usize;
        let a = scene.speech.get(index).copied().unwrap_or(0.0);
        let b = scene.speech.get(index + 1).copied().unwrap_or(0.0);
        let speech = a + (b - a) * sample.fract();
        let pose = self.motion.advance(
            ctx.input(|input| input.time),
            cue.map_or(Expression::Calm, |cue| cue.expression),
            gaze,
            speech,
        );
        let id = Id::unique("companion_presenter");
        appearance.model().paint(
            &ctx.layer_painter(LayerId::new(Order::Tooltip, id)),
            id,
            center,
            scene.radius,
            pose,
        );
        ctx.request_repaint_after(Duration::from_millis(16));
        true
    }
}

impl ToolProvider for Presenter {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool::new(
            "companion.present",
            "Temporarily perform a recording scene with the existing Avatar. Coordinates are normalized to the viewport, radius is in logical points, speech is amplitude at 30 Hz. The scene expires automatically; null restores normal behavior immediately.",
            serde_json::json!({"scene": {
                "type": ["object", "null"],
                "properties": {
                    "duration": {"type":"number", "minimum":0.1, "maximum":120},
                    "center": {"type":"array", "items":{"type":"number"}, "minItems":2, "maxItems":2},
                    "radius": {"type":"number", "minimum":16, "maximum":160},
                    "cues": {"type":"array", "items":{
                        "type":"object", "properties":{
                            "at":{"type":"number"},
                            "look":{"type":["array","null"], "items":{"type":"number"}, "minItems":2, "maxItems":2},
                            "expression":{"enum":["Calm","Happy","Curious","Thinking","Concerned"]}
                        }, "required":["at"], "additionalProperties":false
                    }},
                    "speech":{"type":"array", "items":{"type":"number", "minimum":0, "maximum":1}}
                },
                "required":["duration","center","radius"], "additionalProperties":false
            }}),
            &["scene"],
        )]
    }

    fn call(&mut self, call: &ToolCall) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            scene: Option<Scene>,
        }
        if call.name != "companion.present" {
            return Err("Unknown presenter operation".into());
        }
        let scene = ui_director::arguments::<Input>(call)?.scene;
        if let Some(scene) = &scene {
            scene.validate()?;
        }
        self.scene = scene.map(|scene| (Instant::now(), scene));
        Ok(serde_json::json!({"active":self.scene.is_some()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsorted_cues_and_out_of_bounds_envelopes() {
        let mut scene: Scene = serde_json::from_value(serde_json::json!({
            "duration":2, "center":[0.3,0.7], "radius":80,
            "cues":[{"at":1},{"at":0}]
        }))
        .unwrap();
        assert!(scene.validate().is_err());
        scene.cues.reverse();
        assert!(scene.validate().is_ok());
        scene.speech = vec![0.5; 61];
        assert!(scene.validate().is_err());
        scene.speech = vec![f32::NAN];
        assert!(scene.validate().is_err());
    }
}
