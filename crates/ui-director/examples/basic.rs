//! Run with `cargo run --example basic --features egui -- 18921`.
use eframe::egui;
use ui_director::egui::{self as control, Control, tools::UiTools};

#[derive(Default)]
struct Example {
    tools: UiTools,
    text: String,
    clicks: usize,
}

impl eframe::App for Example {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        control::raw_input(ctx, input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        control::begin_frame(ui.ctx(), "Example", vec![]);
        for request in control::driver().take_host_requests() {
            use control::driver::Command;
            use ui_director::Response;
            if let Command::Call(call) = request.command {
                let response = match ui_director::dispatch(&mut [&mut self.tools], &call) {
                    Ok(value) => Response::ok("Completed", Some(value)),
                    Err(error) => Response::err(error),
                };
                let _ = request.responder.send(response);
            }
        }
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.heading("UI Director");
            ui.label("The same controls work with the mouse, CLI and an agent.");
            let response = ui.button("Run");
            if Control::button("Run").response(ui, response).clicked() {
                self.clicks += 1;
            }
            let response = ui.text_edit_singleline(&mut self.text);
            control::text_response(ui, "Draft", &mut self.text, response);
            let text = format!("Runs: {}", self.clicks);
            let response = ui.label(&text);
            control::record_text_output(ui, &text, &response);
        });
        control::finish_frame("Example");
        control::feedback::paint(
            ui.painter(),
            None,
            egui::Color32::from_rgb(60, 110, 245),
            str::to_owned,
        );
    }
}

fn main() -> eframe::Result<()> {
    let port = std::env::args()
        .nth(1)
        .map(|port| port.parse().expect("port"));
    eframe::run_native(
        "UI Director",
        eframe::NativeOptions::default(),
        Box::new(move |cc| {
            control::init(cc.egui_ctx.clone(), port);
            Ok(Box::new(Example::default()))
        }),
    )
}
