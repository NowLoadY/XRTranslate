use super::{Drawing, Frame, Input, Paint};
use crate::host::Process;
use egui::{self, TextureHandle, TextureId};
use serde_json::Value;
use std::collections::HashMap;

/// One pending frame bounds latency and memory; input events are never discarded.
#[derive(Default)]
pub struct Renderer {
    pending: Option<u64>,
    frame: Option<Frame>,
    input: Option<Input>,
    textures: HashMap<TextureId, TextureHandle>,
}
impl Renderer {
    pub fn accept(&mut self, ctx: &egui::Context, mut frame: Frame) {
        for (id, deltas) in frame.textures.drain(..) {
            for delta in deltas {
                if let Some(texture) = self.textures.get_mut(&id) {
                    if let Some(position) = delta.pos {
                        texture.set_partial(position, delta.image, delta.options)
                    } else {
                        texture.set(delta.image, delta.options)
                    }
                } else {
                    self.textures.insert(
                        id,
                        ctx.load_texture(format!("plugin:{id:?}"), delta.image, delta.options),
                    );
                }
            }
        }
        for id in frame.free.drain(..) {
            self.textures.remove(&id);
        }
        ctx.output_mut(|output| output.append(std::mem::take(&mut frame.platform)));
        self.frame = Some(frame);
        ctx.request_repaint();
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        page: &str,
        process: &mut Process,
        options: Value,
        mut paint: impl FnMut(&Paint) -> Option<egui::Shape>,
        mut texture: impl FnMut(u64) -> Option<TextureId>,
    ) -> std::result::Result<(), String> {
        if let Some(request) = self.pending
            && let Some(result) = process.result(request)
        {
            self.pending = None;
            self.accept(
                ui.ctx(),
                serde_json::from_value(result?).map_err(|e| e.to_string())?,
            );
        }
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }
        let mut raw = ui.input(|input| input.raw.clone());
        // Shell popups and the companion must not click or scroll the page below.
        // Keep a captured drag through its release, even outside the surface.
        if !(response.contains_pointer()
            || response.dragged()
            || response.drag_stopped()
            || response.is_pointer_button_down_on())
        {
            raw.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::PointerMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseMoved(_)
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                        | egui::Event::Zoom(_)
                        | egui::Event::Rotate(_)
                )
            });
            raw.events.push(egui::Event::PointerGone);
            raw.dropped_files.clear();
            raw.hovered_files.clear();
        }
        // A shell composer/menu owns its keyboard while it has focus.
        if !response.has_focus() {
            raw.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Key { .. }
                        | egui::Event::Ime(_)
                )
            });
        }
        let mut files = raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect::<Vec<_>>();
        if let Some(old) = self.input.take() {
            files.splice(0..0, old.files);
            let mut events = old.raw.events;
            events.append(&mut raw.events);
            raw.events = events;
        }
        self.input = Some(Input {
            id: id.into(),
            page: page.into(),
            raw,
            files,
            rect,
            pixels_per_point: ui.ctx().pixels_per_point(),
            style: (**ui.style()).clone(),
            options,
        });
        if self.pending.is_none() {
            let input = self.input.as_mut().unwrap();
            self.pending = Some(process.request(
                "ui.render",
                serde_json::to_value(&*input).map_err(|e| e.to_string())?,
            )?);
            input.raw.events.clear();
            input.raw.dropped_files.clear();
            input.files.clear();
        }
        if let Some(frame) = &self.frame {
            ui_director::egui::driver().append_surface(id, &frame.snapshot);
            ui_director::egui::feedback::import(
                ui.ctx(),
                frame.feedback.clone(),
                frame.feedback_until,
            );
            for drawing in &frame.drawings {
                match drawing {
                    Drawing::Mesh { clip, mesh } => {
                        let mut mesh = mesh.clone();
                        mesh.texture_id = match mesh.texture_id {
                            TextureId::User(id) => match texture(id) {
                                Some(id) => id,
                                None => continue,
                            },
                            id => match self.textures.get(&id) {
                                Some(handle) => handle.id(),
                                None => continue,
                            },
                        };
                        ui.painter()
                            .with_clip_rect(*clip)
                            .add(egui::Shape::mesh(mesh));
                    }
                    Drawing::Paint {
                        clip,
                        paint: command,
                    } => {
                        if let Some(shape) = paint(command) {
                            ui.painter().with_clip_rect(*clip).add(shape);
                        }
                    }
                }
            }
            ui.ctx().request_repaint_after(frame.repaint_after);
        }
        // While visible, input and background changes are sampled with the shell.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
        Ok(())
    }
}
