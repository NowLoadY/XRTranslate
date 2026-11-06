//! The plugin owns this window, its GPU and its render loop. Only commands and
//! state use the host connection; the host never receives this window's frames.
mod renderer;
use eframe::egui;
use serde_json::Value;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use xrtranslate_client_core::ui::{self, components};
use xrtranslate_plugin::{Context, Environment, Plugin, Widget};

#[derive(Default)]
struct Shared {
    context: OnceLock<egui::Context>,
    visible: AtomicBool,
    stopped: AtomicBool,
}
impl Shared {
    fn wake(&self) {
        if let Some(ctx) = self.context.get() {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
    }
}
struct Bridge(Arc<Shared>);
impl Plugin for Bridge {
    fn initialize(&mut self, _: Environment, context: &mut Context) -> Result<(), String> {
        context.view(vec![Widget::Button {
            id: "open".into(),
            label: "Open canvas".into(),
            enabled: true,
            primary: false,
        }]);
        Ok(())
    }
    fn call(&mut self, _: &str, _: Value, _: &mut Context) -> xrtranslate_plugin::Result {
        Err("Use Open canvas".into())
    }
    fn event(&mut self, name: &str, value: Value, _: &mut Context) {
        if name == "ui" && value["id"] == "open" {
            self.0.visible.store(true, Ordering::Relaxed);
            self.0.wake();
        }
    }
}

struct Window {
    tools: ui_director::egui::tools::UiTools,
    shared: Arc<Shared>,
    gpu: Arc<renderer::Renderer>,
    shown: bool,
    standalone: bool,
    paused: bool,
    phase: f32,
    last_frame: std::time::Instant,
}
impl eframe::App for Window {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        ui_director::egui::raw_input(ctx, input);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        if self.shared.stopped.load(Ordering::Relaxed) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.standalone && ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.shared.visible.store(false, Ordering::Relaxed);
        }
        let visible = self.shared.visible.load(Ordering::Relaxed);
        if self.shown != visible {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(visible));
            if visible {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.shown = visible;
        }
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        if !visible {
            return;
        }
        ui_director::egui::begin_frame(ui.ctx(), "canvas", Vec::new());
        for request in ui_director::egui::driver().take_host_requests() {
            if let ui_director::egui::driver::Command::Call(call) = request.command {
                let response = match ui_director::dispatch(&mut [&mut self.tools], &call) {
                    Ok(value) => ui_director::Response::ok("Completed", Some(value)),
                    Err(error) => ui_director::Response::err(error),
                };
                let _ = request.responder.send(response);
            }
        }
        ui.horizontal(|ui| {
            if components::button(ui, if self.paused { "Play" } else { "Pause" }).clicked() {
                self.paused = !self.paused;
            }
            if components::button(ui, "Reset").clicked() {
                self.phase = 0.0;
            }
            if components::button(ui, "Full screen").clicked() {
                let full = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if !self.paused {
            self.phase += elapsed;
            ui.ctx().request_repaint();
        }
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
        if response.dragged() {
            self.phase += response.drag_delta().x * 0.01;
        }
        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                renderer::Paint {
                    renderer: self.gpu.clone(),
                    phase: self.phase,
                },
            ));
        ui_director::egui::finish_frame("canvas");
    }
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args().collect();
    let standalone = args.iter().any(|s| s == "--standalone");
    let director = args
        .windows(2)
        .find(|a| a[0] == "--director-port")
        .and_then(|a| a[1].parse::<u16>().ok());
    let shared = Arc::new(Shared::default());
    shared.visible.store(standalone, Ordering::Relaxed);
    if !standalone {
        let state = shared.clone();
        std::thread::spawn(move || {
            if let Err(error) = xrtranslate_plugin::serve(Bridge(state.clone())) {
                eprintln!("{error}");
            }
            state.stopped.store(true, Ordering::Relaxed);
            state.wake();
        });
    }
    eframe::run_native(
        "Canvas example",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Wgpu,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([960.0, 600.0])
                .with_visible(standalone),
            ..Default::default()
        },
        Box::new(move |cc| {
            ui::configure_surface(&cc.egui_ctx, &serde_json::json!({}));
            if let Some(port) = director {
                ui_director::egui::init(cc.egui_ctx.clone(), Some(port));
            }
            let _ = shared.context.set(cc.egui_ctx.clone());
            let state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or("GPU renderer unavailable")?;
            Ok(Box::new(Window {
                tools: Default::default(),
                gpu: Arc::new(renderer::Renderer::new(state)),
                shared,
                shown: standalone,
                standalone,
                paused: false,
                phase: 0.0,
                last_frame: std::time::Instant::now(),
            }))
        }),
    )
}
