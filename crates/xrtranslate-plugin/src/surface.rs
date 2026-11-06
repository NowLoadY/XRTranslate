//! Plugin-owned egui pages: input in, drawing and the existing Director registry out.
//! No Rust pointers, GPU device or application-specific controls cross the pipe.
#[cfg(feature = "host")]
#[path = "surface_host.rs"]
mod host;
#[cfg(feature = "host")]
pub use host::Renderer;
#[path = "surface_textures.rs"]
mod textures;

pub fn active(context: &egui::Context) -> bool {
    context
        .data(|data| data.get_temp(egui::Id::unique("native_surface")))
        .unwrap_or(false)
}

use crate::{Context, Environment, Plugin, Result, Widget};
use egui::{
    self, Rect,
    epaint::{ImageDelta, Mesh, Primitive},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use ui_director::egui::{
    driver::{Command, Request},
    feedback::LocatedFeedback,
    registry::FrameSnapshot,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub id: String,
    pub page: String,
    pub raw: egui::RawInput,
    #[serde(default)]
    pub files: Vec<std::path::PathBuf>,
    pub rect: Rect,
    pub pixels_per_point: f32,
    pub style: egui::Style,
    pub options: Value,
}

#[derive(Debug)]
struct DroppedPath(std::path::PathBuf);
impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> std::result::Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

/// Shared renderers may provide named paint callbacks without owning plugin state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    pub rect: Rect,
    pub name: String,
    pub data: Value,
}
impl Paint {
    pub fn add(self, painter: &egui::Painter) {
        painter.add(egui::PaintCallback {
            rect: self.rect,
            callback: Arc::new(self),
        });
    }
}
#[derive(Serialize, Deserialize)]
pub enum Drawing {
    Mesh { clip: Rect, mesh: Mesh },
    Paint { clip: Rect, paint: Paint },
}
#[derive(Serialize, Deserialize)]
pub struct Frame {
    pub drawings: Vec<Drawing>,
    #[serde(with = "textures")]
    pub textures: Vec<(egui::TextureId, Vec<ImageDelta>)>,
    pub free: Vec<egui::TextureId>,
    pub platform: egui::PlatformOutput,
    pub snapshot: FrameSnapshot,
    pub feedback: Vec<LocatedFeedback>,
    pub feedback_until: f64,
    pub repaint_after: Duration,
}

pub trait UiPlugin: Plugin {
    /// Install the same shared fonts/theme/image loaders as the shell.
    fn configure_ui(&mut self, _context: &egui::Context, _options: &Value) {}
    fn ui(&mut self, ui: &mut egui::Ui, context: &mut Context);
}

pub fn serve(plugin: impl UiPlugin) -> std::result::Result<(), String> {
    crate::serve(Page {
        plugin,
        context: egui::Context::default(),
        input: None,
    })
}
struct Page<P> {
    plugin: P,
    context: egui::Context,
    input: Option<Input>,
}
impl<P: UiPlugin> Page<P> {
    fn render(
        &mut self,
        input: Input,
        context: &mut Context,
    ) -> std::result::Result<Frame, String> {
        self.context
            .data_mut(|data| data.insert_temp(egui::Id::unique("native_surface"), true));
        self.context.set_pixels_per_point(input.pixels_per_point);
        self.context
            .set_style_of(egui::Theme::Light, input.style.clone());
        self.context
            .set_style_of(egui::Theme::Dark, input.style.clone());
        self.plugin.configure_ui(&self.context, &input.options);
        let mut raw = input.raw.clone();
        raw.dropped_files = input
            .files
            .iter()
            .map(|path| Arc::new(DroppedPath(path.clone())) as egui::DroppedFileHandle)
            .collect();
        ui_director::egui::raw_input(&self.context, &mut raw);
        let mut output = self.context.run_ui(raw, |root| {
            let ctx = root.ctx();
            ui_director::egui::begin_frame(ctx, &input.page, Vec::new());
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique(("plugin_surface", &input.id)),
                egui::UiBuilder::new().max_rect(input.rect),
            );
            ui.set_clip_rect(input.rect);
            self.plugin.ui(&mut ui, context);
            ui_director::egui::finish_frame(&input.page);
        });
        let mut drawings = Vec::new();
        for item in self
            .context
            .tessellate(output.shapes, output.pixels_per_point)
        {
            match item.primitive {
                Primitive::Mesh(mesh) => drawings.push(Drawing::Mesh {
                    clip: item.clip_rect,
                    mesh,
                }),
                Primitive::Callback(callback) => {
                    let paint = callback
                        .callback
                        .downcast_ref::<Paint>()
                        .ok_or("Plugin used an unregistered GPU callback")?;
                    drawings.push(Drawing::Paint {
                        clip: item.clip_rect,
                        paint: paint.clone(),
                    });
                }
            }
        }
        let mut retained = input;
        retained.raw.events.clear();
        retained.raw.dropped_files.clear();
        retained.files.clear();
        self.input = Some(retained);
        Ok(Frame {
            drawings,
            textures: output
                .textures_delta
                .set
                .drain()
                .map(|(id, deltas)| (id, deltas.into_vec()))
                .collect(),
            free: output.textures_delta.free.drain().collect(),
            platform: output.platform_output,
            snapshot: ui_director::egui::driver().snapshot(),
            feedback: ui_director::egui::feedback::current(&self.context),
            feedback_until: ui_director::egui::feedback::expires(&self.context),
            repaint_after: output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(Duration::MAX, |v| v.repaint_delay),
        })
    }
}
impl<P: UiPlugin> Plugin for Page<P> {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> std::result::Result<(), String> {
        self.plugin.initialize(environment, context)?;
        context.view(vec![Widget::Surface]);
        Ok(())
    }
    fn call(&mut self, name: &str, arguments: Value, context: &mut Context) -> Result {
        match name {
            "ui.render" => {
                let input = serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                serde_json::to_value(self.render(input, context)?).map_err(|e| e.to_string())
            }
            "ui.command" => {
                let command: Command =
                    serde_json::from_value(arguments).map_err(|e| e.to_string())?;
                let input = self.input.clone().ok_or("Open the plugin page first")?;
                let (responder, reply) = crossbeam_channel::bounded(1);
                ui_director::egui::driver()
                    .channel()
                    .try_send(Request {
                        command,
                        responder,
                        expires: Instant::now() + Duration::from_secs(30),
                        cancelled: Arc::new(AtomicBool::new(false)),
                    })
                    .map_err(|e| e.to_string())?;
                let frame = self.render(input, context)?;
                context.event(
                    "ui.frame",
                    serde_json::to_value(frame).map_err(|e| e.to_string())?,
                );
                let response = reply.try_recv().map_err(|e| e.to_string())?;
                if response.success {
                    Ok(response.data.unwrap_or(Value::Null))
                } else {
                    Err(response.message)
                }
            }
            _ => self.plugin.call(name, arguments, context),
        }
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        self.plugin.event(name, value, context)
    }
    fn reply(&mut self, id: u64, result: Result, context: &mut Context) {
        self.plugin.reply(id, result, context)
    }
    fn cancel(&mut self, id: u64, context: &mut Context) {
        self.plugin.cancel(id, context)
    }
    fn tick(&mut self, elapsed: Duration, context: &mut Context) -> Option<Vec<Widget>> {
        self.plugin.tick(elapsed, context);
        None
    }
}
