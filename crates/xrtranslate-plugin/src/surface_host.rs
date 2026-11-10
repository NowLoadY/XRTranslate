use super::{Drawing, Frame, Input, Paint};
use crate::host::Process;
use egui::{self, TextureHandle, TextureId};
use serde_json::Value;
use std::collections::HashMap;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// One pending frame bounds latency and memory; input events are never discarded.
#[derive(Default)]
pub struct Renderer {
    pending: Option<u64>,
    frame: Option<Frame>,
    input: Option<Input>,
    textures: HashMap<TextureId, TextureHandle>,
    drawings: Vec<Drawing>,
    texture_ids: Vec<Option<TextureId>>,
    paint_callbacks: Vec<Option<egui::Shape>>,
    repaint_at: Option<Instant>,
    last_request: Option<Instant>,
    pointer: bool,
    focused: bool,
}

impl Renderer {
    pub fn content_bounds(&self) -> Option<&super::ContentBounds> {
        self.frame.as_ref()?.content_bounds.as_ref()
    }

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
        // Commands/events happen once; cursor and IME describe the retained UI.
        ctx.output_mut(|output| {
            output.commands.append(&mut frame.platform.commands);
            output.events.append(&mut frame.platform.events);
            if let Some(update) = frame.platform.accesskit_update.take() {
                output.accesskit_update = Some(update);
            }
        });
        if !frame.reuse_drawings {
            self.drawings = std::mem::take(&mut frame.drawings);
            self.paint_callbacks.clear();
            self.texture_ids = self
                .drawings
                .iter()
                .map(|drawing| match drawing {
                    Drawing::Mesh { mesh, .. } => Some(mesh.texture_id),
                    Drawing::Paint { .. } => None,
                })
                .collect();
        }
        self.repaint_after(ctx, frame.repaint_after);
        self.frame = Some(frame);
        ctx.request_repaint();
    }

    pub fn repaint_after(&mut self, ctx: &egui::Context, delay: Duration) {
        if let Some(deadline) = Instant::now().checked_add(delay) {
            self.repaint_at = Some(self.repaint_at.map_or(deadline, |old| old.min(deadline)));
            ctx.request_repaint_after(delay);
        }
    }

    fn apply_platform(&mut self, ctx: &egui::Context, pointer: bool, focus: bool) {
        let Some(frame) = &mut self.frame else {
            return;
        };
        ctx.output_mut(|output| {
            if pointer {
                output.cursor_icon = frame.platform.cursor_icon;
                output.cursor_image = frame.platform.cursor_image.clone();
                output.mutable_text_under_cursor = frame.platform.mutable_text_under_cursor;
            }
            if focus {
                output.ime = frame.platform.ime;
                if let Some(ime) = &mut frame.platform.ime {
                    ime.should_interrupt_composition = false;
                }
            }
        });
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
        // Texture deltas are ordered; consume every frame, including command frames.
        for frame in process.frames() {
            self.accept(ui.ctx(), frame);
        }
        if let Some(request) = self.pending
            && let Some(result) = process.result(request)
        {
            self.pending = None;
            let value = result?;
            // Older SDKs return a JSON frame; negotiated frames arrive above.
            if !value.is_null() {
                self.accept(
                    ui.ctx(),
                    serde_json::from_value(value).map_err(|e| e.to_string())?,
                );
            }
        }
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        if self.frame.is_none() {
            ui.put(
                egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0)),
                egui::Spinner::new().size(18.0),
            );
        }
        if response.clicked() {
            response.request_focus();
        }
        let mut raw = ui.input(|input| input.raw.clone());
        // Shell popups and the companion must not click or scroll the page below.
        // Keep a captured drag through its release, even outside the surface.
        let pointer = response.contains_pointer()
            || response.dragged()
            || response.drag_stopped()
            || response.is_pointer_button_down_on();
        self.apply_platform(ui.ctx(), pointer, response.has_focus());
        if !pointer {
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
            if self.pointer {
                raw.events.push(egui::Event::PointerGone);
            }
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
        let mut changed = self.pointer != pointer || self.focused != response.has_focus();
        self.pointer = pointer;
        self.focused = response.has_focus();
        let style = ui.style();
        if let Some(old) = &self.input {
            changed |= old.rect != rect
                || old.clip_rect != Some(ui.clip_rect())
                || old.pixels_per_point != ui.ctx().pixels_per_point()
                || old.style != **style
                || old.options != options
                || old.raw.focused != raw.focused
                || old.raw.screen_rect != raw.screen_rect;
        } else {
            changed = true;
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
        changed |= !raw.events.is_empty() || !files.is_empty() || !raw.hovered_files.is_empty();
        self.input = Some(Input {
            id: id.into(),
            page: page.into(),
            raw,
            files,
            rect,
            clip_rect: Some(ui.clip_rect()),
            pixels_per_point: ui.ctx().pixels_per_point(),
            style: (**style).clone(),
            options,
        });
        if changed {
            self.repaint_after(ui.ctx(), Duration::ZERO);
        }
        let now = Instant::now();
        // Older plugins and existing polling workers can change state without a
        // repaint callback. Sample those at 4 Hz, independently of shell frames.
        let poll_at = self
            .last_request
            .map_or(now, |last| last + Duration::from_millis(250));
        let due = self.repaint_at.map_or(poll_at, |at| at.min(poll_at));
        // One outstanding request and a frame-rate bound prevent reply/repaint
        // feedback from outrunning the display while retaining all input events.
        let due = self
            .last_request
            .map_or(due, |last| due.max(last + Duration::from_millis(16)));
        if self.pending.is_none() && now >= due {
            let input = self.input.as_mut().unwrap();
            self.pending = Some(process.request(
                "ui.render",
                serde_json::to_value(&*input).map_err(|e| e.to_string())?,
            )?);
            self.last_request = Some(now);
            self.repaint_at = None;
            input.raw.events.clear();
            input.raw.dropped_files.clear();
            input.files.clear();
        } else if self.pending.is_none() {
            ui.ctx()
                .request_repaint_after(due.saturating_duration_since(now));
        }
        if let Some(frame) = &self.frame {
            ui_director::egui::driver().append_surface(id, &frame.snapshot);
            ui_director::egui::feedback::import(
                ui.ctx(),
                frame.feedback.clone(),
                frame.feedback_until,
            );
            if self.paint_callbacks.len() != self.drawings.len() {
                self.paint_callbacks = self
                    .drawings
                    .iter()
                    .map(|drawing| match drawing {
                        Drawing::Paint { paint: command, .. } => paint(command),
                        Drawing::Mesh { .. } => None,
                    })
                    .collect();
            }
            for (index, drawing) in self.drawings.iter_mut().enumerate() {
                match drawing {
                    Drawing::Mesh { clip, mesh } => {
                        let texture_id = match self.texture_ids[index].unwrap() {
                            TextureId::User(id) => match texture(id) {
                                Some(id) => id,
                                None => continue,
                            },
                            id => match self.textures.get(&id) {
                                Some(handle) => handle.id(),
                                None => continue,
                            },
                        };
                        if mesh.texture_id != texture_id {
                            Arc::make_mut(mesh).texture_id = texture_id;
                        }
                        ui.painter()
                            .with_clip_rect(*clip)
                            .add(egui::Shape::mesh(mesh.clone()));
                    }
                    Drawing::Paint { clip, .. } => {
                        if let Some(shape) = &self.paint_callbacks[index] {
                            ui.painter().with_clip_rect(*clip).add(shape.clone());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(platform: egui::PlatformOutput) -> Frame {
        Frame {
            drawings: Vec::new(),
            reuse_drawings: false,
            content_bounds: None,
            textures: Vec::new(),
            free: Vec::new(),
            platform,
            snapshot: Default::default(),
            feedback: Vec::new(),
            feedback_until: 0.0,
            repaint_after: std::time::Duration::MAX,
        }
    }

    #[test]
    fn retained_surface_keeps_cursor_and_ime_without_repeating_commands_or_stealing_focus() {
        let ctx = egui::Context::default();
        let mut renderer = Renderer::default();
        let ime = egui::output::IMEOutput {
            purpose: egui::IMEPurpose::Normal,
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 30.0)),
            cursor_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 30.0)),
            should_interrupt_composition: true,
        };
        let mut incoming = Some(frame(egui::PlatformOutput {
            cursor_icon: egui::CursorIcon::Text,
            mutable_text_under_cursor: true,
            ime: Some(ime),
            commands: vec![egui::OutputCommand::CopyText("selected text".into())],
            ..Default::default()
        }));
        // The shell renders repeatedly while the plugin is still producing its next frame.
        for tick in 0..6 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                if let Some(frame) = incoming.take() {
                    renderer.accept(ui.ctx(), frame);
                }
                renderer.apply_platform(ui.ctx(), true, true);
            });
            output.textures_delta.clear();
            let output = output.platform_output;
            assert_eq!(
                output.cursor_icon,
                egui::CursorIcon::Text,
                "host frame {tick}"
            );
            assert!(output.mutable_text_under_cursor);
            assert_eq!(output.commands.len(), usize::from(tick == 0));
            assert_eq!(output.ime.unwrap().cursor_rect, ime.cursor_rect);
            assert_eq!(output.ime.unwrap().should_interrupt_composition, tick == 0);
        }
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            // A new plugin reply must not override a shell popup or its text editor.
            renderer.accept(
                ui.ctx(),
                frame(egui::PlatformOutput {
                    cursor_icon: egui::CursorIcon::Text,
                    ime: Some(ime),
                    ..Default::default()
                }),
            );
            ui.ctx().output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Password,
                    ..ime
                })
            });
            renderer.apply_platform(ui.ctx(), false, false);
        });
        output.textures_delta.clear();
        let output = output.platform_output;
        assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand);
        assert_eq!(output.ime.unwrap().purpose, egui::IMEPurpose::Password);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            renderer.accept(ui.ctx(), frame(Default::default()));
            renderer.apply_platform(ui.ctx(), true, true);
        });
        output.textures_delta.clear();
        let output = output.platform_output;
        assert_eq!(output.cursor_icon, egui::CursorIcon::Default);
        assert!(output.ime.is_none());
    }
}
